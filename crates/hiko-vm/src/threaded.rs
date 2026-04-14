//! Multi-threaded runtime: N worker threads executing hiko processes in parallel.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;

use crate::io_backend::{IoBackend, IoToken, MockIoBackend};
use crate::process::{BlockReason, Pid, Process, ProcessStatus, Scope, ScopeId};
use crate::runtime_ops::{
    ChildState, check_child_state, create_child_vm, deliver_message, deliver_result_to_parent,
    prepare_delivery,
};
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::sendable::SendableValue;
use crate::value::Value;
use crate::vm::{RunResult, VM};
use hiko_compile::chunk::CompiledProgram;

/// Thread-safe process table using DashMap for fine-grained locking.
struct ProcessTable {
    processes: DashMap<Pid, Process>,
    waiters: DashMap<Pid, Vec<Pid>>,
    io_waiters: DashMap<IoToken, Pid>,
    scopes: DashMap<ScopeId, Scope>,
}

impl ProcessTable {
    fn new() -> Self {
        Self {
            processes: DashMap::new(),
            waiters: DashMap::new(),
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
    /// (blocked on Receive/Await with no possible waker).
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
                ProcessStatus::Blocked(BlockReason::Receive)
                    | ProcessStatus::Blocked(BlockReason::Await(_))
            )
        }) && !self.processes.iter().any(|e| e.is_runnable())
    }
}

/// Multi-threaded hiko runtime.
pub struct ThreadedRuntime {
    next_pid: Arc<AtomicU64>,
    next_io_token: Arc<AtomicU64>,
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
            table: Arc::new(ProcessTable::new()),
            scheduler: Arc::new(FifoScheduler::new(1000)),
            io_backend: Arc::new(MockIoBackend::new()),
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
        let vm = VM::new(program);
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
                let io_backend = Arc::clone(&self.io_backend);

                std::thread::spawn(move || {
                    worker_loop(&table, &*scheduler, &next_pid, &next_io_token, &*io_backend);
                })
            })
            .collect();

        // Monitor: poll I/O completions and check for termination
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1));

            // Poll I/O backend for completed operations
            let completions = self.io_backend.poll();
            for (token, result) in completions {
                if let Some((_, pid)) = self.table.io_waiters.remove(&token) {
                    if let Some(mut process) = self.table.processes.get_mut(&pid) {
                        let resume_val = match result {
                            crate::io_backend::IoResult::Ok(sv) => {
                                let val = crate::sendable::deserialize(sv, &mut process.vm.heap);
                                crate::runtime_ops::make_io_ok(&mut process.vm, val)
                            }
                            crate::io_backend::IoResult::Err(msg) => {
                                crate::runtime_ops::make_io_err(&mut process.vm, &msg)
                            }
                        };
                        if process.vm.blocked_continuation.is_some() {
                            process.vm.resume_blocked(resume_val);
                        } else {
                            process.vm.stack.pop();
                            process.vm.push_value(resume_val);
                        }
                        process.status = ProcessStatus::Runnable;
                        drop(process);
                        self.scheduler.enqueue(pid);
                    }
                }
            }

            if self.table.is_all_done_or_blocked() {
                let has_io_waiters = !self.table.io_waiters.is_empty();
                if !has_io_waiters {
                    // Detect deadlock: blocked processes with no possible waker
                    if self.table.has_permanently_blocked() {
                        // Mark permanently blocked processes as failed
                        for mut entry in self.table.processes.iter_mut() {
                            if matches!(
                                entry.status,
                                ProcessStatus::Blocked(BlockReason::Receive)
                                    | ProcessStatus::Blocked(BlockReason::Await(_))
                            ) {
                                entry.status = ProcessStatus::Failed(
                                    "deadlock: process blocked with no possible waker".into(),
                                );
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

fn worker_loop(
    table: &ProcessTable,
    scheduler: &dyn Scheduler,
    next_pid: &AtomicU64,
    next_io_token: &AtomicU64,
    io_backend: &dyn IoBackend,
) {
    loop {
        let pid = match scheduler.dequeue() {
            Some(pid) => pid,
            None => return,
        };

        let reductions = scheduler.reductions(pid);
        let mut process = match table.take(pid) {
            Some(p) => p,
            None => continue,
        };

        let result = process.vm.run_slice(reductions);

        match result {
            RunResult::Done => {
                process.status = ProcessStatus::Done;
                table.return_process(process);
                scheduler.remove(pid);
                wake_waiters(table, scheduler, pid);
            }
            RunResult::Yielded => {
                table.return_process(process);
                scheduler.enqueue(pid);
            }
            RunResult::Failed(msg) => {
                process.status = ProcessStatus::Failed(msg);
                table.return_process(process);
                scheduler.remove(pid);
                wake_waiters(table, scheduler, pid);
            }
            RunResult::Spawn {
                proto_idx,
                captures,
            } => {
                let child_pid = Pid(next_pid.fetch_add(1, Ordering::Relaxed));
                let child_vm = crate::runtime_ops::create_child_vm_from_parent(
                    &process.vm,
                    proto_idx,
                    captures,
                );
                let child = Process::new(child_pid, child_vm, Some(pid));
                table.insert(child);
                scheduler.enqueue(child_pid);

                process.vm.stack.pop();
                process.vm.push_value(Value::Int(child_pid.0 as i64));
                table.return_process(process);
                scheduler.enqueue(pid);
            }
            RunResult::Await(child_pid_val) => {
                let child_pid = Pid(child_pid_val);
                handle_await(table, scheduler, process, child_pid);
            }
            RunResult::Send { target_pid, value } => {
                handle_send(table, scheduler, process, Pid(target_pid), value);
            }
            RunResult::Receive => {
                handle_receive(table, scheduler, process);
            }
            RunResult::Io(request) => {
                // Register I/O with backend, block process
                let token = IoToken(next_io_token.fetch_add(1, Ordering::Relaxed));
                process.status = ProcessStatus::Blocked(BlockReason::Io(token));
                table.return_process(process);
                table.io_waiters.insert(token, pid);
                io_backend.register(token, request);
            }
            RunResult::RuntimeEffect { tag, payload } => {
                // Effect-based I/O: process has saved its continuation.
                // Map the effect tag to an I/O request and register with backend.
                let token = IoToken(next_io_token.fetch_add(1, Ordering::Relaxed));
                let io_request = crate::io_backend::IoRequest::Custom {
                    operation: format!("effect_{tag}"),
                    payload,
                };
                process.status = ProcessStatus::Blocked(BlockReason::Io(token));
                table.return_process(process);
                table.io_waiters.insert(token, pid);
                io_backend.register(token, io_request);
            }
            RunResult::Cancelled => {
                process.status = ProcessStatus::Failed("cancelled".into());
                table.return_process(process);
                scheduler.remove(pid);
                wake_waiters(table, scheduler, pid);
            }
        }
    }
}

fn handle_await(
    table: &ProcessTable,
    scheduler: &dyn Scheduler,
    mut parent: Process,
    child_pid: Pid,
) {
    let parent_pid = parent.pid;

    let child_state = {
        match table.processes.get(&child_pid) {
            None => ChildState::NotFound,
            Some(entry) => {
                let c = entry.value();
                check_child_state(
                    Some((&c.status, c.parent)),
                    parent_pid,
                    c.vm.stack.last().copied(),
                    Some(&c.vm.heap),
                )
            }
        }
    };

    match child_state {
        ChildState::NotFound => {
            parent.status =
                ProcessStatus::Failed(format!("await: unknown process {:?}", child_pid));
            table.return_process(parent);
        }
        ChildState::NotChild => {
            parent.status = ProcessStatus::Failed(format!(
                "await: process {:?} is not a child of {:?}",
                child_pid, parent_pid
            ));
            table.return_process(parent);
        }
        ChildState::Done(sendable) => {
            deliver_result_to_parent(&mut parent.vm, sendable);
            table.return_process(parent);
            scheduler.enqueue(parent_pid);
        }
        ChildState::Failed(msg) => {
            parent.status = ProcessStatus::Failed(format!("child process failed: {msg}"));
            table.return_process(parent);
        }
        ChildState::Running => {
            parent.status = ProcessStatus::Blocked(BlockReason::Await(child_pid));
            table.return_process(parent);
            table.waiters.entry(child_pid).or_default().push(parent_pid);
        }
    }
}

fn handle_send(
    table: &ProcessTable,
    scheduler: &dyn Scheduler,
    mut sender: Process,
    target_pid: Pid,
    value: SendableValue,
) {
    let sender_pid = sender.pid;

    // Self-send: put message in own mailbox
    if target_pid == sender_pid {
        sender.mailbox.push_back(value);
        table.return_process(sender);
        scheduler.enqueue(sender_pid);
        return;
    }

    // Check target existence while sender is still held (no TOCTOU)
    let target_exists = table.processes.contains_key(&target_pid);

    if !target_exists {
        sender.status =
            ProcessStatus::Failed(format!("send_message: unknown process {:?}", target_pid));
        table.return_process(sender);
        return;
    }

    // Target exists — return sender, then deliver
    table.return_process(sender);

    if let Some(mut target) = table.processes.get_mut(&target_pid) {
        if matches!(target.status, ProcessStatus::Blocked(BlockReason::Receive)) {
            target.status = ProcessStatus::Runnable;
            deliver_message(&mut target.vm, value);
            drop(target);
            scheduler.enqueue(target_pid);
        } else {
            target.mailbox.push_back(value);
        }
    }
    scheduler.enqueue(sender_pid);
}

fn handle_receive(table: &ProcessTable, scheduler: &dyn Scheduler, mut process: Process) {
    let pid = process.pid;
    if let Some(msg) = process.mailbox.pop_front() {
        deliver_message(&mut process.vm, msg);
        table.return_process(process);
        scheduler.enqueue(pid);
    } else {
        process.status = ProcessStatus::Blocked(BlockReason::Receive);
        table.return_process(process);
    }
}

fn wake_waiters(table: &ProcessTable, scheduler: &dyn Scheduler, finished_pid: Pid) {
    let waiter_pids = match table.waiters.remove(&finished_pid) {
        Some((_, w)) => w,
        None => return,
    };

    let delivery = table
        .processes
        .get(&finished_pid)
        .map(|p| prepare_delivery(&p.status, &p.vm));

    let delivery = match delivery {
        Some(d) => d,
        None => return,
    };

    for waiter_pid in waiter_pids {
        if let Some(mut waiter) = table.processes.get_mut(&waiter_pid) {
            match &delivery {
                Ok(sendable) => {
                    deliver_result_to_parent(&mut waiter.vm, sendable.clone());
                    waiter.status = ProcessStatus::Runnable;
                    drop(waiter);
                    scheduler.enqueue(waiter_pid);
                }
                Err(msg) => {
                    waiter.status = ProcessStatus::Failed(format!("child process failed: {msg}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;

    fn compile(source: &str) -> CompiledProgram {
        let tokens = Lexer::new(source, 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile(program).unwrap();
        compiled
    }

    #[test]
    fn test_threaded_single_process() {
        let program = compile("val _ = println \"hello threaded\"");
        let runtime = ThreadedRuntime::new(2);
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
        let runtime = ThreadedRuntime::new(2);
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
        let runtime = ThreadedRuntime::new(4);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["30\n"]);
    }

    #[test]
    fn test_threaded_send_receive() {
        let program = compile(
            "val child = spawn (fn () =>\n\
               let val (msg : Int) = receive_message ()\n\
               in msg end)\n\
             val _ = send_message (child, 99)\n\
             val result = await_process child\n\
             val _ = println (int_to_string result)",
        );
        let runtime = ThreadedRuntime::new(2);
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();
        let output = runtime.table.get_output(pid);
        assert_eq!(output, vec!["99\n"]);
    }
}
