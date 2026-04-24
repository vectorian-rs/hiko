//! Multi-threaded runtime: N worker threads executing hiko processes in parallel.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::{DashMap, DashSet};

use crate::io_backend::{IoBackend, IoToken, ThreadPoolIoBackend};
use crate::process::{
    AwaitKind, BlockReason, ChildOutcome, ChildRecord, FiberJoinError, Pid, Process,
    ProcessFailure, ProcessStatus, Scope, ScopeId,
};
use crate::runtime_ops::{dedup_pids, deliver_join_result_to_parent, deliver_result_to_parent};
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::value::Value;
use crate::vm::{RunResult, VM};
use hiko_compile::chunk::CompiledProgram;

/// Thread-safe process table using DashMap for fine-grained locking.
struct ProcessTable {
    processes: DashMap<Pid, Process>,
    /// Compact tombstones for terminated child processes.
    tombstones: DashMap<Pid, ChildRecord>,
    /// Permanent child→parent map set at spawn time. Lets operations recognise
    /// a child that is temporarily invisible (taken from `processes` for execution).
    child_parents: DashMap<Pid, Pid>,
    /// Children that should be cancelled when they return from execution.
    pending_cancels: DashSet<Pid>,
    waiters: DashMap<Pid, Vec<Pid>>,
    any_waiters: DashMap<Pid, Vec<Pid>>,
    io_waiters: DashMap<IoToken, Pid>,
    #[allow(dead_code)] // scaffolding for structured concurrency
    scopes: DashMap<ScopeId, Scope>,
}

impl ProcessTable {
    fn new() -> Self {
        Self {
            processes: DashMap::new(),
            tombstones: DashMap::new(),
            child_parents: DashMap::new(),
            pending_cancels: DashSet::new(),
            waiters: DashMap::new(),
            any_waiters: DashMap::new(),
            io_waiters: DashMap::new(),
            scopes: DashMap::new(),
        }
    }

    fn insert(&self, process: Process) {
        self.processes.insert(process.pid, process);
    }

    fn take(&self, pid: Pid) -> Option<Process> {
        self.processes.remove(&pid).map(|(_, p)| p)
    }

    fn return_process(&self, process: Process) {
        self.processes.insert(process.pid, process);
    }

    #[cfg(test)]
    fn get_output(&self, pid: Pid) -> Vec<String> {
        self.processes
            .get(&pid)
            .map(|p| p.vm.get_output().to_vec())
            .unwrap_or_default()
    }

    fn all_outputs(&self) -> Vec<String> {
        let mut out = Vec::new();
        for entry in self.processes.iter() {
            out.extend(entry.value().vm.get_output().iter().cloned());
        }
        out
    }

    fn is_all_done_or_blocked(&self) -> bool {
        self.processes
            .iter()
            .all(|e| e.is_done() || matches!(e.status, ProcessStatus::Blocked(_)))
    }

    /// Check if all non-done processes are permanently blocked
    /// (currently only blocked on Await with no possible waker).
    fn has_permanently_blocked(&self) -> bool {
        let io_count = self.io_waiters.len();
        if io_count > 0 {
            return false; // I/O may still complete
        }
        // If no I/O pending and no runnable processes, remaining blocked
        // processes are deadlocked
        self.processes.iter().any(|e| {
            matches!(
                e.status,
                ProcessStatus::Blocked(BlockReason::Await { .. })
                    | ProcessStatus::Blocked(BlockReason::WaitAny(_))
            )
        }) && !self.processes.iter().any(|e| e.is_runnable())
    }
}

/// Multi-threaded hiko runtime.
pub struct ThreadedRuntime {
    next_pid: Arc<AtomicU64>,
    next_io_token: Arc<AtomicU64>,
    active_workers: Arc<std::sync::atomic::AtomicUsize>,
    table: Arc<ProcessTable>,
    scheduler: Arc<dyn Scheduler>,
    io_backend: Arc<dyn IoBackend>,
    num_workers: usize,
}

impl ThreadedRuntime {
    pub fn new(num_workers: usize) -> Self {
        Self {
            next_pid: Arc::new(AtomicU64::new(1)),
            next_io_token: Arc::new(AtomicU64::new(1)),
            active_workers: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            table: Arc::new(ProcessTable::new()),
            scheduler: Arc::new(FifoScheduler::new(1000)),
            io_backend: Arc::new(ThreadPoolIoBackend::new(num_workers.max(2))),
            num_workers,
        }
    }

    pub fn with_io_backend(mut self, backend: Arc<dyn IoBackend>) -> Self {
        self.io_backend = backend;
        self
    }

    fn new_pid(&self) -> Pid {
        Pid(self.next_pid.fetch_add(1, Ordering::Relaxed))
    }

    pub fn spawn_root(&self, program: CompiledProgram) -> Pid {
        let pid = self.new_pid();
        let mut vm = VM::new(program);
        vm.enable_output_capture();
        vm.set_async_io(true);
        let process = Process::new(pid, vm, None);
        self.table.insert(process);
        self.scheduler.enqueue(pid);
        pid
    }

    /// Run all processes to completion using N worker threads.
    pub fn run_to_completion(&self) -> Result<Vec<String>, String> {
        let handles: Vec<_> = (0..self.num_workers)
            .map(|_| {
                let table = Arc::clone(&self.table);
                let scheduler = Arc::clone(&self.scheduler);
                let next_pid = Arc::clone(&self.next_pid);
                let next_io_token = Arc::clone(&self.next_io_token);
                let active_workers = Arc::clone(&self.active_workers);
                let io_backend = Arc::clone(&self.io_backend);

                std::thread::spawn(move || {
                    worker_loop(
                        &table,
                        &*scheduler,
                        &next_pid,
                        &next_io_token,
                        &active_workers,
                        &*io_backend,
                    );
                })
            })
            .collect();

        // Monitor: poll I/O completions and check for termination
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1));

            // Poll I/O backend for completed operations
            let completions = self.io_backend.poll();
            for (token, result) in completions {
                if let Some((_, pid)) = self.table.io_waiters.remove(&token)
                    && let Some(mut process) = self.table.processes.get_mut(&pid)
                {
                    match result {
                        crate::io_backend::IoResult::Ok { value, io_bytes } => {
                            match process
                                .vm
                                .heap
                                .charge_io_bytes(io_bytes)
                                .map_err(|e| ProcessFailure::runtime(e.to_string()))
                                .and_then(|()| deliver_result_to_parent(&mut process.vm, value))
                            {
                                Ok(()) => {
                                    process.status = ProcessStatus::Runnable;
                                    drop(process);
                                    self.scheduler.enqueue(pid);
                                }
                                Err(failure) => {
                                    process.status = ProcessStatus::Failed(failure);
                                    drop(process);
                                    self.scheduler.remove(pid);
                                    wake_any_waiters(&self.table, &*self.scheduler, pid);
                                    wake_join_waiters(&self.table, &*self.scheduler, pid);
                                }
                            }
                        }
                        crate::io_backend::IoResult::Err(msg) => {
                            process.status = match process.vm.heap.charge_io_bytes(msg.len() as u64)
                            {
                                Ok(()) => ProcessStatus::Failed(ProcessFailure::runtime(msg)),
                                Err(err) => {
                                    ProcessStatus::Failed(ProcessFailure::runtime(err.to_string()))
                                }
                            };
                            drop(process);
                            self.scheduler.remove(pid);
                            wake_any_waiters(&self.table, &*self.scheduler, pid);
                            wake_join_waiters(&self.table, &*self.scheduler, pid);
                        }
                    }
                }
            }

            // `active_workers` is part of the shutdown/deadlock decision, so
            // observe worker activity with acquire semantics before reading the
            // process table state used to classify termination.
            if self.table.is_all_done_or_blocked()
                && self.active_workers.load(Ordering::Acquire) == 0
            {
                let has_io_waiters = !self.table.io_waiters.is_empty();
                if !has_io_waiters {
                    if self.table.has_permanently_blocked() {
                        // Collect pids of permanently blocked processes
                        let stuck: Vec<Pid> = self
                            .table
                            .processes
                            .iter()
                            .filter(|e| {
                                matches!(
                                    e.status,
                                    ProcessStatus::Blocked(BlockReason::Await { .. })
                                        | ProcessStatus::Blocked(BlockReason::WaitAny(_))
                                )
                            })
                            .map(|e| *e.key())
                            .collect();

                        // Mark as failed and atomically clean up waiters
                        for &pid in &stuck {
                            if let Some(mut entry) = self.table.processes.get_mut(&pid) {
                                entry.status = ProcessStatus::Failed(ProcessFailure::runtime(
                                    "deadlock: process blocked with no possible waker",
                                ));
                            }
                            // Clear any waiters that were waiting on this process
                            if let Some((_, waiter_pids)) = self.table.waiters.remove(&pid) {
                                for waiter_pid in waiter_pids {
                                    if let Some(mut waiter) =
                                        self.table.processes.get_mut(&waiter_pid)
                                        && matches!(waiter.status, ProcessStatus::Blocked(_))
                                    {
                                        waiter.status =
                                            ProcessStatus::Failed(ProcessFailure::runtime(
                                                "deadlock: child process deadlocked",
                                            ));
                                    }
                                }
                            }
                        }
                    }
                    self.scheduler.shutdown();
                    break;
                }
            }
        }

        for h in handles {
            h.join().unwrap();
        }

        Ok(self.table.all_outputs())
    }
}

/// Transition a process to terminal state. For children, creates a compact
/// tombstone and drops the Process (freeing VM/heap). For root processes,
/// sets the status and returns to the table.
fn make_terminal(table: &ProcessTable, process: Process, outcome: ChildOutcome) {
    let pid = process.pid;
    if let Some(parent_pid) = process.parent {
        table.tombstones.insert(
            pid,
            ChildRecord::Ready {
                parent: parent_pid,
                outcome,
            },
        );
        // process is dropped here, freeing VM/heap/stack
    } else {
        let mut process = process;
        match outcome {
            ChildOutcome::Ok(_) => process.status = ProcessStatus::Done,
            ChildOutcome::Err(failure) => process.status = ProcessStatus::Failed(failure),
        }
        table.return_process(process);
    }
}

/// Cancel all running children of a terminating process (scope cleanup).
/// When a parent exits, its children must not outlive it.
fn cancel_scope_children(table: &ProcessTable, scheduler: &dyn Scheduler, parent_pid: Pid) {
    let children: Vec<Pid> = table
        .child_parents
        .iter()
        .filter(|entry| *entry.value() == parent_pid)
        .map(|entry| *entry.key())
        .collect();

    for child_pid in children {
        // Skip if already tombstoned
        if table.tombstones.contains_key(&child_pid) {
            continue;
        }

        // Try to cancel in the table
        match table.processes.get_mut(&child_pid) {
            Some(mut child) => match &child.status {
                ProcessStatus::Blocked(reason) => {
                    let reason = reason.clone();
                    let child_parent = child.parent;
                    child.status = ProcessStatus::Failed(ProcessFailure::Cancelled);
                    drop(child);
                    scheduler.remove(child_pid);
                    clear_blocked_registration(table, child_pid, &reason);
                    if let Some(child_parent) = child_parent {
                        table.processes.remove(&child_pid);
                        table.tombstones.insert(
                            child_pid,
                            ChildRecord::Ready {
                                parent: child_parent,
                                outcome: ChildOutcome::Err(ProcessFailure::Cancelled),
                            },
                        );
                    }
                    wake_any_waiters(table, scheduler, child_pid);
                    wake_join_waiters(table, scheduler, child_pid);
                }
                ProcessStatus::Runnable => {
                    child.vm.request_cancellation();
                    drop(child);
                }
                _ => {
                    drop(child);
                }
            },
            None => {
                // Being executed by another worker — schedule pending cancel
                table.pending_cancels.insert(child_pid);
            }
        }
    }
}

fn worker_loop(
    table: &ProcessTable,
    scheduler: &dyn Scheduler,
    next_pid: &AtomicU64,
    next_io_token: &AtomicU64,
    active_workers: &std::sync::atomic::AtomicUsize,
    io_backend: &dyn IoBackend,
) {
    // Ownership model:
    // - runnable/blocked processes live in `table.processes`
    // - the currently executing process is temporarily removed from the table
    // - terminal children are replaced by tombstones so await/cancel can still
    //   observe parent/child relationships without keeping the full VM alive
    struct ActiveSlice<'a>(&'a std::sync::atomic::AtomicUsize);

    impl Drop for ActiveSlice<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }

    loop {
        let pid = match scheduler.dequeue() {
            Some(pid) => pid,
            None => return,
        };

        active_workers.fetch_add(1, Ordering::AcqRel);
        let _active_slice = ActiveSlice(active_workers);

        let reductions = scheduler.reductions(pid);
        let mut process = match table.take(pid) {
            Some(p) => p,
            None => continue,
        };

        // Cooperative cancellation is owned by the VM. A concurrent cancel
        // request can only mark intent here; the interpreter observes it at the
        // next slice boundary and returns `RunResult::Cancelled`.
        if table.pending_cancels.remove(&pid).is_some() {
            process.vm.request_cancellation();
        }

        let result = process.vm.run_slice(reductions);

        match result {
            RunResult::Done => {
                let outcome = if process.parent.is_some() {
                    let val = process.vm.stack.last().copied().unwrap_or(Value::Unit);
                    match crate::sendable::serialize(val, &process.vm.heap) {
                        Ok(sv) => ChildOutcome::Ok(sv),
                        Err(e) => ChildOutcome::Err(ProcessFailure::runtime(format!(
                            "child result not sendable: {e}"
                        ))),
                    }
                } else {
                    ChildOutcome::Ok(crate::sendable::SendableValue::Unit)
                };
                make_terminal(table, process, outcome);
                cancel_scope_children(table, scheduler, pid);
                scheduler.remove(pid);
                wake_any_waiters(table, scheduler, pid);
                wake_join_waiters(table, scheduler, pid);
            }
            RunResult::Yielded => {
                table.return_process(process);
                scheduler.enqueue(pid);
            }
            RunResult::Failed(failure) => {
                make_terminal(table, process, ChildOutcome::Err(failure));
                cancel_scope_children(table, scheduler, pid);
                scheduler.remove(pid);
                wake_any_waiters(table, scheduler, pid);
                wake_join_waiters(table, scheduler, pid);
            }
            RunResult::Spawn {
                proto_idx,
                captures,
            } => {
                let child_pid = Pid(next_pid.fetch_add(1, Ordering::Relaxed));
                match crate::runtime_ops::create_child_vm_from_parent(
                    &process.vm,
                    proto_idx,
                    captures,
                ) {
                    Ok(child_vm) => {
                        let child = Process::new(child_pid, child_vm, Some(pid));
                        table.child_parents.insert(child_pid, pid);
                        table.insert(child);
                        scheduler.enqueue(child_pid);

                        process.vm.stack.pop();
                        process.vm.push_value(Value::Pid(child_pid.0));
                        table.return_process(process);
                        scheduler.enqueue(pid);
                    }
                    Err(failure) => {
                        make_terminal(table, process, ChildOutcome::Err(failure));
                        scheduler.remove(pid);
                        wake_any_waiters(table, scheduler, pid);
                        wake_join_waiters(table, scheduler, pid);
                    }
                }
            }
            RunResult::Await(child_pid_val) => {
                let child_pid = Pid(child_pid_val);
                handle_await(table, scheduler, process, child_pid, AwaitKind::Raw);
            }
            RunResult::AwaitResult(child_pid_val) => {
                let child_pid = Pid(child_pid_val);
                handle_await(table, scheduler, process, child_pid, AwaitKind::Result);
            }
            RunResult::Cancel(child_pid_val) => {
                let child_pid = Pid(child_pid_val);
                handle_cancel(table, scheduler, process, child_pid);
            }
            RunResult::WaitAny(child_pid_vals) => {
                let child_pids = child_pid_vals.into_iter().map(Pid).collect();
                handle_wait_any(table, scheduler, process, child_pids);
            }
            RunResult::Io(request) => {
                let token = IoToken(next_io_token.fetch_add(1, Ordering::Relaxed));
                process.status = ProcessStatus::Blocked(BlockReason::Io(token));
                table.return_process(process);
                table.io_waiters.insert(token, pid);
                io_backend.register(token, request);
            }
            RunResult::Cancelled => {
                make_terminal(table, process, ChildOutcome::Err(ProcessFailure::Cancelled));
                cancel_scope_children(table, scheduler, pid);
                scheduler.remove(pid);
                wake_any_waiters(table, scheduler, pid);
                wake_join_waiters(table, scheduler, pid);
            }
        }
    }
}

fn handle_await(
    table: &ProcessTable,
    scheduler: &dyn Scheduler,
    mut parent: Process,
    child_pid: Pid,
    await_kind: AwaitKind,
) {
    let parent_pid = parent.pid;

    // Check tombstone first (child already finished and freed)
    let tombstone = table.tombstones.get(&child_pid).map(|r| r.value().clone());
    if let Some(record) = tombstone {
        match record {
            ChildRecord::Consumed {
                parent: tombstone_parent,
            } => {
                if tombstone_parent != parent_pid {
                    parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                        "await: process {:?} is not a child of {:?}",
                        child_pid, parent_pid
                    )));
                    table.return_process(parent);
                    return;
                }
                match await_kind {
                    AwaitKind::Raw => {
                        parent.status = ProcessStatus::Failed(ProcessFailure::runtime(
                            "await: child result already consumed",
                        ));
                    }
                    AwaitKind::Result => {
                        match deliver_join_result_to_parent(
                            &mut parent.vm,
                            Err(FiberJoinError::AlreadyJoined),
                        ) {
                            Ok(()) => {
                                parent.status = ProcessStatus::Runnable;
                                table.return_process(parent);
                                scheduler.enqueue(parent_pid);
                                return;
                            }
                            Err(failure) => {
                                parent.status = ProcessStatus::Failed(failure);
                            }
                        }
                    }
                }
                table.return_process(parent);
                return;
            }
            ChildRecord::Ready {
                parent: tombstone_parent,
                outcome,
            } => {
                if tombstone_parent != parent_pid {
                    parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                        "await: process {:?} is not a child of {:?}",
                        child_pid, parent_pid
                    )));
                    table.return_process(parent);
                    return;
                }
                let delivered = match (&await_kind, &outcome) {
                    (AwaitKind::Raw, ChildOutcome::Ok(sv)) => {
                        deliver_result_to_parent(&mut parent.vm, sv.clone())
                    }
                    (AwaitKind::Raw, ChildOutcome::Err(failure)) => {
                        parent.status = ProcessStatus::Failed(ProcessFailure::ChildProcessFailed(
                            Box::new(failure.clone()),
                        ));
                        table.tombstones.insert(
                            child_pid,
                            ChildRecord::Consumed {
                                parent: tombstone_parent,
                            },
                        );
                        table.child_parents.remove(&child_pid);
                        table.return_process(parent);
                        return;
                    }
                    (AwaitKind::Result, ChildOutcome::Ok(sv)) => {
                        deliver_join_result_to_parent(&mut parent.vm, Ok(sv.clone()))
                    }
                    (AwaitKind::Result, ChildOutcome::Err(failure)) => {
                        deliver_join_result_to_parent(
                            &mut parent.vm,
                            Err(FiberJoinError::from_process_failure(failure.clone())),
                        )
                    }
                };
                match delivered {
                    Ok(()) => {
                        parent.status = ProcessStatus::Runnable;
                        table.return_process(parent);
                        scheduler.enqueue(parent_pid);
                    }
                    Err(failure) => {
                        parent.status = ProcessStatus::Failed(failure);
                        table.return_process(parent);
                    }
                }
                table.tombstones.insert(
                    child_pid,
                    ChildRecord::Consumed {
                        parent: tombstone_parent,
                    },
                );
                table.child_parents.remove(&child_pid);
                return;
            }
        }
    }

    // Check live processes (child still running or temporarily taken for execution)
    let child_known = table.processes.get(&child_pid).map(|c| c.parent);

    match child_known {
        Some(Some(cp)) if cp != parent_pid => {
            parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                "await: process {:?} is not a child of {:?}",
                child_pid, parent_pid
            )));
            table.return_process(parent);
        }
        Some(_) => {
            // Child is in the table (running) — block and register waiter
            block_on_child(table, scheduler, parent, parent_pid, child_pid, await_kind);
        }
        None => {
            // Not in processes — check child_parents to see if it's being executed
            match table.child_parents.get(&child_pid).map(|r| *r.value()) {
                Some(cp) if cp != parent_pid => {
                    parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                        "await: process {:?} is not a child of {:?}",
                        child_pid, parent_pid
                    )));
                    table.return_process(parent);
                }
                Some(_) => {
                    // Child is executing on another worker — block and register waiter
                    block_on_child(table, scheduler, parent, parent_pid, child_pid, await_kind);
                }
                None => {
                    parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                        "await: unknown process {:?}",
                        child_pid
                    )));
                    table.return_process(parent);
                }
            }
        }
    }
}

/// Block a parent on a child and register a waiter, with tombstone recheck.
fn block_on_child(
    table: &ProcessTable,
    scheduler: &dyn Scheduler,
    mut parent: Process,
    parent_pid: Pid,
    child_pid: Pid,
    await_kind: AwaitKind,
) {
    // Transition:
    // runnable parent -> blocked parent registered under `waiters[child_pid]`
    //
    // Registration happens before the parent is re-published so a racing child
    // completion can still discover the waiter and deliver the tombstoned
    // result.
    parent.status = ProcessStatus::Blocked(BlockReason::Await {
        child: child_pid,
        kind: await_kind,
    });
    // Register waiter BEFORE returning parent to table, so that
    // if the child finishes concurrently, wake_join_waiters will
    // find this waiter entry.
    table.waiters.entry(child_pid).or_default().push(parent_pid);
    table.return_process(parent);

    // Recheck: if the child became tombstoned during the gap,
    // wake_join_waiters may have already run and missed us.
    if table.tombstones.contains_key(&child_pid) {
        wake_join_waiters(table, scheduler, child_pid);
    }
}

fn handle_cancel(
    table: &ProcessTable,
    scheduler: &dyn Scheduler,
    mut parent: Process,
    child_pid: Pid,
) {
    let parent_pid = parent.pid;

    // Check tombstone first — child already terminal
    let tombstone_parent = table.tombstones.get(&child_pid).map(|r| r.parent());
    if let Some(tombstone_parent) = tombstone_parent {
        if tombstone_parent != parent_pid {
            parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                "cancel: process {:?} is not a child of {:?}",
                child_pid, parent_pid
            )));
            table.return_process(parent);
            return;
        }
        // Already terminal — cancel is no-op
        parent.vm.stack.pop();
        parent.vm.push_value(Value::Unit);
        table.return_process(parent);
        scheduler.enqueue(parent_pid);
        return;
    }

    // Check live processes (or child_parents for executing children)
    match table.processes.get_mut(&child_pid) {
        None => {
            // Not in processes — check if executing on another worker
            match table.child_parents.get(&child_pid).map(|r| *r.value()) {
                Some(cp) if cp == parent_pid => {
                    // Child is executing — schedule a pending cancel
                    table.pending_cancels.insert(child_pid);
                    parent.vm.stack.pop();
                    parent.vm.push_value(Value::Unit);
                    table.return_process(parent);
                    scheduler.enqueue(parent_pid);
                }
                Some(_) => {
                    parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                        "cancel: process {:?} is not a child of {:?}",
                        child_pid, parent_pid
                    )));
                    table.return_process(parent);
                }
                None => {
                    parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                        "cancel: unknown process {:?}",
                        child_pid
                    )));
                    table.return_process(parent);
                }
            }
        }
        Some(child) if child.parent != Some(parent_pid) => {
            parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                "cancel: process {:?} is not a child of {:?}",
                child_pid, parent_pid
            )));
            table.return_process(parent);
        }
        Some(mut child) => {
            match &child.status {
                ProcessStatus::Done | ProcessStatus::Failed(_) => {
                    // Shouldn't happen with tombstones (terminal children are tombstoned),
                    // but handle defensively.
                    drop(child);
                }
                ProcessStatus::Blocked(reason) => {
                    let reason = reason.clone();
                    let child_parent = child.parent;
                    child.status = ProcessStatus::Failed(ProcessFailure::Cancelled);
                    drop(child);
                    scheduler.remove(child_pid);
                    clear_blocked_registration(table, child_pid, &reason);
                    // Tombstone the cancelled child
                    if let Some(child_parent) = child_parent {
                        table.processes.remove(&child_pid);
                        table.tombstones.insert(
                            child_pid,
                            ChildRecord::Ready {
                                parent: child_parent,
                                outcome: ChildOutcome::Err(ProcessFailure::Cancelled),
                            },
                        );
                    }
                    wake_any_waiters(table, scheduler, child_pid);
                    wake_join_waiters(table, scheduler, child_pid);
                }
                ProcessStatus::Runnable => {
                    child.vm.request_cancellation();
                    drop(child);
                }
            }

            parent.vm.stack.pop();
            parent.vm.push_value(Value::Unit);
            table.return_process(parent);
            scheduler.enqueue(parent_pid);
        }
    }
}

fn handle_wait_any(
    table: &ProcessTable,
    scheduler: &dyn Scheduler,
    mut parent: Process,
    child_pids: Vec<Pid>,
) {
    let parent_pid = parent.pid;
    let child_pids = dedup_pids(child_pids);

    if child_pids.is_empty() {
        parent.status = ProcessStatus::Failed(ProcessFailure::runtime(
            "wait_any: expected non-empty pid list",
        ));
        table.return_process(parent);
        return;
    }

    for &child_pid in &child_pids {
        // Check tombstone first — child already finished
        if let Some(record) = table.tombstones.get(&child_pid) {
            let tombstone_parent = record.parent();
            drop(record);
            if tombstone_parent != parent_pid {
                parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                    "wait_any: process {:?} is not a child of {:?}",
                    child_pid, parent_pid
                )));
                table.return_process(parent);
                return;
            }
            // Child already terminal — immediate winner
            crate::runtime_ops::deliver_pid_to_parent(&mut parent.vm, child_pid);
            table.return_process(parent);
            scheduler.enqueue(parent_pid);
            return;
        }

        // Check live process or executing child
        match table.processes.get(&child_pid) {
            Some(child) if child.parent != Some(parent_pid) => {
                parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                    "wait_any: process {:?} is not a child of {:?}",
                    child_pid, parent_pid
                )));
                table.return_process(parent);
                return;
            }
            Some(_) => {} // Running — will check later
            None => {
                // Not in processes — check child_parents for executing children
                match table.child_parents.get(&child_pid).map(|r| *r.value()) {
                    Some(cp) if cp == parent_pid => {} // Executing — valid child
                    Some(_) => {
                        parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                            "wait_any: process {:?} is not a child of {:?}",
                            child_pid, parent_pid
                        )));
                        table.return_process(parent);
                        return;
                    }
                    None => {
                        parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                            "wait_any: unknown process {:?}",
                            child_pid
                        )));
                        table.return_process(parent);
                        return;
                    }
                }
            }
        }
    }

    // Transition:
    // runnable parent -> blocked on an arbitrary child completion
    //
    // The parent is registered with every child pid so the first child that
    // reaches a tombstone can wake and resume it.
    parent.status = ProcessStatus::Blocked(BlockReason::WaitAny(child_pids.clone()));
    // Register all waiters BEFORE returning parent to table, so that
    // if a child finishes concurrently, wake_any_waiters will find
    // this waiter entry.
    for &child_pid in &child_pids {
        table
            .any_waiters
            .entry(child_pid)
            .or_default()
            .push(parent_pid);
    }
    table.return_process(parent);

    // Recheck: if any child became tombstoned during the gap between
    // our initial scan and the waiter registration, trigger the wake
    // path. The single-get_mut wake logic prevents double delivery.
    for &child_pid in &child_pids {
        if table.tombstones.contains_key(&child_pid) {
            wake_any_waiters(table, scheduler, child_pid);
            break;
        }
    }
}

fn wake_any_waiters(table: &ProcessTable, scheduler: &dyn Scheduler, finished_pid: Pid) {
    let waiter_pids = match table.any_waiters.remove(&finished_pid) {
        Some((_, waiters)) => waiters,
        None => return,
    };

    for waiter_pid in waiter_pids {
        // Use a single get_mut() to check status and deliver atomically,
        // avoiding a TOCTOU race where two children finishing concurrently
        // could both deliver to the same waiter. The finished pid is only a
        // wakeup signal; the semantic winner is the leftmost completed pid in
        // the parent's original wait_any input list.
        let child_pids = match table.processes.get_mut(&waiter_pid) {
            Some(mut waiter) => match &waiter.status {
                ProcessStatus::Blocked(BlockReason::WaitAny(child_pids)) => {
                    let child_pids = child_pids.clone();
                    let winner = child_pids.iter().copied().find(|child_pid| {
                        table
                            .tombstones
                            .get(child_pid)
                            .is_some_and(|record| record.parent() == waiter_pid)
                    });
                    let Some(winner) = winner else {
                        continue;
                    };
                    crate::runtime_ops::deliver_pid_to_parent(&mut waiter.vm, winner);
                    waiter.status = ProcessStatus::Runnable;
                    drop(waiter);
                    scheduler.enqueue(waiter_pid);
                    child_pids
                }
                _ => continue,
            },
            None => continue,
        };

        remove_any_waiter_registration(table, waiter_pid, &child_pids);
    }
}

fn wake_join_waiters(table: &ProcessTable, scheduler: &dyn Scheduler, finished_pid: Pid) {
    let waiter_pids = match table.waiters.remove(&finished_pid) {
        Some((_, w)) => w,
        None => return,
    };

    // Read delivery payload from the tombstone
    let delivery = match table.tombstones.get(&finished_pid) {
        Some(record) => match record.value() {
            ChildRecord::Ready { outcome, .. } => match outcome {
                ChildOutcome::Ok(sv) => Ok(sv.clone()),
                ChildOutcome::Err(failure) => Err(failure.clone()),
            },
            ChildRecord::Consumed { .. } => return,
        },
        None => return,
    };

    for waiter_pid in waiter_pids {
        // Single get_mut() to check status and deliver atomically,
        // avoiding a TOCTOU race if the waiter is concurrently cancelled.
        if let Some(mut waiter) = table.processes.get_mut(&waiter_pid) {
            let await_kind = match &waiter.status {
                ProcessStatus::Blocked(BlockReason::Await { child, kind })
                    if *child == finished_pid =>
                {
                    *kind
                }
                _ => continue,
            };

            match (await_kind, &delivery) {
                (AwaitKind::Raw, Ok(sendable)) => {
                    match deliver_result_to_parent(&mut waiter.vm, sendable.clone()) {
                        Ok(()) => {
                            waiter.status = ProcessStatus::Runnable;
                            drop(waiter);
                            scheduler.enqueue(waiter_pid);
                        }
                        Err(failure) => {
                            waiter.status = ProcessStatus::Failed(failure);
                        }
                    }
                }
                (AwaitKind::Raw, Err(msg)) => {
                    waiter.status = ProcessStatus::Failed(ProcessFailure::ChildProcessFailed(
                        Box::new(msg.clone()),
                    ));
                }
                (AwaitKind::Result, Ok(sendable)) => {
                    match deliver_join_result_to_parent(&mut waiter.vm, Ok(sendable.clone())) {
                        Ok(()) => {
                            waiter.status = ProcessStatus::Runnable;
                            drop(waiter);
                            scheduler.enqueue(waiter_pid);
                        }
                        Err(failure) => {
                            waiter.status = ProcessStatus::Failed(failure);
                        }
                    }
                }
                (AwaitKind::Result, Err(msg)) => {
                    match deliver_join_result_to_parent(
                        &mut waiter.vm,
                        Err(FiberJoinError::from_process_failure(msg.clone())),
                    ) {
                        Ok(()) => {
                            waiter.status = ProcessStatus::Runnable;
                            drop(waiter);
                            scheduler.enqueue(waiter_pid);
                        }
                        Err(failure) => {
                            waiter.status = ProcessStatus::Failed(failure);
                        }
                    }
                }
            }
        }
    }

    // Mark tombstone as consumed after all deliveries
    if let Some(record) = table.tombstones.get(&finished_pid) {
        let parent = record.parent();
        drop(record);
        table
            .tombstones
            .insert(finished_pid, ChildRecord::Consumed { parent });
        table.child_parents.remove(&finished_pid);
    }
}

fn clear_blocked_registration(table: &ProcessTable, pid: Pid, reason: &BlockReason) {
    match reason {
        BlockReason::Await { child, .. } => remove_waiter(&table.waiters, *child, pid),
        BlockReason::WaitAny(child_pids) => remove_any_waiter_registration(table, pid, child_pids),
        BlockReason::Io(token) => {
            table.io_waiters.remove(token);
        }
    }
}

fn remove_any_waiter_registration(table: &ProcessTable, waiter_pid: Pid, child_pids: &[Pid]) {
    for &child_pid in child_pids {
        remove_waiter(&table.any_waiters, child_pid, waiter_pid);
    }
}

fn remove_waiter(waiters: &DashMap<Pid, Vec<Pid>>, child_pid: Pid, waiter_pid: Pid) {
    let should_remove = if let Some(mut waiter_pids) = waiters.get_mut(&child_pid) {
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
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn compile(source: &str) -> CompiledProgram {
        let tokens = Lexer::new(source, 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile(program).unwrap();
        compiled
    }

    fn compile_file(path: &Path) -> CompiledProgram {
        let source = std::fs::read_to_string(path).expect("read source");
        let tokens = Lexer::new(&source, 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile_file(program, path).unwrap();
        compiled
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hiko-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn test_program_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/run")
            .join(name)
    }

    #[test]
    fn test_threaded_single_process() {
        let program = compile("val _ = println \"hello threaded\"");
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["hello threaded\n"]);
    }

    #[test]
    fn test_threaded_spawn_await() {
        let program = compile(
            "val child = spawn (fn () => 42)\n\
             val result = await_process child\n\
             val _ = println (int_to_string result)",
        );
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["42\n"]);
    }

    #[test]
    fn test_threaded_many_processes() {
        let program = compile(
            "fun make n = spawn (fn () => n * 2)\n\
             val c1 = make 1\n\
             val c2 = make 2\n\
             val c3 = make 3\n\
             val c4 = make 4\n\
             val c5 = make 5\n\
             val r1 = await_process c1\n\
             val r2 = await_process c2\n\
             val r3 = await_process c3\n\
             val r4 = await_process c4\n\
             val r5 = await_process c5\n\
             val _ = println (int_to_string (r1 + r2 + r3 + r4 + r5))",
        );
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["30\n"]);
    }

    #[test]
    fn test_async_sleep() {
        // sleep builtin suspends the process via MockIoBackend (deterministic)
        let program = compile(
            "val _ = sleep 999999\n\
             val _ = println \"after sleep\"",
        );
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["after sleep\n"]);
    }

    #[test]
    fn test_mock_backend_concurrent_sleep() {
        // Two child processes sleeping via MockIoBackend (instant, deterministic)
        let program = compile(
            "val a = spawn (fn () => sleep 999999)\n\
             val b = spawn (fn () => sleep 999999)\n\
             val _ = await_process a\n\
             val _ = await_process b\n\
             val _ = println \"both done\"",
        );
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["both done\n"]);
    }

    #[test]
    fn test_wait_any_returns_only_completed_child_pid() {
        let program = compile(
            "val slow = spawn (fn () => let val _ = sleep 999999 in 10 end)\n\
             val fast = spawn (fn () => 20)\n\
             val winner = wait_any [slow, fast]\n\
             val result = await_process winner\n\
             val _ = println (int_to_string result)",
        );
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["20\n"]);
    }

    #[test]
    fn test_cancel_marks_blocked_child_cancelled() {
        let program = compile(
            "val child = spawn (fn () => let val _ = sleep 999999 in 42 end)\n\
             val _ = cancel child\n\
             val _ = println \"cancelled\"",
        );
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();

        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["cancelled\n"]);
        // With tombstones, the cancelled child is in the tombstones map, not processes.
        assert!(runtime.table.tombstones.iter().any(|entry| {
            matches!(
                entry.value(),
                ChildRecord::Ready {
                    outcome: ChildOutcome::Err(ProcessFailure::Cancelled),
                    ..
                }
            )
        }));
    }

    #[test]
    fn test_stdlib_fiber_module() {
        let program = compile_file(&test_program_path("test_fiber.hml"));
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        let status = runtime
            .table
            .processes
            .get(&pid)
            .map(|process| format!("{:?}", process.status))
            .unwrap_or_else(|| "<missing>".into());
        assert_eq!(status, "Done");
        assert_eq!(output, vec!["fiber tests passed\n"]);
    }

    #[test]
    fn test_stdlib_fiber_error_paths() {
        let program = compile_file(&test_program_path("test_fiber_errors.hml"));
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        let status = runtime
            .table
            .processes
            .get(&pid)
            .map(|process| format!("{:?}", process.status))
            .unwrap_or_else(|| "<missing>".into());
        assert_eq!(status, "Done");
        assert_eq!(output, vec!["fiber error tests passed\n"]);
    }

    #[test]
    fn test_stdlib_fiber_reports_child_fuel_exhaustion() {
        let program = compile_file(&test_program_path("test_fiber_fuel.hml"));
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.new_pid();
        let mut vm = VM::new(program);
        vm.enable_output_capture();
        vm.set_fuel(2_500);
        let process = Process::new(pid, vm, None);
        runtime.table.insert(process);
        runtime.scheduler.enqueue(pid);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        let status = runtime
            .table
            .processes
            .get(&pid)
            .map(|process| format!("{:?}", process.status))
            .unwrap_or_else(|| "<missing>".into());
        assert_eq!(status, "Done");
        assert_eq!(output, vec!["fiber fuel tests passed\n"]);
    }

    #[test]
    fn test_stdlib_fiber_reports_child_heap_limit() {
        let program = compile_file(&test_program_path("test_fiber_heap.hml"));
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.new_pid();
        let mut vm = VM::new(program);
        vm.enable_output_capture();
        vm.set_max_heap(64);
        let process = Process::new(pid, vm, None);
        runtime.table.insert(process);
        runtime.scheduler.enqueue(pid);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        let status = runtime
            .table
            .processes
            .get(&pid)
            .map(|process| format!("{:?}", process.status))
            .unwrap_or_else(|| "<missing>".into());
        assert_eq!(status, "Done");
        assert_eq!(output, vec!["fiber heap tests passed\n"]);
    }

    #[test]
    fn test_fiber_first_does_not_leak_children() {
        // Repeated Fiber.first calls must not accumulate unreaped children.
        // Each iteration spawns 2 children; losers must be reaped via cancel+join.
        let program = compile_file(&test_program_path("test_fiber_no_leak.hml"));
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let status = runtime
            .table
            .processes
            .get(&pid)
            .map(|process| format!("{:?}", process.status))
            .unwrap_or_else(|| "<missing>".into());
        assert_eq!(status, "Done");
        // After completion, only the root process should remain in the table.
        let remaining = runtime.table.processes.len();
        assert_eq!(
            remaining, 1,
            "expected only root process in table, found {remaining} processes"
        );
    }

    #[test]
    fn test_async_read_file() {
        // read_file should work asynchronously in threaded runtime
        let path = temp_path("async-read-file");
        let path_str = path.to_string_lossy();
        let program = compile(&format!(
            "val _ = write_file (\"{path_str}\", \"hello async\")\n\
             val contents = read_file \"{path_str}\"\n\
             val _ = println contents\n\
             val _ = remove_file \"{path_str}\"",
        ));
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["hello async\n"]);
    }

    #[test]
    fn test_async_read_file_respects_io_limit() {
        let root = temp_path("async-read-file-io-limit");
        let data = root.join("data.txt");
        let root_str = root.to_string_lossy();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&data, "hello").unwrap();

        let program = compile("val _ = read_file \"data.txt\"");
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.new_pid();
        let mut vm = VM::new(program);
        vm.enable_output_capture();
        vm.set_async_io(true);
        vm.set_fs_root(root_str.to_string());
        vm.set_max_io_bytes(4);
        let process = Process::new(pid, vm, None);
        runtime.table.insert(process);
        runtime.scheduler.enqueue(pid);

        runtime.run_to_completion().unwrap();

        let process = runtime
            .table
            .processes
            .get(&pid)
            .expect("root process missing");
        match &process.status {
            ProcessStatus::Failed(ProcessFailure::RuntimeError(message)) => {
                assert!(message.starts_with("io limit exceeded:"));
            }
            other => panic!("expected io limit failure, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_double_await_fails() {
        // Second await on the same child should fail (result consumed)
        let program = compile(
            "val child = spawn (fn () => 42)\n\
             val r1 = await_process child\n\
             val r2 = await_process child\n\
             val _ = println (int_to_string (r1 + r2))",
        );
        let runtime = ThreadedRuntime::new(1);
        let _pid = runtime.spawn_root(program);
        let result = runtime.run_to_completion();
        // The process should fail — either the second await errors or the parent fails
        assert!(
            result.is_err()
                || runtime
                    .table
                    .processes
                    .iter()
                    .any(|p| matches!(p.status, ProcessStatus::Failed(_)))
        );
    }

    #[test]
    fn test_async_read_file_with_fs_root() {
        // Async read_file should use the validated path, not the raw input
        let root = temp_path("async-root-test");
        let data = root.join("data.txt");
        let data_str = data.to_string_lossy();
        let root_str = root.to_string_lossy();
        let program = compile(&format!(
            "val _ = write_file (\"{data_str}\", \"rooted\")\n\
             val contents = read_file \"data.txt\"\n\
             val _ = println contents\n\
             val _ = remove_file \"{data_str}\"",
        ));
        // Set up the directory
        let _ = std::fs::create_dir_all(&root);
        let _ = std::fs::write(&data, "rooted");

        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.new_pid();
        let mut vm = VM::new(program);
        vm.enable_output_capture();
        vm.set_async_io(true);
        vm.set_fs_root(root_str.to_string());
        let process = Process::new(pid, vm, None);
        runtime.table.insert(process);
        runtime.scheduler.enqueue(pid);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["rooted\n"]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_concurrent_async_http() {
        // Multiple spawned processes doing http_get via MockIoBackend
        let program = compile(
            "val a = spawn (fn () => http_get \"http://a.test\")\n\
             val b = spawn (fn () => http_get \"http://b.test\")\n\
             val c = spawn (fn () => http_get \"http://c.test\")\n\
             val (_, _, body_a) = await_process a\n\
             val (_, _, body_b) = await_process b\n\
             val (_, _, body_c) = await_process c\n\
             val _ = println body_a\n\
             val _ = println body_b\n\
             val _ = println body_c",
        );
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output.len(), 3);
        assert!(output[0].contains("mock response from http://a.test"));
        assert!(output[1].contains("mock response from http://b.test"));
        assert!(output[2].contains("mock response from http://c.test"));
    }

    #[test]
    fn test_mock_backend_sleep() {
        // MockIoBackend completes sleep immediately
        let program = compile(
            "val _ = sleep 999999\n\
             val _ = println \"instant\"",
        );
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["instant\n"]);
    }

    #[test]
    fn test_threaded_root_process_may_finish_with_non_sendable_value() {
        let program = compile("val f = fn () => 1");
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let status = runtime
            .table
            .processes
            .get(&pid)
            .map(|p| format!("{:?}", p.status))
            .expect("root process should exist");
        assert_eq!(status, "Done");
    }

    #[test]
    fn test_threaded_reaps_finished_child_after_await() {
        let program = compile(
            "val child = spawn (fn () => 42)\n\
             val result = await_process child\n\
             val _ = println (int_to_string result)",
        );
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();

        assert_eq!(runtime.table.processes.len(), 1);
        assert!(runtime.table.processes.contains_key(&pid));
    }

    #[test]
    fn test_threaded_reaps_failed_child_after_await() {
        let program = compile(
            "val child = spawn (fn () => panic \"boom\")\n\
             val _ = await_process child",
        );
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();

        assert_eq!(runtime.table.processes.len(), 1);
        let status = runtime
            .table
            .processes
            .get(&pid)
            .map(|p| format!("{:?}", p.status))
            .expect("root process should exist");
        assert!(status.contains("Failed"));
    }

    // --- Multi-worker tests: exercise concurrent scheduling and race-fix paths ---

    #[test]
    fn test_multiworker_spawn_await() {
        let program = compile(
            "val child = spawn (fn () => 42)\n\
             val result = await_process child\n\
             val _ = println (int_to_string result)",
        );
        let runtime = ThreadedRuntime::new(4);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["42\n"]);
    }

    #[test]
    fn test_multiworker_many_children() {
        let program = compile(
            "fun make n = spawn (fn () => n * 2)\n\
             val c1 = make 1\n\
             val c2 = make 2\n\
             val c3 = make 3\n\
             val c4 = make 4\n\
             val c5 = make 5\n\
             val r1 = await_process c1\n\
             val r2 = await_process c2\n\
             val r3 = await_process c3\n\
             val r4 = await_process c4\n\
             val r5 = await_process c5\n\
             val _ = println (int_to_string (r1 + r2 + r3 + r4 + r5))",
        );
        let runtime = ThreadedRuntime::new(4);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["30\n"]);
    }

    #[test]
    fn test_multiworker_wait_any_returns_leftmost_completed_child() {
        let program = compile(
            "val left = spawn (fn () => 10)\n\
             val right = spawn (fn () => 20)\n\
             val winner = wait_any [left, right]\n\
             val result = await_process winner\n\
             val _ = println (int_to_string result)",
        );
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["10\n"]);
    }

    #[test]
    fn test_multiworker_wait_any() {
        let program = compile(
            "val fast = spawn (fn () => 20)\n\
             val slow = spawn (fn () => let val _ = sleep 999999 in 10 end)\n\
             val winner = wait_any [fast, slow]\n\
             val result = await_process winner\n\
             val _ = println (int_to_string result)",
        );
        let runtime = ThreadedRuntime::new(4)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["20\n"]);
    }

    #[test]
    fn test_multiworker_cancel_blocked_child() {
        let program = compile(
            "val child = spawn (fn () => let val _ = sleep 999999 in 42 end)\n\
             val _ = cancel child\n\
             val _ = println \"cancelled\"",
        );
        let runtime = ThreadedRuntime::new(4)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["cancelled\n"]);
    }

    #[test]
    fn test_multiworker_concurrent_sleep() {
        // Multiple children sleeping concurrently with multiple workers
        let program = compile(
            "val a = spawn (fn () => sleep 999999)\n\
             val b = spawn (fn () => sleep 999999)\n\
             val c = spawn (fn () => sleep 999999)\n\
             val _ = await_process a\n\
             val _ = await_process b\n\
             val _ = await_process c\n\
             val _ = println \"all done\"",
        );
        let runtime = ThreadedRuntime::new(4)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["all done\n"]);
    }

    #[test]
    fn test_multiworker_fiber_first_no_leak() {
        let program = compile_file(&test_program_path("test_fiber_no_leak.hml"));
        let runtime = ThreadedRuntime::new(4)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let status = runtime
            .table
            .processes
            .get(&pid)
            .map(|process| format!("{:?}", process.status))
            .unwrap_or_else(|| "<missing>".into());
        assert_eq!(status, "Done");
        let remaining = runtime.table.processes.len();
        assert_eq!(
            remaining, 1,
            "expected only root process in table, found {remaining} processes"
        );
    }

    #[test]
    fn test_multiworker_fiber_error_paths() {
        let program = compile_file(&test_program_path("test_fiber_errors.hml"));
        let runtime = ThreadedRuntime::new(4)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["fiber error tests passed\n"]);
    }

    #[test]
    fn test_multiworker_stress_spawn_await_loop() {
        let program = compile(
            "fun loop n =\n\
               if n = 0 then ()\n\
               else\n\
                 let\n\
                   val child = spawn (fn () => n)\n\
                   val _ = await_process child\n\
                 in\n\
                   loop (n - 1)\n\
                 end\n\
             val _ = loop 100\n\
             val _ = println \"done\"",
        );
        let runtime = ThreadedRuntime::new(4);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["done\n"]);
    }

    // --- Scope cleanup tests ---

    #[test]
    fn test_scope_cancels_unawaited_child() {
        // Parent finishes without awaiting child → child should be cancelled by scope cleanup
        let program = compile(
            "val _ = spawn (fn () => let val _ = sleep 999999 in 42 end)\n\
             val _ = println \"parent done\"",
        );
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["parent done\n"]);
        // Only root should remain in processes (child was scope-cancelled and tombstoned)
        assert_eq!(runtime.table.processes.len(), 1);
    }

    #[test]
    fn test_scope_cancels_multiple_unawaited_children() {
        let program = compile(
            "val _ = spawn (fn () => let val _ = sleep 999999 in 1 end)\n\
             val _ = spawn (fn () => let val _ = sleep 999999 in 2 end)\n\
             val _ = spawn (fn () => let val _ = sleep 999999 in 3 end)\n\
             val _ = println \"done\"",
        );
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["done\n"]);
        assert_eq!(runtime.table.processes.len(), 1);
    }

    #[test]
    fn test_scope_cascades_cancellation() {
        // Parent spawns child, child spawns grandchild; parent exits → both cancelled
        let program = compile(
            "val _ = spawn (fn () =>\n\
               let val _ = spawn (fn () => sleep 999999)\n\
               in sleep 999999 end)\n\
             val _ = println \"root done\"",
        );
        let runtime = ThreadedRuntime::new(1)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["root done\n"]);
        assert_eq!(runtime.table.processes.len(), 1);
    }

    #[test]
    fn test_multiworker_scope_cleanup() {
        // Same scope cleanup test but with 4 workers
        let program = compile(
            "val _ = spawn (fn () => let val _ = sleep 999999 in 1 end)\n\
             val _ = spawn (fn () => let val _ = sleep 999999 in 2 end)\n\
             val _ = println \"done\"",
        );
        let runtime = ThreadedRuntime::new(4)
            .with_io_backend(Arc::new(crate::io_backend::MockIoBackend::new()));
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["done\n"]);
        assert_eq!(runtime.table.processes.len(), 1);
    }
}
