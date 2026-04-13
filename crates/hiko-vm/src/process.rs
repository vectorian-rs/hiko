//! Hiko process: an isolated VM execution unit.

use std::collections::VecDeque;

use crate::sendable::SendableValue;
use crate::vm::VM;

/// Unique process identifier.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Pid(pub u64);

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
        }
    }

    pub fn is_runnable(&self) -> bool {
        matches!(self.status, ProcessStatus::Runnable)
    }

    pub fn is_done(&self) -> bool {
        matches!(self.status, ProcessStatus::Done | ProcessStatus::Failed(_))
    }
}
