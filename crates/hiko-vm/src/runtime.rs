//! Runtime: single-threaded scheduler loop for running multiple hiko processes.
//!
//! The lifecycle rules match the threaded runtime (see `threaded.rs`): every
//! terminal transition goes through [`Runtime::terminate`], which records the
//! outcome, cancels the process's children, reaps their unconsumed tombstones,
//! and wakes the processes waiting on it. Builtins that do I/O block the
//! thread here, so a process is never blocked on I/O.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::process::{
    AwaitKind, BlockReason, ChildOutcome, ChildRecord, Pid, Process, ProcessFailure, ProcessStatus,
};
use crate::runtime_ops::{ChildState, dedup_pids, deliver_already_joined, deliver_await_outcome};
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::sendable::{SendableValue, serialize};
use crate::value::Value;
use crate::vm::{RunResult, VM};
use hiko_compile::chunk::CompiledProgram;

/// Processes waiting to be terminated; see [`Runtime::terminate`].
type Terminations = Vec<(Pid, ChildOutcome)>;

/// The hiko runtime — manages processes and scheduling.
pub struct Runtime {
    next_pid: AtomicU64,
    /// Live processes, plus terminated root processes.
    processes: HashMap<Pid, Process>,
    /// Outcomes of terminated children that their parent has not consumed.
    tombstones: HashMap<Pid, ChildRecord>,
    scheduler: Box<dyn Scheduler>,
    /// Processes waiting for another process to finish: child_pid → [waiter_pids]
    waiters: HashMap<Pid, Vec<Pid>>,
    /// Processes waiting for any child in a set to finish: child_pid → [waiter_pids]
    any_waiters: HashMap<Pid, Vec<Pid>>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::with_scheduler(Box::new(FifoScheduler::new(1000)))
    }
}

impl Runtime {
    /// Create a new runtime with the default FIFO scheduler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a runtime with a custom scheduler.
    pub fn with_scheduler(scheduler: Box<dyn Scheduler>) -> Self {
        Self {
            next_pid: AtomicU64::new(1),
            processes: HashMap::new(),
            tombstones: HashMap::new(),
            scheduler,
            waiters: HashMap::new(),
            any_waiters: HashMap::new(),
        }
    }

    /// Allocate a new process ID.
    fn new_pid(&self) -> Pid {
        Pid(self.next_pid.fetch_add(1, Ordering::Relaxed))
    }

    fn missing_process_message(context: &str, pid: Pid) -> String {
        format!("{context}: process {:?} missing from runtime table", pid)
    }

    fn is_live(&self, pid: Pid) -> bool {
        self.processes.get(&pid).is_some_and(|p| !p.is_done())
    }

    /// Spawn a root process from a compiled program.
    /// Returns the Pid.
    pub fn spawn_root(&mut self, program: CompiledProgram) -> Pid {
        self.spawn_root_vm(VM::new(program))
    }

    /// Spawn a root process from an already-configured VM.
    /// Returns the Pid.
    pub fn spawn_root_vm(&mut self, mut vm: VM) -> Pid {
        let pid = self.new_pid();
        vm.enable_output_capture();
        let process = Process::new(pid, vm, None);
        self.processes.insert(pid, process);
        self.scheduler.enqueue(pid);
        pid
    }

    /// Run all processes to completion (single-threaded).
    /// Returns the root process's output lines.
    pub fn run_to_completion(&mut self) -> Result<Vec<String>, String> {
        self.run_with_probe(&mut |_| {})
    }

    /// `run_to_completion`, calling `probe` after every slice.
    fn run_with_probe(&mut self, probe: &mut dyn FnMut(&Self)) -> Result<Vec<String>, String> {
        loop {
            while let Some(pid) = self.try_dequeue() {
                self.run_one_slice(pid)?;
                probe(self);
            }
            // Safety net, as in the threaded runtime: with correct
            // bookkeeping no process stays blocked once nothing is runnable.
            let stuck: Vec<Pid> = self
                .processes
                .iter()
                .filter(|(_, p)| matches!(p.status, ProcessStatus::Blocked(_)))
                .map(|(pid, _)| *pid)
                .collect();
            if stuck.is_empty() {
                break;
            }
            let mut terminations = Terminations::new();
            for pid in stuck {
                self.clear_blocked_registration(pid);
                terminations.push((
                    pid,
                    ChildOutcome::Err(ProcessFailure::runtime(
                        "deadlock: process blocked with no possible waker",
                    )),
                ));
            }
            self.drain_terminations(&mut terminations);
        }

        // Collect output from all processes (root first)
        let mut all_output = Vec::new();
        for process in self.processes.values() {
            all_output.extend(process.vm.get_output().iter().cloned());
        }
        Ok(all_output)
    }

    fn run_one_slice(&mut self, pid: Pid) -> Result<(), String> {
        let reductions = self.scheduler.reductions(pid);

        let result = {
            let Some(process) = self.processes.get_mut(&pid) else {
                return Err(Self::missing_process_message("run", pid));
            };
            if !process.is_runnable() {
                // Stale queue entry.
                return Ok(());
            }
            process.vm.run_slice(reductions)
        };

        match result {
            RunResult::Done => {
                let Some(process) = self.processes.get_mut(&pid) else {
                    return Err(Self::missing_process_message("finish", pid));
                };
                let outcome = if process.parent.is_some() {
                    child_result(process)
                } else {
                    ChildOutcome::Ok(SendableValue::Unit)
                };
                self.terminate(pid, outcome);
            }
            RunResult::Yielded => {
                self.scheduler.enqueue(pid);
            }
            RunResult::Failed(failure) => {
                self.terminate(pid, ChildOutcome::Err(failure));
            }
            RunResult::Spawn {
                proto_idx,
                captures,
            } => match self.handle_spawn(pid, proto_idx, captures) {
                Ok(child_pid) => {
                    let Some(process) = self.processes.get_mut(&pid) else {
                        return Err(Self::missing_process_message("spawn", pid));
                    };
                    process.vm.stack.pop();
                    process.vm.push_value(Value::Pid(child_pid.0));
                    self.scheduler.enqueue(pid);
                }
                Err(failure) => {
                    self.terminate(pid, ChildOutcome::Err(failure));
                }
            },
            RunResult::Await(child_pid_val) => {
                self.handle_await(pid, Pid(child_pid_val), AwaitKind::Raw);
            }
            RunResult::AwaitResult(child_pid_val) => {
                self.handle_await(pid, Pid(child_pid_val), AwaitKind::Result);
            }
            RunResult::Cancel(child_pid_val) => {
                self.handle_cancel(pid, Pid(child_pid_val));
            }
            RunResult::WaitAny(child_pid_vals) => {
                let child_pids = child_pid_vals.into_iter().map(Pid).collect();
                self.handle_wait_any(pid, child_pids);
            }
            RunResult::Io(_req) => {
                self.terminate(
                    pid,
                    ChildOutcome::Err(ProcessFailure::runtime(
                        "async I/O requires ThreadedRuntime",
                    )),
                );
            }
            RunResult::Cancelled => {
                self.terminate(pid, ChildOutcome::Err(ProcessFailure::Cancelled));
            }
        }
        Ok(())
    }

    /// Handle a spawn request: create child process from closure prototype.
    fn handle_spawn(
        &mut self,
        parent_pid: Pid,
        proto_idx: usize,
        captures: Vec<SendableValue>,
    ) -> Result<Pid, ProcessFailure> {
        let parent = self.processes.get(&parent_pid).ok_or_else(|| {
            ProcessFailure::runtime(Self::missing_process_message("spawn", parent_pid))
        })?;
        let child_vm =
            crate::runtime_ops::create_child_vm_from_parent(&parent.vm, proto_idx, captures)?;
        let child_pid = self.new_pid();
        let child = Process::new(child_pid, child_vm, Some(parent_pid));
        self.processes.insert(child_pid, child);
        if let Some(parent) = self.processes.get_mut(&parent_pid) {
            parent.children.insert(child_pid);
        }
        self.scheduler.enqueue(child_pid);
        Ok(child_pid)
    }

    // --- Terminal transitions ---------------------------------------------------

    /// Make `pid` terminal. The single entry point for terminal transitions.
    fn terminate(&mut self, pid: Pid, outcome: ChildOutcome) {
        let mut terminations = vec![(pid, outcome)];
        self.drain_terminations(&mut terminations);
    }

    fn fail(&mut self, pid: Pid, failure: ProcessFailure) {
        self.terminate(pid, ChildOutcome::Err(failure));
    }

    fn drain_terminations(&mut self, terminations: &mut Terminations) {
        while let Some((pid, outcome)) = terminations.pop() {
            self.finish_process(pid, outcome, terminations);
        }
    }

    /// Record the outcome (a tombstone for a child whose parent is alive, a
    /// terminal status for a root), cancel live children, reap the tombstones
    /// of terminated ones, and wake waiters.
    fn finish_process(&mut self, pid: Pid, outcome: ChildOutcome, terminations: &mut Terminations) {
        if !self.is_live(pid) {
            // Already terminated (queued twice, e.g. by a cancel cascade).
            return;
        }
        let Some(mut process) = self.processes.remove(&pid) else {
            return;
        };
        let children = std::mem::take(&mut process.children);
        self.scheduler.remove(pid);

        match process.parent {
            Some(parent) => {
                drop(process);
                if self.is_live(parent) {
                    self.tombstones.insert(pid, ChildRecord { parent, outcome });
                }
            }
            None => {
                process.status = match outcome {
                    ChildOutcome::Ok(_) => ProcessStatus::Done,
                    ChildOutcome::Err(failure) => ProcessStatus::Failed(failure),
                };
                self.processes.insert(pid, process);
            }
        }

        // Scope exit: children must not outlive their parent.
        for child in children {
            if self.is_live(child) {
                self.cancel_process(child, terminations);
            }
            self.tombstones.remove(&child);
        }

        self.wake_any_waiters(pid, terminations);
        self.wake_join_waiters(pid, terminations);
    }

    /// Cancel a live process. A blocked process terminates right away; a
    /// runnable one observes the request at the start of its next slice.
    fn cancel_process(&mut self, pid: Pid, terminations: &mut Terminations) {
        let Some(process) = self.processes.get_mut(&pid) else {
            return;
        };
        match process.status {
            ProcessStatus::Blocked(_) => {
                self.clear_blocked_registration(pid);
                terminations.push((pid, ChildOutcome::Err(ProcessFailure::Cancelled)));
            }
            ProcessStatus::Runnable => process.vm.request_cancellation(),
            ProcessStatus::Done | ProcessStatus::Failed(_) => {}
        }
    }

    fn clear_blocked_registration(&mut self, pid: Pid) {
        let Some(ProcessStatus::Blocked(reason)) = self.processes.get(&pid).map(|p| &p.status)
        else {
            return;
        };
        match reason.clone() {
            BlockReason::Await { child, .. } => remove_waiter(&mut self.waiters, child, pid),
            BlockReason::WaitAny(child_pids) => self.remove_wait_any_registration(pid, &child_pids),
            BlockReason::Io(_) => {}
        }
    }

    // --- Process operations -------------------------------------------------------

    fn classify_child(&self, parent_pid: Pid, child_pid: Pid) -> ChildState {
        let is_child = self
            .processes
            .get(&parent_pid)
            .is_some_and(|p| p.children.contains(&child_pid));
        if is_child {
            if self.is_live(child_pid) {
                ChildState::Live
            } else {
                ChildState::Terminated
            }
        } else if self.processes.contains_key(&child_pid)
            || self.tombstones.contains_key(&child_pid)
        {
            ChildState::NotChild
        } else if child_pid.0 > 0 && child_pid.0 < self.next_pid.load(Ordering::Relaxed) {
            ChildState::Reaped
        } else {
            ChildState::Unknown
        }
    }

    /// Resume a process with the result of a runtime operation, or fail it.
    fn resume_or_fail(&mut self, pid: Pid, delivered: Result<(), ProcessFailure>) {
        match delivered {
            Ok(()) => {
                if let Some(process) = self.processes.get_mut(&pid) {
                    process.status = ProcessStatus::Runnable;
                    self.scheduler.enqueue(pid);
                }
            }
            Err(failure) => self.fail(pid, failure),
        }
    }

    /// Deliver a terminated child's outcome to its parent, consuming it.
    fn consume_child(&mut self, parent_pid: Pid, child_pid: Pid, await_kind: AwaitKind) {
        let record = self.tombstones.remove(&child_pid);
        let Some(parent) = self.processes.get_mut(&parent_pid) else {
            return;
        };
        parent.children.remove(&child_pid);
        let delivered = match record {
            Some(record) => deliver_await_outcome(&mut parent.vm, await_kind, record.outcome),
            None => deliver_already_joined(&mut parent.vm, await_kind),
        };
        self.resume_or_fail(parent_pid, delivered);
    }

    /// Handle an await request.
    fn handle_await(&mut self, parent_pid: Pid, child_pid: Pid, await_kind: AwaitKind) {
        match self.classify_child(parent_pid, child_pid) {
            ChildState::Live => {
                let Some(parent) = self.processes.get_mut(&parent_pid) else {
                    return;
                };
                parent.status = ProcessStatus::Blocked(BlockReason::Await {
                    child: child_pid,
                    kind: await_kind,
                });
                self.waiters.entry(child_pid).or_default().push(parent_pid);
            }
            ChildState::Terminated => self.consume_child(parent_pid, child_pid, await_kind),
            ChildState::Reaped => {
                let Some(parent) = self.processes.get_mut(&parent_pid) else {
                    return;
                };
                let delivered = deliver_already_joined(&mut parent.vm, await_kind);
                self.resume_or_fail(parent_pid, delivered);
            }
            ChildState::NotChild => self.fail(
                parent_pid,
                ProcessFailure::runtime(format!(
                    "await: process {child_pid:?} is not a child of {parent_pid:?}"
                )),
            ),
            ChildState::Unknown => self.fail(
                parent_pid,
                ProcessFailure::runtime(format!("await: unknown process {child_pid:?}")),
            ),
        }
    }

    fn handle_cancel(&mut self, parent_pid: Pid, child_pid: Pid) {
        let failure = match self.classify_child(parent_pid, child_pid) {
            ChildState::Live => {
                let mut terminations = Terminations::new();
                self.cancel_process(child_pid, &mut terminations);
                self.drain_terminations(&mut terminations);
                None
            }
            // Cancelling a finished child is a no-op.
            ChildState::Terminated | ChildState::Reaped => None,
            ChildState::NotChild => Some(format!(
                "cancel: process {child_pid:?} is not a child of {parent_pid:?}"
            )),
            ChildState::Unknown => Some(format!("cancel: unknown process {child_pid:?}")),
        };

        match failure {
            None => {
                if let Some(parent) = self.processes.get_mut(&parent_pid) {
                    parent.vm.stack.pop();
                    parent.vm.push_value(Value::Unit);
                    parent.status = ProcessStatus::Runnable;
                    self.scheduler.enqueue(parent_pid);
                }
            }
            Some(message) => self.fail(parent_pid, ProcessFailure::runtime(message)),
        }
    }

    fn handle_wait_any(&mut self, parent_pid: Pid, child_pids: Vec<Pid>) {
        let child_pids = dedup_pids(child_pids);
        if child_pids.is_empty() {
            self.fail(
                parent_pid,
                ProcessFailure::runtime("wait_any: expected non-empty pid list"),
            );
            return;
        }

        for &child_pid in &child_pids {
            let failure = match self.classify_child(parent_pid, child_pid) {
                ChildState::Live => continue,
                // Already terminal: the leftmost such pid wins immediately.
                ChildState::Terminated | ChildState::Reaped => {
                    let Some(parent) = self.processes.get_mut(&parent_pid) else {
                        return;
                    };
                    let delivered =
                        crate::runtime_ops::deliver_pid_to_parent(&mut parent.vm, child_pid);
                    self.resume_or_fail(parent_pid, delivered);
                    return;
                }
                ChildState::NotChild => {
                    format!("wait_any: process {child_pid:?} is not a child of {parent_pid:?}")
                }
                ChildState::Unknown => format!("wait_any: unknown process {child_pid:?}"),
            };
            self.fail(parent_pid, ProcessFailure::runtime(failure));
            return;
        }

        let Some(parent) = self.processes.get_mut(&parent_pid) else {
            return;
        };
        parent.status = ProcessStatus::Blocked(BlockReason::WaitAny(child_pids.clone()));
        for child_pid in child_pids {
            self.any_waiters
                .entry(child_pid)
                .or_default()
                .push(parent_pid);
        }
    }

    fn remove_wait_any_registration(&mut self, parent_pid: Pid, child_pids: &[Pid]) {
        for &child_pid in child_pids {
            remove_waiter(&mut self.any_waiters, child_pid, parent_pid);
        }
    }

    // --- Waking ---------------------------------------------------------------------

    fn wake_any_waiters(&mut self, finished_pid: Pid, terminations: &mut Terminations) {
        let Some(waiter_pids) = self.any_waiters.remove(&finished_pid) else {
            return;
        };

        for waiter_pid in waiter_pids {
            let child_pids = match self.processes.get(&waiter_pid).map(|p| &p.status) {
                Some(ProcessStatus::Blocked(BlockReason::WaitAny(child_pids))) => {
                    child_pids.clone()
                }
                _ => continue,
            };

            // The finished pid is only a notification; the winner is the
            // leftmost terminated pid in the original list.
            let Some(winner) = child_pids
                .iter()
                .copied()
                .find(|child_pid| !self.is_live(*child_pid))
            else {
                continue;
            };

            self.remove_wait_any_registration(waiter_pid, &child_pids);
            let Some(process) = self.processes.get_mut(&waiter_pid) else {
                continue;
            };
            match crate::runtime_ops::deliver_pid_to_parent(&mut process.vm, winner) {
                Ok(()) => {
                    process.status = ProcessStatus::Runnable;
                    self.scheduler.enqueue(waiter_pid);
                }
                Err(failure) => terminations.push((waiter_pid, ChildOutcome::Err(failure))),
            }
        }
    }

    fn wake_join_waiters(&mut self, finished_pid: Pid, terminations: &mut Terminations) {
        let Some(waiter_pids) = self.waiters.remove(&finished_pid) else {
            return;
        };

        for waiter_pid in waiter_pids {
            let await_kind = match self.processes.get(&waiter_pid).map(|p| &p.status) {
                Some(ProcessStatus::Blocked(BlockReason::Await { child, kind }))
                    if *child == finished_pid =>
                {
                    *kind
                }
                _ => continue,
            };

            // Only the parent may await, so delivery consumes the tombstone.
            let record = self.tombstones.remove(&finished_pid);
            let Some(process) = self.processes.get_mut(&waiter_pid) else {
                continue;
            };
            process.children.remove(&finished_pid);
            let delivered = match record {
                Some(record) => deliver_await_outcome(&mut process.vm, await_kind, record.outcome),
                None => deliver_already_joined(&mut process.vm, await_kind),
            };
            match delivered {
                Ok(()) => {
                    process.status = ProcessStatus::Runnable;
                    self.scheduler.enqueue(waiter_pid);
                }
                Err(failure) => terminations.push((waiter_pid, ChildOutcome::Err(failure))),
            }
        }
    }

    /// Try to dequeue a runnable process without blocking.
    /// Returns None when no runnable processes remain.
    fn try_dequeue(&self) -> Option<Pid> {
        self.scheduler.try_dequeue()
    }

    /// Get a process's output.
    pub fn get_output(&self, pid: Pid) -> Vec<String> {
        self.processes
            .get(&pid)
            .map(|p| p.vm.get_output().to_vec())
            .unwrap_or_default()
    }

    /// Get the current lifecycle status of a process.
    pub fn get_status(&self, pid: Pid) -> Option<&ProcessStatus> {
        self.processes.get(&pid).map(|p| &p.status)
    }

    /// Get the root VM's last error span, if any.
    pub fn get_error_span(&self, pid: Pid) -> Option<hiko_syntax::span::Span> {
        self.processes.get(&pid).and_then(|p| p.vm.error_span())
    }

    /// Get the number of processes.
    pub fn process_count(&self) -> usize {
        self.processes.len()
    }
}

#[cfg(test)]
impl Runtime {
    /// Observable result of `pid` plus any bookkeeping left behind.
    pub(crate) fn lifecycle_report(&self, pid: Pid) -> crate::lifecycle_tests::Report {
        let mut leftovers = Vec::new();
        let mut note = |name: &str, len: usize| {
            if len > 0 {
                leftovers.push(format!("{name}={len}"));
            }
        };
        note("extra_processes", self.processes.len().saturating_sub(1));
        note("tombstones", self.tombstones.len());
        note("waiters", self.waiters.len());
        note("any_waiters", self.any_waiters.len());
        crate::lifecycle_tests::Report {
            output: self.get_output(pid),
            status: self
                .get_status(pid)
                .map(|status| format!("{status:?}"))
                .unwrap_or_else(|| "<missing>".into()),
            leftovers,
        }
    }

    /// Run to completion and return the largest tombstone count seen after
    /// any slice.
    pub(crate) fn run_tracking_max_tombstones(&mut self) -> Result<usize, String> {
        let mut max = 0;
        self.run_with_probe(&mut |runtime| max = max.max(runtime.tombstones.len()))?;
        Ok(max)
    }
}

/// Serialize the final value of a finished child for its parent.
fn child_result(process: &mut Process) -> ChildOutcome {
    let val = process.vm.stack.last().copied().unwrap_or(Value::Unit);
    match process
        .vm
        .heap
        .charge_host_work(crate::sendable::SENDABLE_BOUNDARY_BASE_HOST_WORK)
        .map_err(ProcessFailure::runtime)
        .and_then(|()| {
            serialize(val, &process.vm.heap)
                .map_err(|e| ProcessFailure::runtime(format!("child result not sendable: {e}")))
        })
        .and_then(|sv| {
            process
                .vm
                .heap
                .charge_host_work(crate::sendable::host_work_for_sendable(&sv))
                .map_err(ProcessFailure::runtime)?;
            Ok(sv)
        }) {
        Ok(sv) => ChildOutcome::Ok(sv),
        Err(failure) => ChildOutcome::Err(failure),
    }
}

fn remove_waiter(waiters: &mut HashMap<Pid, Vec<Pid>>, child_pid: Pid, waiter_pid: Pid) {
    let should_remove = if let Some(waiter_pids) = waiters.get_mut(&child_pid) {
        waiter_pids.retain(|pid| *pid != waiter_pid);
        waiter_pids.is_empty()
    } else {
        false
    };
    if should_remove {
        waiters.remove(&child_pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;
    use std::path::Path;

    fn compile(source: &str) -> CompiledProgram {
        let tokens = Lexer::new(source, 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile(program).unwrap();
        compiled
    }

    struct MissingPidScheduler;

    impl Scheduler for MissingPidScheduler {
        fn enqueue(&self, _pid: Pid) {}
        fn dequeue(&self) -> Option<Pid> {
            None
        }
        fn remove(&self, _pid: Pid) {}
        fn reductions(&self, _pid: Pid) -> u64 {
            1000
        }
        fn try_dequeue(&self) -> Option<Pid> {
            Some(Pid(999))
        }
        fn shutdown(&self) {}
    }

    #[test]
    fn runtime_reports_missing_scheduled_process() {
        let mut runtime = Runtime::with_scheduler(Box::new(MissingPidScheduler));
        let err = runtime.run_to_completion().unwrap_err();
        assert!(err.contains("run: process Pid(999) missing from runtime table"));
    }

    #[test]
    fn handle_spawn_reports_missing_parent_process() {
        let mut runtime = Runtime::new();
        let err = runtime.handle_spawn(Pid(404), 0, Vec::new()).unwrap_err();
        assert_eq!(
            err,
            ProcessFailure::RuntimeError(
                "spawn: process Pid(404) missing from runtime table".into()
            )
        );
    }

    /// Compile an example in place against the repository's `hiko.lock.toml`,
    /// resolving `Std` from `libraries/` (no module server, no user cache).
    fn compile_example(path: &Path) -> CompiledProgram {
        crate::test_support::compile_file_hermetic(path)
    }

    #[test]
    fn test_single_process_runs_to_completion() {
        let program = compile("val x = 42");
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        let result = runtime.run_to_completion();
        assert!(result.is_ok());
        assert!(runtime.processes[&pid].is_done());
    }

    #[test]
    fn test_single_process_output() {
        let program = compile("val _ = println \"hello from process\"");
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.get_output(pid);
        assert_eq!(output, vec!["hello from process\n"]);
    }

    #[test]
    fn test_process_failure() {
        let program = compile("val _ = panic \"boom\"");
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        match &runtime.processes[&pid].status {
            ProcessStatus::Failed(msg) => assert!(msg.to_string().contains("boom")),
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    #[test]
    fn test_run_slice_yields() {
        let program = compile(
            "fun loop n = if n = 0 then () else loop (n - 1)\n\
             val _ = loop 10000",
        );
        let mut vm = VM::new(program);
        // Run with very few reductions — should yield
        let result = vm.run_slice(100);
        assert!(matches!(result, RunResult::Yielded));

        // Continue running with more fuel — should complete
        let result = vm.run_slice(1_000_000);
        assert!(matches!(result, RunResult::Done));
    }

    #[test]
    fn test_run_slice_completes() {
        let program = compile("val x = 1 + 1");
        let mut vm = VM::new(program);
        let result = vm.run_slice(1000);
        assert!(matches!(result, RunResult::Done));
    }

    #[test]
    fn test_runtime_with_yielding_process() {
        // A process that needs many reductions
        let program = compile(
            "fun loop n = if n = 0 then () else loop (n - 1)\n\
             val _ = loop 5000\n\
             val _ = println \"done\"",
        );
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        assert!(runtime.processes[&pid].is_done());
        let output = runtime.get_output(pid);
        assert_eq!(output, vec!["done\n"]);
    }

    #[test]
    fn test_spawn_and_await_basic() {
        let program = compile(
            "val child = spawn (fn () => 42)\n\
             val result = await_process child\n\
             val _ = println (int_to_string result)",
        );
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        assert!(runtime.processes[&pid].is_done());
        let output = runtime.get_output(pid);
        assert_eq!(output, vec!["42\n"]);
    }

    #[test]
    fn test_spawn_with_captured_value() {
        // Use let-binding to force closure capture (top-level vals are globals,
        // not captured by closures in the child VM)
        let program = compile(
            "fun make_spawner x = spawn (fn () => x + 32)\n\
             val child = make_spawner 10\n\
             val result = await_process child\n\
             val _ = println (int_to_string result)",
        );
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.get_output(pid);
        assert_eq!(output, vec!["42\n"]);
    }

    #[test]
    fn test_spawn_two_children() {
        let program = compile(
            "val c1 = spawn (fn () => 10)\n\
             val c2 = spawn (fn () => 20)\n\
             val r1 = await_process c1\n\
             val r2 = await_process c2\n\
             val _ = println (int_to_string (r1 + r2))",
        );
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.get_output(pid);
        assert_eq!(output, vec!["30\n"]);
    }

    #[test]
    fn test_wait_any_returns_leftmost_completed_child() {
        let program = compile(
            "val left = spawn (fn () => 10)\n\
             val right = spawn (fn () => 20)\n\
             val winner = wait_any [left, right]\n\
             val result = await_process winner\n\
             val _ = println (int_to_string result)",
        );
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.get_output(pid);
        assert_eq!(output, vec!["10\n"]);
    }

    #[test]
    fn test_spawn_stress_example() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/spawn_stress.hml");
        let program = compile_example(&path);
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();

        let output = runtime.get_output(pid).join("");
        assert!(output.contains("spawn_stress ok: 6000 children"));
    }

    #[test]
    fn test_spawn_stress_example_verifies() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/spawn_stress.hml");
        let program = compile_example(&path);
        if let Err(err) = VM::try_new(program) {
            panic!("verifier rejected spawn_stress example: {err}");
        }
    }

    #[test]
    fn test_root_process_may_finish_with_non_sendable_value() {
        let program = compile("val f = fn () => 1");
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        assert!(matches!(runtime.get_status(pid), Some(ProcessStatus::Done)));
    }

    #[test]
    fn test_reaps_finished_child_after_await() {
        let program = compile(
            "val child = spawn (fn () => 42)\n\
             val result = await_process child\n\
             val _ = println (int_to_string result)",
        );
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();

        assert_eq!(runtime.process_count(), 1);
        assert!(runtime.get_status(pid).is_some());
    }

    #[test]
    fn test_reaps_failed_child_after_await() {
        let program = compile(
            "val child = spawn (fn () => panic \"boom\")\n\
             val _ = await_process child",
        );
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();

        assert_eq!(runtime.process_count(), 1);
        assert!(matches!(
            runtime.get_status(pid),
            Some(ProcessStatus::Failed(_))
        ));
    }
}
