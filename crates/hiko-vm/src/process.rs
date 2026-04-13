//! Hiko process: an isolated VM execution unit.

use std::collections::VecDeque;

use crate::sendable::SendableValue;
use crate::vm::VM;

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
#[derive(Debug)]
pub enum BlockReason {
    /// Waiting for a message in the mailbox.
    Receive,
    /// Waiting for a child process to complete.
    Await(Pid),
    /// Waiting for an I/O operation to complete.
    Io(crate::io_backend::IoToken),
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
    Failed(String),
}

/// An isolated hiko process.
pub struct Process {
    pub pid: Pid,
    pub vm: VM,
    pub mailbox: VecDeque<SendableValue>,
    pub status: ProcessStatus,
    pub parent: Option<Pid>,
    /// The process's return value (set when Done).
    pub result: Option<SendableValue>,
    /// The scope this process belongs to.
    pub scope_id: Option<ScopeId>,
    /// Cooperative cancellation flag. Checked at suspension/resume points.
    pub cancelled: bool,
}

impl Process {
    pub fn new(pid: Pid, vm: VM, parent: Option<Pid>) -> Self {
        Self {
            pid,
            vm,
            mailbox: VecDeque::new(),
            status: ProcessStatus::Runnable,
            parent,
            result: None,
            scope_id: None,
            cancelled: false,
        }
    }

    pub fn new_in_scope(pid: Pid, vm: VM, parent: Option<Pid>, scope_id: ScopeId) -> Self {
        Self {
            pid,
            vm,
            mailbox: VecDeque::new(),
            status: ProcessStatus::Runnable,
            parent,
            result: None,
            scope_id: Some(scope_id),
            cancelled: false,
        }
    }

    pub fn is_runnable(&self) -> bool {
        matches!(self.status, ProcessStatus::Runnable)
    }

    pub fn is_done(&self) -> bool {
        matches!(self.status, ProcessStatus::Done | ProcessStatus::Failed(_))
    }
}
