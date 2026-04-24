//! Hiko process: an isolated VM execution unit.

use crate::sendable::SendableValue;
use crate::vm::VM;
use std::fmt;

/// Unique process identifier.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Pid(pub u64);

/// Unique scope identifier.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ScopeId(pub u64);

/// An ownership boundary for child processes and I/O operations.
/// When a scope exits, all children must be completed or cancelled.
pub struct Scope {
    pub id: ScopeId,
    pub owner: Pid,
    pub children: Vec<Pid>,
}

/// Why a process is blocked.
#[derive(Clone, Debug)]
pub enum BlockReason {
    /// Waiting for a child process to complete.
    Await { child: Pid, kind: AwaitKind },
    /// Waiting for any child in the set to complete.
    WaitAny(Vec<Pid>),
    /// Waiting for an I/O operation to complete.
    Io(crate::io_backend::IoToken),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AwaitKind {
    Raw,
    Result,
}

/// Process lifecycle status.
#[derive(Debug)]
pub enum ProcessStatus {
    /// Ready to be scheduled.
    Runnable,
    /// Waiting for an external event.
    Blocked(BlockReason),
    /// Finished successfully.
    Done,
    /// Finished with an error.
    Failed(ProcessFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessFailure {
    RuntimeError(String),
    HeapObjectLimitExceeded { limit: usize, live: usize },
    FuelExhausted,
    Cancelled,
    ChildProcessFailed(Box<ProcessFailure>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FiberJoinError {
    RuntimeError(String),
    HeapObjectLimitExceeded { limit: usize, live: usize },
    FuelExhausted,
    Cancelled,
    AlreadyJoined,
}

impl FiberJoinError {
    pub fn from_process_failure(failure: ProcessFailure) -> Self {
        match failure {
            ProcessFailure::RuntimeError(message) => Self::RuntimeError(message),
            ProcessFailure::HeapObjectLimitExceeded { limit, live } => {
                Self::HeapObjectLimitExceeded { limit, live }
            }
            ProcessFailure::FuelExhausted => Self::FuelExhausted,
            ProcessFailure::Cancelled => Self::Cancelled,
            ProcessFailure::ChildProcessFailed(cause) => Self::from_process_failure(*cause),
        }
    }
}

impl fmt::Display for FiberJoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeError(message) => f.write_str(message),
            Self::HeapObjectLimitExceeded { limit, live } => {
                write!(f, "heap limit exceeded: {live} objects (max {limit})")
            }
            Self::FuelExhausted => f.write_str("fuel exhausted (max_fuel limit reached)"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::AlreadyJoined => f.write_str("fiber already joined"),
        }
    }
}

/// Compact outcome stored when a child process terminates.
#[derive(Debug, Clone)]
pub enum ChildOutcome {
    Ok(SendableValue),
    Err(ProcessFailure),
}

/// Compact tombstone replacing a full Process after termination.
/// Stores just enough state for subsequent await/cancel operations.
#[derive(Debug, Clone)]
pub enum ChildRecord {
    /// Result available for joining.
    Ready { parent: Pid, outcome: ChildOutcome },
    /// Already joined; further awaits return AlreadyJoined.
    Consumed { parent: Pid },
}

impl ChildRecord {
    pub fn parent(&self) -> Pid {
        match self {
            Self::Ready { parent, .. } | Self::Consumed { parent } => *parent,
        }
    }
}

impl ProcessFailure {
    pub fn runtime(message: impl Into<String>) -> Self {
        Self::RuntimeError(message.into())
    }

    pub fn from_runtime_message(message: String) -> Self {
        if message == "fuel exhausted (max_fuel limit reached)" {
            return Self::FuelExhausted;
        }

        if let Some((live, limit)) = parse_heap_limit_message(&message) {
            return Self::HeapObjectLimitExceeded { limit, live };
        }

        Self::RuntimeError(message)
    }
}

impl fmt::Display for ProcessFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeError(message) => f.write_str(message),
            Self::HeapObjectLimitExceeded { limit, live } => {
                write!(f, "heap limit exceeded: {live} objects (max {limit})")
            }
            Self::FuelExhausted => f.write_str("fuel exhausted (max_fuel limit reached)"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::ChildProcessFailed(cause) => write!(f, "child process failed: {cause}"),
        }
    }
}

fn parse_heap_limit_message(message: &str) -> Option<(usize, usize)> {
    let suffix = message.strip_prefix("heap limit exceeded: ")?;
    let (live, limit) = suffix.split_once(" objects (max ")?;
    let limit = limit.strip_suffix(')')?;
    Some((live.parse().ok()?, limit.parse().ok()?))
}

/// An isolated hiko process.
pub struct Process {
    pub(crate) pid: Pid,
    pub(crate) vm: VM,
    pub(crate) status: ProcessStatus,
    pub(crate) parent: Option<Pid>,
    /// The scope this process belongs to.
    pub(crate) scope_id: Option<ScopeId>,
}

impl Process {
    pub fn new(pid: Pid, vm: VM, parent: Option<Pid>) -> Self {
        Self {
            pid,
            vm,
            status: ProcessStatus::Runnable,
            parent,
            scope_id: None,
        }
    }

    pub fn new_in_scope(pid: Pid, vm: VM, parent: Option<Pid>, scope_id: ScopeId) -> Self {
        Self {
            pid,
            vm,
            status: ProcessStatus::Runnable,
            parent,
            scope_id: Some(scope_id),
        }
    }

    pub fn pid(&self) -> Pid {
        self.pid
    }

    pub fn vm(&self) -> &VM {
        &self.vm
    }

    pub fn status(&self) -> &ProcessStatus {
        &self.status
    }

    pub fn parent(&self) -> Option<Pid> {
        self.parent
    }

    pub fn scope_id(&self) -> Option<ScopeId> {
        self.scope_id
    }

    pub fn is_runnable(&self) -> bool {
        matches!(self.status, ProcessStatus::Runnable)
    }

    pub fn is_done(&self) -> bool {
        matches!(self.status, ProcessStatus::Done | ProcessStatus::Failed(_))
    }
}
