//! Multi-threaded runtime: N worker threads executing hiko processes in parallel.
//!
//! # Ownership model
//!
//! A process is owned by exactly one party at a time:
//!
//! - a worker, while it runs a slice (the process is out of the table);
//! - the table, while it is runnable and queued, or blocked;
//! - whoever removed it from the table with [`ProcessTable::take_runnable`] or
//!   [`ProcessTable::take_blocked`] to change its state.
//!
//! State changes happen only on an owned process. A blocked process is put back
//! with [`park`], a runnable one with [`resume`], and a terminating one is
//! handed to [`terminate_process`], the only place that makes a process
//! terminal. `terminate_process` records the outcome (a tombstone for a child,
//! a terminal status for a root), cancels the process's children, reaps their
//! unconsumed tombstones, and wakes every process waiting on it.
//!
//! # Wake protocol
//!
//! A process that blocks on a child (await, wait_any) or on I/O first
//! registers itself in `waiters`, `any_waiters` or `io_waiters`, then parks
//! itself, then rechecks whether the awaited child already terminated. A
//! terminating child marks itself terminal first and only then reads the
//! registrations. Whichever side comes second sees the other, so a wakeup
//! cannot be lost. A waker that finds the waiter not yet parked leaves the
//! registration in place; the waiter's own recheck then delivers the result.
//! Registrations are removed by the party that delivers, or when the waiter
//! terminates.
//!
//! Cancellation of a process that is out of the table is recorded in
//! `pending_cancels`. The owner observes it when it next parks the process
//! (see [`park`]) or when a worker next dequeues it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use dashmap::{DashMap, DashSet};

use crate::io_backend::{IoBackend, IoContext, IoResult, IoToken, ThreadPoolIoBackend};
use crate::process::{
    AwaitKind, BlockReason, ChildOutcome, ChildRecord, Pid, Process, ProcessFailure, ProcessStatus,
    Scope, ScopeId,
};
use crate::runtime_ops::{ChildState, dedup_pids, deliver_await_outcome, deliver_result_to_parent};
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::value::Value;
use crate::vm::{RunResult, VM};
use hiko_compile::chunk::CompiledProgram;

/// Thread-safe process table using DashMap for fine-grained locking.
struct ProcessTable {
    /// Runnable and blocked processes, plus terminated root processes.
    /// A process that a worker is executing is temporarily absent.
    processes: DashMap<Pid, Process>,
    /// Every process that has not terminated, including executing ones.
    live: DashSet<Pid>,
    /// Outcomes of terminated children that their parent has not consumed.
    tombstones: DashMap<Pid, ChildRecord>,
    /// Live processes with a cancellation request that their owner has not
    /// observed yet, because the process was out of the table.
    pending_cancels: DashSet<Pid>,
    /// child pid -> processes blocked in `await` on it.
    waiters: DashMap<Pid, Vec<Pid>>,
    /// child pid -> processes blocked in `wait_any` on a list containing it.
    any_waiters: DashMap<Pid, Vec<Pid>>,
    /// I/O token -> process blocked on it.
    io_waiters: DashMap<IoToken, Pid>,
    #[allow(dead_code)] // scaffolding for structured concurrency
    scopes: DashMap<ScopeId, Scope>,
    next_pid: AtomicU64,
    /// Largest tombstone count ever observed after a tombstone insert.
    #[cfg(test)]
    max_tombstones: AtomicUsize,
}

impl ProcessTable {
    fn new() -> Self {
        Self {
            processes: DashMap::new(),
            live: DashSet::new(),
            tombstones: DashMap::new(),
            pending_cancels: DashSet::new(),
            waiters: DashMap::new(),
            any_waiters: DashMap::new(),
            io_waiters: DashMap::new(),
            scopes: DashMap::new(),
            next_pid: AtomicU64::new(1),
            #[cfg(test)]
            max_tombstones: AtomicUsize::new(0),
        }
    }

    fn alloc_pid(&self) -> Pid {
        Pid(self.next_pid.fetch_add(1, Ordering::SeqCst))
    }

    /// Publish a new, runnable process. The caller enqueues it.
    fn insert(&self, process: Process) {
        self.live.insert(process.pid);
        self.processes.insert(process.pid, process);
    }

    /// Take a runnable process out of the table to run it. Stale queue
    /// entries (the process is blocked, terminal, or gone) yield `None`.
    fn take_runnable(&self, pid: Pid) -> Option<Process> {
        self.processes
            .remove_if(&pid, |_, process| process.is_runnable())
            .map(|(_, process)| process)
    }

    /// Take a blocked process out of the table if its block reason matches.
    fn take_blocked(&self, pid: Pid, pred: impl Fn(&BlockReason) -> bool) -> Option<Process> {
        self.processes
            .remove_if(&pid, |_, process| match &process.status {
                ProcessStatus::Blocked(reason) => pred(reason),
                _ => false,
            })
            .map(|(_, process)| process)
    }

    /// Take a process that is registered as a waiter, for delivery.
    fn take_waiter(
        &self,
        pid: Pid,
        pred: impl Fn(&BlockReason) -> bool,
    ) -> Result<Process, NotTaken> {
        loop {
            if let Some(process) = self.take_blocked(pid, &pred) {
                return Ok(process);
            }
            match self.processes.get(&pid) {
                // It was parked after our attempt; try again.
                Some(process) if matches!(&process.status, ProcessStatus::Blocked(reason) if pred(reason)) =>
                {
                    continue;
                }
                // In the table but not waiting for this: the registration is stale.
                Some(_) => return Err(NotTaken::Stale),
                // Out of the table: either about to park (its recheck will
                // deliver), or owned by another party that will handle it.
                None if self.live.contains(&pid) => return Err(NotTaken::InFlight),
                None => return Err(NotTaken::Stale),
            }
        }
    }

    /// Classify `pid` from the point of view of `parent`, which the caller owns.
    fn classify_child(&self, parent: &Process, pid: Pid) -> ChildState {
        if parent.children.contains(&pid) {
            // A terminating child gets its tombstone before it stops being
            // live, so "not live" implies "tombstone present".
            if self.live.contains(&pid) {
                ChildState::Live
            } else {
                ChildState::Terminated
            }
        } else if self.live.contains(&pid)
            || self.tombstones.contains_key(&pid)
            || self.processes.contains_key(&pid)
        {
            ChildState::NotChild
        } else if pid.0 > 0 && pid.0 < self.next_pid.load(Ordering::SeqCst) {
            ChildState::Reaped
        } else {
            ChildState::Unknown
        }
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

    /// Processes blocked on another process with no runnable process and no
    /// pending I/O left to wake them.
    fn permanently_blocked(&self) -> Vec<Pid> {
        if !self.io_waiters.is_empty() || self.processes.iter().any(|e| e.is_runnable()) {
            return Vec::new();
        }
        self.processes
            .iter()
            .filter(|e| {
                matches!(
                    e.status,
                    ProcessStatus::Blocked(BlockReason::Await { .. })
                        | ProcessStatus::Blocked(BlockReason::WaitAny(_))
                )
            })
            .map(|e| *e.key())
            .collect()
    }

    /// Check the bookkeeping invariants. Only meaningful while no worker is
    /// executing a slice, for example after `run_to_completion`.
    #[cfg(test)]
    fn check_invariants(&self) -> Result<(), String> {
        let known = |pid: &Pid| self.live.contains(pid) || self.tombstones.contains_key(pid);

        for entry in self.waiters.iter() {
            let child_pid = *entry.key();
            if !known(&child_pid) {
                return Err(format!("waiters contains unknown child {child_pid:?}"));
            }
            for waiter_pid in entry.value() {
                let Some(waiter) = self.processes.get(waiter_pid) else {
                    return Err(format!(
                        "waiter {waiter_pid:?} registered for {child_pid:?} is missing"
                    ));
                };
                match &waiter.status {
                    ProcessStatus::Blocked(BlockReason::Await { child, .. })
                        if *child == child_pid => {}
                    status => {
                        return Err(format!(
                            "waiter {waiter_pid:?} registered for {child_pid:?} but has status {status:?}"
                        ));
                    }
                }
            }
        }

        for entry in self.any_waiters.iter() {
            let child_pid = *entry.key();
            if !known(&child_pid) {
                return Err(format!("any_waiters contains unknown child {child_pid:?}"));
            }
            for waiter_pid in entry.value() {
                let Some(waiter) = self.processes.get(waiter_pid) else {
                    return Err(format!(
                        "wait-any waiter {waiter_pid:?} registered for {child_pid:?} is missing"
                    ));
                };
                match &waiter.status {
                    ProcessStatus::Blocked(BlockReason::WaitAny(child_pids))
                        if child_pids.contains(&child_pid) => {}
                    status => {
                        return Err(format!(
                            "wait-any waiter {waiter_pid:?} registered for {child_pid:?} but has status {status:?}"
                        ));
                    }
                }
            }
        }

        for entry in self.io_waiters.iter() {
            let token = *entry.key();
            let pid = *entry.value();
            let Some(process) = self.processes.get(&pid) else {
                return Err(format!("I/O waiter {pid:?} for {token:?} is missing"));
            };
            match &process.status {
                ProcessStatus::Blocked(BlockReason::Io(blocked_token))
                    if *blocked_token == token => {}
                status => {
                    return Err(format!(
                        "I/O waiter {pid:?} registered for {token:?} but has status {status:?}"
                    ));
                }
            }
        }

        for entry in self.tombstones.iter() {
            if !self.live.contains(&entry.value().parent) {
                return Err(format!(
                    "tombstone {:?} outlived its parent {:?}",
                    entry.key(),
                    entry.value().parent
                ));
            }
        }

        for pid in self.pending_cancels.iter() {
            if !self.live.contains(&*pid) {
                return Err(format!("pending cancel for terminated process {:?}", *pid));
            }
        }

        for pid in self.live.iter() {
            match self.processes.get(&*pid) {
                Some(process) if !process.is_done() => {}
                Some(_) => return Err(format!("live process {:?} is terminal", *pid)),
                None => return Err(format!("live process {:?} is missing", *pid)),
            }
        }

        for entry in self.processes.iter() {
            if entry.is_done() && entry.parent.is_some() {
                return Err(format!("terminal child {:?} kept in table", entry.key()));
            }
        }

        Ok(())
    }
}

/// Why `take_waiter` did not return the waiter.
enum NotTaken {
    /// Out of the table but live: about to park, or owned by another party.
    InFlight,
    /// Not waiting for this event: the registration is stale.
    Stale,
}

/// Borrowed runtime services passed to every lifecycle transition.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    table: &'a ProcessTable,
    scheduler: &'a dyn Scheduler,
    io: &'a dyn IoBackend,
}

/// Processes waiting to be terminated. Transitions push here instead of
/// recursing, so cascades (a failure propagating up an await chain, a
/// cancellation propagating down a process tree) run in constant stack.
type Terminations = Vec<(Process, ChildOutcome)>;

/// Multi-threaded hiko runtime.
pub struct ThreadedRuntime {
    active_workers: Arc<AtomicUsize>,
    /// Number of slices ever started. Lets the monitor detect a slice that
    /// started while it was scanning the process table.
    slices_started: Arc<AtomicU64>,
    next_io_token: Arc<AtomicU64>,
    table: Arc<ProcessTable>,
    scheduler: Arc<dyn Scheduler>,
    io_backend: Arc<dyn IoBackend>,
    num_workers: usize,
}

impl ThreadedRuntime {
    pub fn new(num_workers: usize) -> Self {
        let num_workers = num_workers.max(1);
        Self {
            active_workers: Arc::new(AtomicUsize::new(0)),
            slices_started: Arc::new(AtomicU64::new(0)),
            next_io_token: Arc::new(AtomicU64::new(1)),
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
        self.table.alloc_pid()
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            table: &self.table,
            scheduler: &*self.scheduler,
            io: &*self.io_backend,
        }
    }

    pub fn spawn_root(&self, program: CompiledProgram) -> Pid {
        self.spawn_root_vm(VM::new(program))
    }

    pub fn spawn_root_vm(&self, mut vm: VM) -> Pid {
        vm.enable_output_capture();
        vm.set_async_io(true);
        self.insert_root(vm)
    }

    /// Publish an already-configured root VM.
    fn insert_root(&self, vm: VM) -> Pid {
        let pid = self.new_pid();
        self.table.insert(Process::new(pid, vm, None));
        self.scheduler.enqueue(pid);
        pid
    }

    pub fn failure(&self, pid: Pid) -> Option<ProcessFailure> {
        self.table
            .processes
            .get(&pid)
            .and_then(|process| match &process.status {
                ProcessStatus::Failed(failure) => Some(failure.clone()),
                _ => None,
            })
    }

    #[cfg(test)]
    pub fn check_invariants(&self) -> Result<(), String> {
        self.table.check_invariants()
    }

    /// Run all processes to completion using N worker threads.
    pub fn run_to_completion(&self) -> Result<Vec<String>, String> {
        let handles: Vec<_> = (0..self.num_workers)
            .map(|_| {
                let table = Arc::clone(&self.table);
                let scheduler = Arc::clone(&self.scheduler);
                let next_io_token = Arc::clone(&self.next_io_token);
                let active_workers = Arc::clone(&self.active_workers);
                let slices_started = Arc::clone(&self.slices_started);
                let io_backend = Arc::clone(&self.io_backend);

                std::thread::spawn(move || {
                    let cx = Ctx {
                        table: &table,
                        scheduler: &*scheduler,
                        io: &*io_backend,
                    };
                    worker_loop(cx, &next_io_token, &active_workers, &slices_started);
                })
            })
            .collect();

        // Monitor: poll I/O completions and check for termination
        let cx = self.ctx();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1));

            // Poll I/O backend for completed operations
            let completions = self.io_backend.poll();
            for (token, result) in completions {
                handle_io_completion(cx, token, result);
            }

            // A running process is out of the table, so the table scan alone
            // cannot tell "all done" from "a slice is in flight". Only trust
            // the scan if no slice was active when it began and none started
            // while it ran. Workers bump `active_workers` before
            // `slices_started`, so seeing a slice's start here implies seeing
            // it as active.
            let slices_before = self.slices_started.load(Ordering::SeqCst);
            let idle_before = self.active_workers.load(Ordering::SeqCst) == 0;
            if idle_before
                && self.table.is_all_done_or_blocked()
                && self.slices_started.load(Ordering::SeqCst) == slices_before
                && self.table.io_waiters.is_empty()
            {
                let stuck = self.table.permanently_blocked();
                if stuck.is_empty() {
                    self.scheduler.shutdown();
                    break;
                }
                // Safety net: with correct bookkeeping no process can block
                // forever, because only a parent may await its child. Fail the
                // stuck processes through the normal terminal path so their
                // own waiters observe the failure, then keep running.
                fail_deadlocked(cx, &stuck);
            }
        }

        for h in handles {
            h.join().unwrap();
        }

        Ok(self.table.all_outputs())
    }
}

#[cfg(test)]
impl ThreadedRuntime {
    /// Observable result of `pid` plus any bookkeeping left behind.
    pub(crate) fn lifecycle_report(&self, pid: Pid) -> crate::lifecycle_tests::Report {
        let table = &self.table;
        let mut leftovers = Vec::new();
        let mut note = |name: &str, len: usize| {
            if len > 0 {
                leftovers.push(format!("{name}={len}"));
            }
        };
        note("extra_processes", table.processes.len().saturating_sub(1));
        note("live", table.live.len());
        note("tombstones", table.tombstones.len());
        note("pending_cancels", table.pending_cancels.len());
        note("waiters", table.waiters.len());
        note("any_waiters", table.any_waiters.len());
        note("io_waiters", table.io_waiters.len());
        if let Err(message) = table.check_invariants() {
            leftovers.push(message);
        }
        crate::lifecycle_tests::Report {
            output: table.get_output(pid),
            status: table
                .processes
                .get(&pid)
                .map(|process| format!("{:?}", process.status))
                .unwrap_or_else(|| "<missing>".into()),
            leftovers,
        }
    }

    /// The largest number of tombstones held at any point so far.
    pub(crate) fn max_tombstones(&self) -> usize {
        self.table.max_tombstones.load(Ordering::SeqCst)
    }
}

fn fail_deadlocked(cx: Ctx<'_>, stuck: &[Pid]) {
    let mut terminations = Terminations::new();
    for &pid in stuck {
        let Some(process) = cx.table.take_blocked(pid, |reason| {
            matches!(reason, BlockReason::Await { .. } | BlockReason::WaitAny(_))
        }) else {
            continue;
        };
        clear_blocked_registration(cx, &process);
        terminations.push((
            process,
            ChildOutcome::Err(ProcessFailure::runtime(
                "deadlock: process blocked with no possible waker",
            )),
        ));
    }
    drain_terminations(cx, &mut terminations);
}

// --- Ownership transitions ---------------------------------------------------

/// Return an owned, runnable process to the table and queue it.
fn resume(cx: Ctx<'_>, mut process: Process) {
    let pid = process.pid;
    process.status = ProcessStatus::Runnable;
    cx.table.processes.insert(pid, process);
    cx.scheduler.enqueue(pid);
}

/// Return an owned, blocked process to the table. Its block registration must
/// already be in place. Observes a cancellation requested while it was owned.
fn park(cx: Ctx<'_>, process: Process, terminations: &mut Terminations) {
    debug_assert!(matches!(process.status, ProcessStatus::Blocked(_)));
    let pid = process.pid;
    cx.table.processes.insert(pid, process);
    if cx.table.pending_cancels.remove(&pid).is_some() {
        try_cancel_in_table(cx, pid, terminations);
    }
}

/// Terminate an owned process. The single entry point for terminal
/// transitions; see [`finish_process`].
fn terminate_process(cx: Ctx<'_>, process: Process, outcome: ChildOutcome) {
    let mut terminations = vec![(process, outcome)];
    drain_terminations(cx, &mut terminations);
}

fn drain_terminations(cx: Ctx<'_>, terminations: &mut Terminations) {
    while let Some((process, outcome)) = terminations.pop() {
        finish_process(cx, process, outcome, terminations);
    }
}

/// Make an owned process terminal:
///
/// 1. record the outcome: a tombstone for a child whose parent is alive,
///    nothing for a child whose parent is gone, a terminal status for a root;
/// 2. stop being live;
/// 3. cancel live children and reap the tombstones of terminated ones;
/// 4. wake processes blocked on this one.
///
/// Step 1 precedes step 2 so that "not live" always implies "terminal outcome
/// recorded or reaped". A child and its parent terminating concurrently each
/// record their own state before reading the other's, so either the child sees
/// the parent gone and reaps itself, or the parent sees the child's tombstone
/// and reaps it.
fn finish_process(
    cx: Ctx<'_>,
    mut process: Process,
    outcome: ChildOutcome,
    terminations: &mut Terminations,
) {
    let pid = process.pid;
    let children = std::mem::take(&mut process.children);
    cx.scheduler.remove(pid);

    match process.parent {
        Some(parent) => {
            // Free the VM, heap and stack before publishing the tombstone.
            drop(process);
            cx.table
                .tombstones
                .insert(pid, ChildRecord { parent, outcome });
            #[cfg(test)]
            cx.table
                .max_tombstones
                .fetch_max(cx.table.tombstones.len(), Ordering::SeqCst);
            if !cx.table.live.contains(&parent) {
                cx.table.tombstones.remove(&pid);
            }
        }
        None => {
            process.status = match outcome {
                ChildOutcome::Ok(_) => ProcessStatus::Done,
                ChildOutcome::Err(failure) => ProcessStatus::Failed(failure),
            };
            cx.table.processes.insert(pid, process);
        }
    }

    cx.table.live.remove(&pid);
    cx.table.pending_cancels.remove(&pid);

    // Scope exit: children must not outlive their parent. A child that is
    // terminating concurrently may already have its tombstone even though it
    // is still live, so always remove the tombstone after cancelling; a child
    // that records its tombstone later sees this process gone and reaps it.
    for child in children {
        if cx.table.live.contains(&child) {
            cancel_pid(cx, child, terminations);
        }
        cx.table.tombstones.remove(&child);
    }

    wake_any_waiters(cx, pid, terminations);
    wake_join_waiters(cx, pid, terminations);
}

/// Cancel a live process wherever it currently is.
fn cancel_pid(cx: Ctx<'_>, pid: Pid, terminations: &mut Terminations) {
    if try_cancel_in_table(cx, pid, terminations) {
        return;
    }
    // Out of the table: leave a request for its owner.
    cx.table.pending_cancels.insert(pid);
    if !cx.table.live.contains(&pid) {
        // It terminated meanwhile; don't leave a stale request behind.
        cx.table.pending_cancels.remove(&pid);
        return;
    }
    // The owner may have parked it before seeing the request.
    try_cancel_in_table(cx, pid, terminations);
}

/// Cancel `pid` if it is in the table. Returns false if it is not.
fn try_cancel_in_table(cx: Ctx<'_>, pid: Pid, terminations: &mut Terminations) -> bool {
    loop {
        if let Some(process) = cx.table.take_blocked(pid, |_| true) {
            // Cancelling a blocked process wakes it right away: it stops
            // waiting (timers and I/O are cancelled) and terminates now.
            clear_blocked_registration(cx, &process);
            terminations.push((process, ChildOutcome::Err(ProcessFailure::Cancelled)));
            return true;
        }
        let Some(mut process) = cx.table.processes.get_mut(&pid) else {
            return false;
        };
        match process.status {
            // Observed by the VM at the start of its next slice.
            ProcessStatus::Runnable => process.vm.request_cancellation(),
            // Parked between our two lookups; take it.
            ProcessStatus::Blocked(_) => continue,
            // A terminated root: nothing to cancel.
            ProcessStatus::Done | ProcessStatus::Failed(_) => {}
        }
        return true;
    }
}

/// Remove the registration that makes `process` (blocked, owned) wakeable.
fn clear_blocked_registration(cx: Ctx<'_>, process: &Process) {
    let pid = process.pid;
    let ProcessStatus::Blocked(reason) = &process.status else {
        return;
    };
    match reason {
        BlockReason::Await { child, .. } => remove_waiter(&cx.table.waiters, *child, pid),
        BlockReason::WaitAny(child_pids) => {
            remove_any_waiter_registration(cx.table, pid, child_pids)
        }
        BlockReason::Io(token) => {
            cx.table.io_waiters.remove(token);
            cx.io.cancel(*token);
        }
    }
}

// --- Worker loop ---------------------------------------------------------------

fn worker_loop(
    cx: Ctx<'_>,
    next_io_token: &AtomicU64,
    active_workers: &AtomicUsize,
    slices_started: &AtomicU64,
) {
    struct ActiveSlice<'a>(&'a AtomicUsize);

    impl Drop for ActiveSlice<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    loop {
        let pid = match cx.scheduler.dequeue() {
            Some(pid) => pid,
            None => return,
        };

        active_workers.fetch_add(1, Ordering::SeqCst);
        slices_started.fetch_add(1, Ordering::SeqCst);
        let _active_slice = ActiveSlice(active_workers);

        let reductions = cx.scheduler.reductions(pid);
        let Some(mut process) = cx.table.take_runnable(pid) else {
            continue;
        };

        // Cooperative cancellation is owned by the VM. A concurrent cancel
        // request can only mark intent here; the interpreter observes it at the
        // next slice boundary and returns `RunResult::Cancelled`.
        if cx.table.pending_cancels.remove(&pid).is_some() {
            process.vm.request_cancellation();
        }

        let result = process.vm.run_slice(reductions);
        handle_slice_result(cx, process, result, next_io_token);
    }
}

fn handle_slice_result(
    cx: Ctx<'_>,
    mut process: Process,
    result: RunResult,
    next_io_token: &AtomicU64,
) {
    let pid = process.pid;
    match result {
        RunResult::Done => {
            let outcome = if process.parent.is_some() {
                child_result(&mut process)
            } else {
                ChildOutcome::Ok(crate::sendable::SendableValue::Unit)
            };
            terminate_process(cx, process, outcome);
        }
        RunResult::Yielded => resume(cx, process),
        RunResult::Failed(failure) => {
            terminate_process(cx, process, ChildOutcome::Err(failure));
        }
        RunResult::Spawn {
            proto_idx,
            captures,
        } => {
            match crate::runtime_ops::create_child_vm_from_parent(&process.vm, proto_idx, captures)
            {
                Ok(child_vm) => {
                    let child_pid = cx.table.alloc_pid();
                    process.children.insert(child_pid);
                    cx.table.insert(
                        Process::new(child_pid, child_vm, Some(pid)).with_root(process.root),
                    );
                    cx.scheduler.enqueue(child_pid);

                    process.vm.stack.pop();
                    process.vm.push_value(Value::Pid(child_pid.0));
                    resume(cx, process);
                }
                Err(failure) => {
                    terminate_process(cx, process, ChildOutcome::Err(failure));
                }
            }
        }
        RunResult::Await(child_pid_val) => {
            handle_await(cx, process, Pid(child_pid_val), AwaitKind::Raw);
        }
        RunResult::AwaitResult(child_pid_val) => {
            handle_await(cx, process, Pid(child_pid_val), AwaitKind::Result);
        }
        RunResult::Cancel(child_pid_val) => {
            handle_cancel(cx, process, Pid(child_pid_val));
        }
        RunResult::WaitAny(child_pid_vals) => {
            let child_pids = child_pid_vals.into_iter().map(Pid).collect();
            handle_wait_any(cx, process, child_pids);
        }
        RunResult::Io(request) => {
            let token = IoToken(next_io_token.fetch_add(1, Ordering::Relaxed));
            let context = IoContext {
                owner: process.root.0,
                max_in_flight: usize::try_from(process.vm.io_limits().max_concurrent_io)
                    .unwrap_or(usize::MAX)
                    .max(1),
            };
            // Register and park before handing the request to the backend, so
            // a completion always finds the process parked.
            process.status = ProcessStatus::Blocked(BlockReason::Io(token));
            cx.table.io_waiters.insert(token, pid);
            let mut terminations = Terminations::new();
            park(cx, process, &mut terminations);
            drain_terminations(cx, &mut terminations);

            // Cancelled while parking: never start the request.
            if !cx.table.io_waiters.contains_key(&token) {
                return;
            }
            match cx.io.register_for(token, request, context) {
                Ok(()) => {
                    // Cancelled before the backend knew the token: release it.
                    if !cx.table.io_waiters.contains_key(&token) {
                        cx.io.cancel(token);
                    }
                }
                Err(message) => {
                    if cx.table.io_waiters.remove(&token).is_some()
                        && let Some(process) = cx.table.take_blocked(
                            pid,
                            |reason| matches!(reason, BlockReason::Io(t) if *t == token),
                        )
                    {
                        terminate_process(
                            cx,
                            process,
                            ChildOutcome::Err(ProcessFailure::runtime(message)),
                        );
                    }
                }
            }
        }
        RunResult::Cancelled => {
            terminate_process(cx, process, ChildOutcome::Err(ProcessFailure::Cancelled));
        }
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
            crate::sendable::serialize(val, &process.vm.heap)
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

// --- I/O completion ------------------------------------------------------------

fn handle_io_completion(cx: Ctx<'_>, token: IoToken, result: IoResult) {
    let Some((_, pid)) = cx.table.io_waiters.remove(&token) else {
        // Cancelled, or not ours: drop the late result.
        return;
    };

    let Some(mut process) = cx.table.take_blocked(
        pid,
        |reason| matches!(reason, BlockReason::Io(t) if *t == token),
    ) else {
        // Defensive: a registration for a process not waiting on it.
        remove_io_waiters_for_pid(cx.table, pid);
        return;
    };

    let failure = match result {
        IoResult::Ok { value, io_bytes } => {
            match process
                .vm
                .heap
                .charge_io_bytes(io_bytes)
                .map_err(|e| ProcessFailure::runtime(e.to_string()))
                .and_then(|()| deliver_result_to_parent(&mut process.vm, value))
            {
                Ok(()) => {
                    resume(cx, process);
                    return;
                }
                Err(failure) => failure,
            }
        }
        IoResult::Err(msg) => match process.vm.heap.charge_io_bytes(msg.len() as u64) {
            Ok(()) => ProcessFailure::runtime(msg),
            Err(err) => ProcessFailure::runtime(err.to_string()),
        },
    };
    terminate_process(cx, process, ChildOutcome::Err(failure));
}

// --- Process operations ----------------------------------------------------------

fn handle_await(cx: Ctx<'_>, mut parent: Process, child_pid: Pid, await_kind: AwaitKind) {
    let parent_pid = parent.pid;
    let mut terminations = Terminations::new();

    match cx.table.classify_child(&parent, child_pid) {
        ChildState::Live => {
            block_on_child(cx, parent, child_pid, await_kind, &mut terminations);
        }
        ChildState::Terminated => match cx.table.tombstones.remove(&child_pid) {
            Some((_, record)) => {
                consume_child(
                    cx,
                    parent,
                    child_pid,
                    record.outcome,
                    await_kind,
                    &mut terminations,
                );
            }
            None => {
                // Unreachable while the parent is alive; report it as joined.
                parent.children.remove(&child_pid);
                deliver_already_joined(cx, parent, await_kind, &mut terminations);
            }
        },
        ChildState::Reaped => {
            deliver_already_joined(cx, parent, await_kind, &mut terminations);
        }
        ChildState::NotChild => terminations.push((
            parent,
            ChildOutcome::Err(ProcessFailure::runtime(format!(
                "await: process {child_pid:?} is not a child of {parent_pid:?}"
            ))),
        )),
        ChildState::Unknown => terminations.push((
            parent,
            ChildOutcome::Err(ProcessFailure::runtime(format!(
                "await: unknown process {child_pid:?}"
            ))),
        )),
    }

    drain_terminations(cx, &mut terminations);
}

/// Deliver a terminated child's outcome to its owned parent, consuming it.
fn consume_child(
    cx: Ctx<'_>,
    mut parent: Process,
    child_pid: Pid,
    outcome: ChildOutcome,
    await_kind: AwaitKind,
    terminations: &mut Terminations,
) {
    parent.children.remove(&child_pid);
    match deliver_await_outcome(&mut parent.vm, await_kind, outcome) {
        Ok(()) => resume(cx, parent),
        Err(failure) => terminations.push((parent, ChildOutcome::Err(failure))),
    }
}

/// The child's result was already consumed (or the pid is no longer known).
fn deliver_already_joined(
    cx: Ctx<'_>,
    mut parent: Process,
    await_kind: AwaitKind,
    terminations: &mut Terminations,
) {
    match crate::runtime_ops::deliver_already_joined(&mut parent.vm, await_kind) {
        Ok(()) => resume(cx, parent),
        Err(failure) => terminations.push((parent, ChildOutcome::Err(failure))),
    }
}

/// Block an owned parent on a live child. See the module docs for the wake
/// protocol: register, park, then recheck.
fn block_on_child(
    cx: Ctx<'_>,
    mut parent: Process,
    child_pid: Pid,
    await_kind: AwaitKind,
    terminations: &mut Terminations,
) {
    let parent_pid = parent.pid;
    parent.status = ProcessStatus::Blocked(BlockReason::Await {
        child: child_pid,
        kind: await_kind,
    });
    cx.table
        .waiters
        .entry(child_pid)
        .or_default()
        .push(parent_pid);
    park(cx, parent, terminations);

    if !cx.table.live.contains(&child_pid) {
        wake_join_waiters(cx, child_pid, terminations);
    }
}

fn handle_cancel(cx: Ctx<'_>, mut parent: Process, child_pid: Pid) {
    let parent_pid = parent.pid;
    let mut terminations = Terminations::new();

    let failure = match cx.table.classify_child(&parent, child_pid) {
        ChildState::Live => {
            cancel_pid(cx, child_pid, &mut terminations);
            None
        }
        // Cancelling a finished child is a no-op.
        ChildState::Terminated | ChildState::Reaped => None,
        ChildState::NotChild => Some(format!(
            "cancel: process {child_pid:?} is not a child of {parent_pid:?}"
        )),
        ChildState::Unknown => Some(format!("cancel: unknown process {child_pid:?}")),
    };

    // Finish the cancelled child before the parent can observe it.
    drain_terminations(cx, &mut terminations);

    match failure {
        None => {
            parent.vm.stack.pop();
            parent.vm.push_value(Value::Unit);
            resume(cx, parent);
        }
        Some(message) => {
            terminate_process(
                cx,
                parent,
                ChildOutcome::Err(ProcessFailure::runtime(message)),
            );
        }
    }
}

fn handle_wait_any(cx: Ctx<'_>, mut parent: Process, child_pids: Vec<Pid>) {
    let parent_pid = parent.pid;
    let child_pids = dedup_pids(child_pids);

    if child_pids.is_empty() {
        terminate_process(
            cx,
            parent,
            ChildOutcome::Err(ProcessFailure::runtime(
                "wait_any: expected non-empty pid list",
            )),
        );
        return;
    }

    for &child_pid in &child_pids {
        let failure = match cx.table.classify_child(&parent, child_pid) {
            ChildState::Live => continue,
            // Already terminal: the leftmost such pid wins immediately.
            ChildState::Terminated | ChildState::Reaped => {
                match crate::runtime_ops::deliver_pid_to_parent(&mut parent.vm, child_pid) {
                    Ok(()) => resume(cx, parent),
                    Err(failure) => terminate_process(cx, parent, ChildOutcome::Err(failure)),
                }
                return;
            }
            ChildState::NotChild => {
                format!("wait_any: process {child_pid:?} is not a child of {parent_pid:?}")
            }
            ChildState::Unknown => format!("wait_any: unknown process {child_pid:?}"),
        };
        terminate_process(
            cx,
            parent,
            ChildOutcome::Err(ProcessFailure::runtime(failure)),
        );
        return;
    }

    // Every listed child is live. Register with each, park, then recheck.
    parent.status = ProcessStatus::Blocked(BlockReason::WaitAny(child_pids.clone()));
    for &child_pid in &child_pids {
        cx.table
            .any_waiters
            .entry(child_pid)
            .or_default()
            .push(parent_pid);
    }
    let mut terminations = Terminations::new();
    park(cx, parent, &mut terminations);

    if let Some(&finished) = child_pids
        .iter()
        .find(|child_pid| !cx.table.live.contains(child_pid))
    {
        wake_any_waiters(cx, finished, &mut terminations);
    }
    drain_terminations(cx, &mut terminations);
}

// --- Waking ----------------------------------------------------------------------

fn wake_any_waiters(cx: Ctx<'_>, finished_pid: Pid, terminations: &mut Terminations) {
    let Some(waiter_pids) = cx
        .table
        .any_waiters
        .get(&finished_pid)
        .map(|waiters| waiters.clone())
    else {
        return;
    };

    for waiter_pid in waiter_pids {
        let mut waiter = match cx.table.take_waiter(waiter_pid, |reason| {
            matches!(reason, BlockReason::WaitAny(child_pids) if child_pids.contains(&finished_pid))
        }) {
            Ok(waiter) => waiter,
            Err(NotTaken::InFlight) => continue,
            Err(NotTaken::Stale) => {
                remove_stale_waiter(cx.table, &cx.table.any_waiters, finished_pid, waiter_pid);
                continue;
            }
        };
        let ProcessStatus::Blocked(BlockReason::WaitAny(child_pids)) = &waiter.status else {
            unreachable!("take_waiter returned a process not blocked in wait_any");
        };
        let child_pids = child_pids.clone();

        // The finished pid is only a wakeup signal; the semantic winner is the
        // leftmost terminated pid in the parent's original wait_any list.
        // Every listed pid is a child of the waiter or already reaped, so
        // "not live" means "terminated".
        let Some(winner) = child_pids
            .iter()
            .copied()
            .find(|child_pid| !cx.table.live.contains(child_pid))
        else {
            park(cx, waiter, terminations);
            continue;
        };

        remove_any_waiter_registration(cx.table, waiter_pid, &child_pids);
        match crate::runtime_ops::deliver_pid_to_parent(&mut waiter.vm, winner) {
            Ok(()) => resume(cx, waiter),
            Err(failure) => terminations.push((waiter, ChildOutcome::Err(failure))),
        }
    }
}

fn wake_join_waiters(cx: Ctx<'_>, finished_pid: Pid, terminations: &mut Terminations) {
    let Some(waiter_pids) = cx
        .table
        .waiters
        .get(&finished_pid)
        .map(|waiters| waiters.clone())
    else {
        return;
    };

    if !cx.table.tombstones.contains_key(&finished_pid) {
        // Reaped: its parent is gone, so nobody can still be waiting.
        cx.table.waiters.remove(&finished_pid);
        return;
    }

    for waiter_pid in waiter_pids {
        let waiter = match cx.table.take_waiter(
            waiter_pid,
            |reason| matches!(reason, BlockReason::Await { child, .. } if *child == finished_pid),
        ) {
            Ok(waiter) => waiter,
            Err(NotTaken::InFlight) => continue,
            Err(NotTaken::Stale) => {
                remove_stale_waiter(cx.table, &cx.table.waiters, finished_pid, waiter_pid);
                continue;
            }
        };
        let ProcessStatus::Blocked(BlockReason::Await { kind, .. }) = waiter.status else {
            unreachable!("take_waiter returned a process not blocked in await");
        };
        remove_waiter(&cx.table.waiters, finished_pid, waiter_pid);

        // Only the parent may await a child, so this waiter is the parent and
        // delivery consumes the tombstone.
        match cx.table.tombstones.remove(&finished_pid) {
            Some((_, record)) => {
                consume_child(cx, waiter, finished_pid, record.outcome, kind, terminations);
            }
            None => {
                let mut waiter = waiter;
                waiter.children.remove(&finished_pid);
                deliver_already_joined(cx, waiter, kind, terminations);
            }
        }
    }
}

// --- Registration helpers ----------------------------------------------------------

fn remove_any_waiter_registration(table: &ProcessTable, waiter_pid: Pid, child_pids: &[Pid]) {
    for &child_pid in child_pids {
        remove_waiter(&table.any_waiters, child_pid, waiter_pid);
    }
}

fn remove_io_waiters_for_pid(table: &ProcessTable, pid: Pid) {
    let tokens: Vec<IoToken> = table
        .io_waiters
        .iter()
        .filter(|entry| *entry.value() == pid)
        .map(|entry| *entry.key())
        .collect();
    for token in tokens {
        table.io_waiters.remove(&token);
    }
}

/// Drop a registration that `take_waiter` found stale. A live waiter may
/// already be registered for a different child (a late waker raced with its
/// delivery and it has since blocked again), so only its entry for
/// `finished_pid` goes. A terminated waiter can hold no valid registration.
fn remove_stale_waiter(
    table: &ProcessTable,
    waiters: &DashMap<Pid, Vec<Pid>>,
    finished_pid: Pid,
    waiter_pid: Pid,
) {
    if table.live.contains(&waiter_pid) {
        remove_waiter(waiters, finished_pid, waiter_pid);
    } else {
        remove_waiter_globally(waiters, waiter_pid);
    }
}

fn remove_waiter_globally(waiters: &DashMap<Pid, Vec<Pid>>, waiter_pid: Pid) {
    let child_pids: Vec<Pid> = waiters.iter().map(|entry| *entry.key()).collect();
    for child_pid in child_pids {
        remove_waiter(waiters, child_pid, waiter_pid);
    }
}

fn remove_waiter(waiters: &DashMap<Pid, Vec<Pid>>, child_pid: Pid, waiter_pid: Pid) {
    waiters.remove_if_mut(&child_pid, |_, waiter_pids| {
        waiter_pids.retain(|pid| *pid != waiter_pid);
        waiter_pids.is_empty()
    });
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
        crate::test_support::compile_file_hermetic(path)
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

    fn ok_unit_io_result() -> IoResult {
        IoResult::Ok {
            value: crate::sendable::SendableValue::Unit,
            io_bytes: 0,
        }
    }

    fn ready_unit_child(parent: Pid) -> ChildRecord {
        ChildRecord {
            parent,
            outcome: ChildOutcome::Ok(crate::sendable::SendableValue::Unit),
        }
    }

    /// Run a lifecycle transition against `table` and finish any
    /// terminations it queued.
    fn with_ctx(table: &ProcessTable, f: impl FnOnce(Ctx<'_>, &mut Terminations)) {
        let scheduler = FifoScheduler::new(1000);
        let io = crate::io_backend::MockIoBackend::new();
        let cx = Ctx {
            table,
            scheduler: &scheduler,
            io: &io,
        };
        let mut terminations = Terminations::new();
        f(cx, &mut terminations);
        drain_terminations(cx, &mut terminations);
    }

    #[test]
    fn test_threaded_single_process() {
        let program = compile("val _ = println \"hello threaded\"");
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["hello threaded\n"]);
        runtime.check_invariants().unwrap();
    }

    #[test]
    fn test_threaded_monitor_does_not_stop_while_root_is_between_slices() {
        // While a slice runs, its process is out of the table. The monitor
        // must not mistake the moment it is put back for "all done".
        let program = compile(
            "fun loop n = if n = 0 then 0 else loop (n - 1)\n\
             val _ = loop 300000\n\
             val _ = println \"finished\"",
        );
        for _ in 0..20 {
            let runtime = ThreadedRuntime::new(1);
            let pid = runtime.spawn_root(program.clone());
            runtime.run_to_completion().unwrap();
            assert_eq!(runtime.table.get_output(pid), vec!["finished\n"]);
        }
    }

    #[test]
    fn test_threaded_invariants_reject_stale_wait_any_registration() {
        let runtime = ThreadedRuntime::new(1);
        let waiter = runtime.spawn_root(compile("val _ = ()"));
        let child = Pid(999);
        runtime.table.any_waiters.insert(child, vec![waiter]);

        let err = runtime.check_invariants().unwrap_err();
        assert!(err.contains("any_waiters contains unknown child"));
    }

    #[test]
    fn test_wake_any_waiters_removes_missing_waiter_registrations() {
        let table = ProcessTable::new();
        let waiter = Pid(10);
        let child1 = Pid(11);
        let child2 = Pid(12);
        table.any_waiters.insert(child1, vec![waiter]);
        table.any_waiters.insert(child2, vec![waiter]);

        with_ctx(&table, |cx, t| wake_any_waiters(cx, child1, t));

        assert!(!table.any_waiters.contains_key(&child1));
        assert!(
            !table
                .any_waiters
                .get(&child2)
                .is_some_and(|waiters| waiters.contains(&waiter))
        );
    }

    #[test]
    fn test_wake_any_waiters_drops_only_finished_entry_of_non_blocked_live_waiter() {
        let table = ProcessTable::new();
        let waiter = Pid(10);
        let child1 = Pid(11);
        let child2 = Pid(12);
        table.insert(Process::new(waiter, VM::new(compile("val _ = ()")), None));
        table.any_waiters.insert(child1, vec![waiter]);
        table.any_waiters.insert(child2, vec![waiter]);

        with_ctx(&table, |cx, t| wake_any_waiters(cx, child1, t));

        assert!(!table.any_waiters.contains_key(&child1));
        // The waiter is live, so it may register for other children at any
        // moment; only the entry for the finished child is removed.
        assert!(
            table
                .any_waiters
                .get(&child2)
                .is_some_and(|waiters| waiters.contains(&waiter))
        );
    }

    #[test]
    fn test_io_completion_removes_missing_waiter_registrations() {
        let table = ProcessTable::new();
        let pid = Pid(10);
        let token1 = IoToken(1);
        let token2 = IoToken(2);
        table.io_waiters.insert(token1, pid);
        table.io_waiters.insert(token2, pid);

        with_ctx(&table, |cx, _| {
            handle_io_completion(cx, token1, ok_unit_io_result())
        });

        assert!(!table.io_waiters.contains_key(&token1));
        assert!(!table.io_waiters.contains_key(&token2));
    }

    #[test]
    fn test_io_completion_removes_non_blocked_waiter_registrations() {
        let table = ProcessTable::new();
        let pid = Pid(10);
        let token1 = IoToken(1);
        let token2 = IoToken(2);
        table.insert(Process::new(pid, VM::new(compile("val _ = ()")), None));
        table.io_waiters.insert(token1, pid);
        table.io_waiters.insert(token2, pid);

        with_ctx(&table, |cx, _| {
            handle_io_completion(cx, token1, ok_unit_io_result())
        });

        assert!(!table.io_waiters.contains_key(&token1));
        assert!(!table.io_waiters.contains_key(&token2));
        assert!(matches!(
            table
                .processes
                .get(&pid)
                .map(|process| process.is_runnable()),
            Some(true)
        ));
    }

    #[test]
    fn test_wake_join_waiters_removes_missing_waiter_registrations() {
        let table = ProcessTable::new();
        let waiter = Pid(10);
        let child1 = Pid(11);
        let child2 = Pid(12);
        table.waiters.insert(child1, vec![waiter]);
        table.waiters.insert(child2, vec![waiter]);
        table.tombstones.insert(child1, ready_unit_child(waiter));

        with_ctx(&table, |cx, t| wake_join_waiters(cx, child1, t));

        assert!(!table.waiters.contains_key(&child1));
        assert!(
            !table
                .waiters
                .get(&child2)
                .is_some_and(|waiters| waiters.contains(&waiter))
        );
    }

    /// A late waker for a child the waiter already consumed must not drop
    /// the waiter's registration for the child it awaits now. (Found by the
    /// 50-run loop: the root's registration vanished and the deadlock safety
    /// net failed it.)
    #[test]
    fn test_late_join_wake_keeps_registration_for_current_child() {
        let table = ProcessTable::new();
        let waiter = Pid(10);
        let old_child = Pid(11);
        let current_child = Pid(12);
        let mut process = Process::new(waiter, VM::new(compile("val _ = ()")), None);
        process.status = ProcessStatus::Blocked(BlockReason::Await {
            child: current_child,
            kind: AwaitKind::Raw,
        });
        table.insert(process);
        table.live.insert(current_child);
        table.waiters.insert(old_child, vec![waiter]);
        table.waiters.insert(current_child, vec![waiter]);
        table.tombstones.insert(old_child, ready_unit_child(waiter));

        with_ctx(&table, |cx, t| wake_join_waiters(cx, old_child, t));

        assert!(!table.waiters.contains_key(&old_child));
        assert_eq!(
            table.waiters.get(&current_child).map(|w| w.clone()),
            Some(vec![waiter])
        );
    }

    /// Same race for wait_any.
    #[test]
    fn test_late_wait_any_wake_keeps_registration_for_current_children() {
        let table = ProcessTable::new();
        let waiter = Pid(10);
        let old_child = Pid(11);
        let current_child = Pid(12);
        let mut process = Process::new(waiter, VM::new(compile("val _ = ()")), None);
        process.status = ProcessStatus::Blocked(BlockReason::WaitAny(vec![current_child]));
        table.insert(process);
        table.live.insert(current_child);
        table.any_waiters.insert(old_child, vec![waiter]);
        table.any_waiters.insert(current_child, vec![waiter]);

        with_ctx(&table, |cx, t| wake_any_waiters(cx, old_child, t));

        assert!(!table.any_waiters.contains_key(&old_child));
        assert_eq!(
            table.any_waiters.get(&current_child).map(|w| w.clone()),
            Some(vec![waiter])
        );
    }

    #[test]
    fn test_wake_join_waiters_drops_only_finished_entry_of_non_blocked_live_waiter() {
        let table = ProcessTable::new();
        let waiter = Pid(10);
        let child1 = Pid(11);
        let child2 = Pid(12);
        table.insert(Process::new(waiter, VM::new(compile("val _ = ()")), None));
        table.waiters.insert(child1, vec![waiter]);
        table.waiters.insert(child2, vec![waiter]);
        table.tombstones.insert(child1, ready_unit_child(waiter));

        with_ctx(&table, |cx, t| wake_join_waiters(cx, child1, t));

        assert!(!table.waiters.contains_key(&child1));
        // The waiter is live, so it may register for other children at any
        // moment; only the entry for the finished child is removed.
        assert!(
            table
                .waiters
                .get(&child2)
                .is_some_and(|waiters| waiters.contains(&waiter))
        );
    }

    #[test]
    fn test_threaded_zero_workers_clamps_to_one() {
        let program = compile("val _ = println \"hello threaded\"");
        let runtime = ThreadedRuntime::new(0);
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

    struct FailingIoBackend;

    impl IoBackend for FailingIoBackend {
        fn register(
            &self,
            _token: crate::io_backend::IoToken,
            _request: crate::io_backend::IoRequest,
        ) -> Result<(), String> {
            Err("backend offline".into())
        }

        fn poll(&self) -> Vec<(crate::io_backend::IoToken, crate::io_backend::IoResult)> {
            Vec::new()
        }
    }

    #[test]
    fn test_threaded_io_register_failure_fails_process() {
        let program = compile("val _ = sleep 1");
        let runtime = ThreadedRuntime::new(1).with_io_backend(Arc::new(FailingIoBackend));
        let pid = runtime.spawn_root(program);

        runtime.run_to_completion().unwrap();

        let process = runtime.table.processes.get(&pid).unwrap();
        match &process.status {
            ProcessStatus::Failed(ProcessFailure::RuntimeError(message)) => {
                assert!(message.contains("backend offline"));
            }
            other => panic!("expected backend failure, got {other:?}"),
        }
        assert!(runtime.table.io_waiters.is_empty());
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
        // The cancelled child was tombstoned, then reaped when the root exited.
        assert_eq!(runtime.table.processes.len(), 1);
        assert!(runtime.table.tombstones.is_empty());
        runtime.check_invariants().unwrap();
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
    fn test_join_on_child_that_fails_during_io_returns_error() {
        let program = compile_file(&test_program_path("test_fiber_io_failure.hml"));
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        assert!(runtime.failure(pid).is_none(), "{:?}", runtime.failure(pid));
        assert_eq!(runtime.table.get_output(pid), vec!["err\n"]);
        runtime.check_invariants().unwrap();
    }

    #[test]
    fn test_child_fuel_exhaustion_exhausts_the_tree() {
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
        assert_eq!(status, "Failed(FuelExhausted)");
        assert!(output.is_empty(), "{output:?}");
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

    /// Local HTTP server that answers every request with `body` after
    /// `delay`, handling each connection on its own thread.
    #[cfg(feature = "builtin-http")]
    fn delayed_http_server(delay: std::time::Duration, body: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                std::thread::spawn(move || {
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request);
                    std::thread::sleep(delay);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        format!("http://{addr}/")
    }

    /// Program for the pool-fairness tests: `slow_count` requests to
    /// `slow_url`, then a `sleep 10` and one request to `fast_url`, each of
    /// which prints how long it took in milliseconds.
    #[cfg(feature = "builtin-http")]
    fn pool_fairness_program(slow_url: &str, slow_count: usize, fast_url: &str) -> CompiledProgram {
        let slows = (0..slow_count)
            .map(|_| format!("spawn (fn () => http_get {slow_url:?})"))
            .collect::<Vec<_>>()
            .join(", ");
        compile(&format!(
            "val slows = [{slows}]\n\
             val sleeper = spawn (fn () =>\n\
               let val a = monotonic_ms () val _ = sleep 10 in monotonic_ms () - a end)\n\
             val fast = spawn (fn () =>\n\
               let val a = monotonic_ms () val _ = http_get {fast_url:?} in monotonic_ms () - a end)\n\
             val _ = println (int_to_string (await_process sleeper))\n\
             val _ = println (int_to_string (await_process fast))"
        ))
    }

    #[cfg(feature = "builtin-http")]
    fn http_test_vm(program: CompiledProgram) -> crate::builder::VMBuilder {
        crate::builder::VMBuilder::new(program)
            .with_core()
            .register_builtin_name("spawn")
            .register_builtin_name("await_process")
            .allow_http_builtin("http_get", vec!["127.0.0.1".to_string()])
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_slow_io_does_not_delay_sleep_or_fast_requests() {
        let slow_url = delayed_http_server(std::time::Duration::from_secs(2), "slow");
        let fast_url = delayed_http_server(std::time::Duration::ZERO, "fast");
        let vm = http_test_vm(pool_fairness_program(&slow_url, 8, &fast_url)).build();
        let runtime =
            ThreadedRuntime::new(1).with_io_backend(Arc::new(ThreadPoolIoBackend::new(4)));
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();

        let output = runtime.table.get_output(pid);
        let sleep_ms: u64 = output[0].trim().parse().unwrap();
        let fast_ms: u64 = output[1].trim().parse().unwrap();
        eprintln!("sleep 10 took {sleep_ms}ms, fast request took {fast_ms}ms");
        assert!(sleep_ms <= 60, "sleep 10 took {sleep_ms}ms");
        assert!(fast_ms <= 100, "fast request took {fast_ms}ms");
    }

    /// `count` spawned `http_get url` children, then await each one's result
    /// so a failed child does not fail the root.
    #[cfg(feature = "builtin-http")]
    fn spawn_and_join_program(
        url: &str,
        count: usize,
        prelude: &str,
        epilogue: &str,
    ) -> CompiledProgram {
        let mut source = String::from(prelude);
        for i in 0..count {
            source.push_str(&format!("val c{i} = spawn (fn () => http_get {url:?})\n"));
        }
        for i in 0..count {
            source.push_str(&format!("val _ = await_process_result c{i}\n"));
        }
        source.push_str(epilogue);
        compile(&source)
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_http_timeout_counts_time_spent_queued() {
        // Two requests may run at once; four more queue behind 2s responses.
        // Every request must end within its 500ms timeout of being issued.
        let slow_url = delayed_http_server(std::time::Duration::from_secs(2), "slow");
        let program = spawn_and_join_program(&slow_url, 6, "", "");
        let vm = http_test_vm(program)
            .register_builtin_name("await_process_result")
            .http_timeout_ms(500)
            .max_concurrent_io(2)
            .build();
        let runtime =
            ThreadedRuntime::new(1).with_io_backend(Arc::new(ThreadPoolIoBackend::new(4)));
        let pid = runtime.spawn_root_vm(vm);

        let started = std::time::Instant::now();
        runtime.run_to_completion().unwrap();
        let elapsed = started.elapsed();

        assert!(runtime.failure(pid).is_none(), "{:?}", runtime.failure(pid));
        assert!(
            elapsed >= std::time::Duration::from_millis(450)
                && elapsed <= std::time::Duration::from_millis(800),
            "requests ended after {elapsed:?}"
        );
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_process_tree_over_io_limit_waits_while_others_proceed() {
        let slow_url = delayed_http_server(std::time::Duration::from_millis(500), "slow");
        let fast_url = delayed_http_server(std::time::Duration::ZERO, "fast");

        // Tree A may run two requests at once, so its six take three rounds.
        let busy = spawn_and_join_program(
            &slow_url,
            6,
            "val t0 = monotonic_ms ()\n",
            "val _ = println (int_to_string (monotonic_ms () - t0))",
        );
        let busy_vm = http_test_vm(busy)
            .register_builtin_name("await_process_result")
            .max_concurrent_io(2)
            .build();
        // Tree B starts its request once A's are queued.
        let other = compile(&format!(
            "val _ = sleep 100\n\
             val a = monotonic_ms ()\n\
             val _ = http_get {fast_url:?}\n\
             val _ = println (int_to_string (monotonic_ms () - a))"
        ));
        let other_vm = http_test_vm(other).build();

        let runtime =
            ThreadedRuntime::new(1).with_io_backend(Arc::new(ThreadPoolIoBackend::new(4)));
        let busy_pid = runtime.spawn_root_vm(busy_vm);
        let other_pid = runtime.spawn_root_vm(other_vm);
        runtime.run_to_completion().unwrap();

        let busy_ms: u64 = runtime.table.get_output(busy_pid)[0]
            .trim()
            .parse()
            .unwrap();
        let other_ms: u64 = runtime.table.get_output(other_pid)[0]
            .trim()
            .parse()
            .unwrap();
        assert!(
            busy_ms >= 1400,
            "tree A ran more than two at once: {busy_ms}ms"
        );
        assert!(other_ms <= 100, "tree B waited {other_ms}ms");
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_http_timeout_does_not_block_other_processes() {
        // A server that accepts the connection and never answers.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(4));
            drop(stream);
        });

        // With one VM worker, the second process can only run before the
        // HTTP timeout if the pending request does not occupy that worker.
        let program = compile(&format!(
            "val t0 = monotonic_ms ()\n\
             val slow = spawn (fn () => http_get \"http://{addr}/\")\n\
             val counter = spawn (fn () => monotonic_ms ())\n\
             val t_counter = await_process counter\n\
             val _ = println (int_to_string (t_counter - t0))\n\
             val _ = await_process slow"
        ));
        let vm = crate::builder::VMBuilder::new(program)
            .with_core()
            .register_builtin_name("spawn")
            .register_builtin_name("await_process")
            .allow_http_builtin("http_get", vec!["127.0.0.1".to_string()])
            .http_timeout_ms(1000)
            .build();
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root_vm(vm);

        let started = std::time::Instant::now();
        runtime.run_to_completion().unwrap();
        let elapsed = started.elapsed();

        let failure = runtime
            .failure(pid)
            .expect("root fails when http_get times out");
        let failure = format!("{failure:?}");
        assert!(failure.contains("http_get: http: timeout"), "{failure}");
        assert!(
            elapsed >= std::time::Duration::from_millis(900)
                && elapsed <= std::time::Duration::from_secs(2),
            "timeout took {elapsed:?}"
        );
        let output = runtime.table.get_output(pid);
        let counter_ms: u64 = output[0].trim().parse().unwrap();
        assert!(counter_ms < 900, "second process ran after {counter_ms}ms");
        server.join().unwrap();
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

    /// `count` children each evaluating `child`, awaited with
    /// `await_process_result` so that a failed child does not fail the root
    /// by itself. The root prints how many children succeeded.
    fn spawn_children_program(count: usize, child: &str) -> CompiledProgram {
        let mut source = String::from("import __Builtin.Process\n");
        source.push_str(&format!(
            "fun spawn_all n acc = if n = 0 then acc else spawn_all (n - 1) (spawn (fn () => {child}) :: acc)\n\
             fun join_all ps ok = case ps of [] => ok | p :: rest => (case BuiltinProcess.await_result_raw p of BuiltinProcess.Ok _ => join_all rest (ok + 1) | BuiltinProcess.Err _ => join_all rest ok)\n\
             val ok = join_all (spawn_all {count} []) 0\n\
             val _ = println (int_to_string ok)\n"
        ));
        crate::test_support::compile_source_hermetic(&source)
    }

    fn tree_budget_vm(program: CompiledProgram) -> crate::builder::VMBuilder {
        crate::builder::VMBuilder::new(program)
            .with_core()
            .register_builtin_name("spawn")
            .register_builtin_name("await_process_result")
    }

    #[test]
    fn test_children_share_the_tree_fuel_budget() {
        const FUEL: u64 = 200_000;
        let program = spawn_children_program(100, "let fun spin () = spin () in spin () end");
        let vm = tree_budget_vm(program).max_fuel(FUEL).build();
        let budget = vm.fuel_budget().unwrap().clone();
        let runtime = ThreadedRuntime::new(4);
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();

        assert_eq!(runtime.failure(pid), Some(ProcessFailure::FuelExhausted));
        assert_eq!(budget.used(), FUEL, "the tree spent exactly its budget");
        assert_eq!(runtime.table.processes.len(), 1);
    }

    #[test]
    fn test_children_share_the_tree_io_budget() {
        const IO_BYTES: u64 = 1_000;
        let program = spawn_children_program(
            100,
            "let fun spam () = let val _ = print \"0123456789\" in spam () end in spam () end",
        );
        let vm = tree_budget_vm(program).max_io_bytes(IO_BYTES).build();
        let budget = vm.heap().io_budget().unwrap().clone();
        let runtime = ThreadedRuntime::new(4);
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();

        // 100 ten-byte prints fill the budget exactly; every child then
        // fails, and so does the root's final println.
        assert_eq!(budget.used(), IO_BYTES);
        let failure = format!("{:?}", runtime.failure(pid));
        assert!(failure.contains("io limit exceeded"), "{failure}");
        assert!(runtime.table.get_output(pid).is_empty());
    }

    #[test]
    fn test_children_share_the_tree_host_work_budget() {
        const HOST_WORK: u64 = 5_000;
        let program = spawn_children_program(
            100,
            "let fun measure n = measure (n + string_length \"abc\") in measure 0 end",
        );
        let vm = tree_budget_vm(program).max_host_work(HOST_WORK).build();
        let budget = vm.heap().host_work_budget().unwrap().clone();
        let runtime = ThreadedRuntime::new(4);
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();

        // Children charge host work until the budget cannot cover the next
        // call, then fail. Receiving a failed join result costs no host
        // work, so the root finishes and reports that no child succeeded.
        let used = budget.used();
        assert!(used <= HOST_WORK && used > HOST_WORK - 100, "used {used}");
        assert!(runtime.failure(pid).is_none(), "{:?}", runtime.failure(pid));
        assert_eq!(runtime.table.get_output(pid), vec!["0\n"]);
    }

    #[test]
    fn test_unused_child_budget_stays_with_the_tree() {
        // The child uses a little fuel and returns; the parent can then spend
        // everything the child did not use.
        const FUEL: u64 = 100_000;
        let program = compile(
            "fun count n = if n = 0 then 0 else count (n - 1)\n\
             val c = spawn (fn () => count 100)\n\
             val _ = await_process_result c\n\
             fun spin () = spin ()\n\
             val _ = spin ()\n",
        );
        let vm = tree_budget_vm(program).max_fuel(FUEL).build();
        let budget = vm.fuel_budget().unwrap().clone();
        let runtime = ThreadedRuntime::new(2);
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();

        assert_eq!(runtime.failure(pid), Some(ProcessFailure::FuelExhausted));
        assert_eq!(budget.used(), FUEL, "nothing is lost or counted twice");
    }

    #[test]
    fn test_spawn_capturing_exponential_dag_fails_fast() {
        // `grow 30` is 31 heap objects that expand into 2^31 nodes when
        // copied. (The plan's `val x1 = (x0, x0) ... val x30` version gives a
        // tuple type with 2^30 leaves, which the type checker cannot handle.)
        let program = compile(
            "datatype t = Leaf | Node of t * t\n\
             fun grow n acc = if n = 0 then acc else grow (n - 1) (Node (acc, acc))\n\
             val c = let val x30 = grow 30 Leaf in spawn (fn () => case x30 of Leaf => 0 | Node _ => 1) end\n\
             val _ = await_process_result c\n",
        );
        let vm = tree_budget_vm(program).build();
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root_vm(vm);
        let started = std::time::Instant::now();
        runtime.run_to_completion().unwrap();
        let elapsed = started.elapsed();

        let failure = format!("{:?}", runtime.failure(pid));
        assert!(failure.contains("max_message_bytes"), "{failure}");
        eprintln!("x30 capture failed after {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "took {elapsed:?}"
        );
    }

    #[test]
    fn test_spawn_limits_are_enforced() {
        // Ten children that never finish: the fourth spawn exceeds
        // max_live_processes = 3 and fails the root.
        let program = compile(
            "fun spawn_all n = if n = 0 then () else let val _ = spawn (fn () => let fun spin () = spin () in spin () end) in spawn_all (n - 1) end\n\
             val _ = spawn_all 10\n",
        );
        let vm = tree_budget_vm(program).max_live_processes(3).build();
        let runtime = ThreadedRuntime::new(2);
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();
        let failure = format!("{:?}", runtime.failure(pid));
        assert!(failure.contains("max_live_processes (3)"), "{failure}");
        assert_eq!(runtime.table.processes.len(), 1, "children are cancelled");

        // Children that finish free their slot, but max_spawns counts every
        // spawn.
        let program = compile(
            "fun spawn_all n = if n = 0 then () else let val c = spawn (fn () => 0) val _ = await_process_result c in spawn_all (n - 1) end\n\
             val _ = spawn_all 10\n\
             val _ = println \"done\"\n",
        );
        let vm = tree_budget_vm(program.clone())
            .max_live_processes(1)
            .build();
        let runtime = ThreadedRuntime::new(2);
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();
        assert!(runtime.failure(pid).is_none(), "{:?}", runtime.failure(pid));

        let vm = tree_budget_vm(program).max_spawns(5).build();
        let runtime = ThreadedRuntime::new(2);
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();
        let failure = format!("{:?}", runtime.failure(pid));
        assert!(failure.contains("max_spawns (5)"), "{failure}");
    }
}
