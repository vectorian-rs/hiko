//! Multi-threaded runtime: N worker threads executing hiko processes in parallel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::process::{BlockReason, Pid, Process, ProcessStatus};
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::sendable::{SendableValue, deserialize, serialize};
use crate::value::Value;
use crate::vm::{RunResult, VM};
use hiko_compile::chunk::CompiledProgram;

/// Thread-safe process table.
struct ProcessTable {
    processes: Mutex<HashMap<Pid, Process>>,
    waiters: Mutex<HashMap<Pid, Vec<Pid>>>,
}

impl ProcessTable {
    fn new() -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
        }
    }

    fn insert(&self, process: Process) {
        self.processes.lock().unwrap().insert(process.pid, process);
    }

    /// Take a process out of the table for exclusive execution.
    fn take(&self, pid: Pid) -> Option<Process> {
        self.processes.lock().unwrap().remove(&pid)
    }

    /// Return a process to the table after execution.
    fn return_process(&self, process: Process) {
        self.processes.lock().unwrap().insert(process.pid, process);
    }

    /// Get a process's status.
    fn get_status(&self, pid: Pid) -> Option<String> {
        self.processes
            .lock()
            .unwrap()
            .get(&pid)
            .map(|p| match &p.status {
                ProcessStatus::Done => "done".into(),
                ProcessStatus::Failed(m) => format!("failed:{m}"),
                ProcessStatus::Runnable => "runnable".into(),
                ProcessStatus::Blocked(_) => "blocked".into(),
            })
    }

    fn get_parent(&self, pid: Pid) -> Option<Option<Pid>> {
        self.processes.lock().unwrap().get(&pid).map(|p| p.parent)
    }

    fn get_output(&self, pid: Pid) -> Vec<String> {
        self.processes
            .lock()
            .unwrap()
            .get(&pid)
            .map(|p| p.vm.get_output().to_vec())
            .unwrap_or_default()
    }

    fn all_outputs(&self) -> Vec<String> {
        let procs = self.processes.lock().unwrap();
        let mut out = Vec::new();
        for p in procs.values() {
            out.extend(p.vm.get_output().iter().cloned());
        }
        out
    }

    fn is_all_done_or_blocked(&self) -> bool {
        let procs = self.processes.lock().unwrap();
        procs
            .values()
            .all(|p| p.is_done() || matches!(p.status, ProcessStatus::Blocked(_)))
    }
}

/// Multi-threaded hiko runtime.
pub struct ThreadedRuntime {
    next_pid: Arc<AtomicU64>,
    table: Arc<ProcessTable>,
    scheduler: Arc<dyn Scheduler>,
    num_workers: usize,
}

impl ThreadedRuntime {
    pub fn new(num_workers: usize) -> Self {
        Self {
            next_pid: Arc::new(AtomicU64::new(1)),
            table: Arc::new(ProcessTable::new()),
            scheduler: Arc::new(FifoScheduler::new(1000)),
            num_workers,
        }
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

                std::thread::spawn(move || {
                    worker_loop(&table, &*scheduler, &next_pid);
                })
            })
            .collect();

        // Monitor: wait for all processes to complete, then signal shutdown
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1));
            if self.table.is_all_done_or_blocked() {
                self.scheduler.shutdown();
                break;
            }
        }

        for h in handles {
            h.join().unwrap();
        }

        Ok(self.table.all_outputs())
    }
}

fn worker_loop(table: &ProcessTable, scheduler: &dyn Scheduler, next_pid: &AtomicU64) {
    loop {
        // Block until work is available or shutdown
        let pid = match scheduler.dequeue() {
            Some(pid) => pid,
            None => return, // shutdown signaled
        };

        let reductions = scheduler.reductions(pid);

        // Take the process out for exclusive access
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
                let program = process.vm.get_program();
                let mut child_vm = VM::new(program);
                let child_captures: Vec<Value> = captures
                    .into_iter()
                    .map(|v| deserialize(v, &mut child_vm.heap))
                    .collect();
                child_vm.setup_closure_call(proto_idx, &child_captures);
                let child = Process::new(child_pid, child_vm, Some(pid));
                table.insert(child);
                scheduler.enqueue(child_pid);

                // Resume parent with child pid
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

    enum ChildState {
        Done(SendableValue),
        Failed(String),
        Running,
        NotFound,
        NotChild,
    }

    let child_state = {
        let procs = table.processes.lock().unwrap();
        match procs.get(&child_pid) {
            None => ChildState::NotFound,
            Some(c) if c.parent != Some(parent_pid) => ChildState::NotChild,
            Some(c) => match &c.status {
                ProcessStatus::Done => {
                    let val = c.vm.stack.last().copied().unwrap_or(Value::Unit);
                    ChildState::Done(serialize(val, &c.vm.heap).unwrap_or(SendableValue::Unit))
                }
                ProcessStatus::Failed(msg) => ChildState::Failed(msg.clone()),
                _ => ChildState::Running,
            },
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
            parent.vm.stack.pop();
            let val = deserialize(sendable, &mut parent.vm.heap);
            parent.vm.push_value(val);
            table.return_process(parent);
            scheduler.enqueue(parent_pid);
        }
        ChildState::Failed(msg) => {
            parent.status = ProcessStatus::Failed(format!("child process failed: {msg}"));
            table.return_process(parent);
        }
        ChildState::Running => {
            // Child still running — block parent
            parent.status = ProcessStatus::Blocked(BlockReason::Await(child_pid));
            table.return_process(parent);
            table
                .waiters
                .lock()
                .unwrap()
                .entry(child_pid)
                .or_default()
                .push(parent_pid);
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
    let mut procs = table.processes.lock().unwrap();

    match procs.get_mut(&target_pid) {
        Some(target) => {
            if matches!(target.status, ProcessStatus::Blocked(BlockReason::Receive)) {
                target.status = ProcessStatus::Runnable;
                target.vm.stack.pop();
                let val = deserialize(value, &mut target.vm.heap);
                target.vm.push_value(val);
                scheduler.enqueue(target_pid);
            } else {
                target.mailbox.push_back(value);
            }
            drop(procs);
            table.return_process(sender);
            scheduler.enqueue(sender_pid);
        }
        None => {
            drop(procs);
            sender.status =
                ProcessStatus::Failed(format!("send_message: unknown process {:?}", target_pid));
            table.return_process(sender);
        }
    }
}

fn handle_receive(table: &ProcessTable, scheduler: &dyn Scheduler, mut process: Process) {
    let pid = process.pid;
    if let Some(msg) = process.mailbox.pop_front() {
        process.vm.stack.pop();
        let val = deserialize(msg, &mut process.vm.heap);
        process.vm.push_value(val);
        table.return_process(process);
        scheduler.enqueue(pid);
    } else {
        process.status = ProcessStatus::Blocked(BlockReason::Receive);
        table.return_process(process);
    }
}

fn wake_waiters(table: &ProcessTable, scheduler: &dyn Scheduler, finished_pid: Pid) {
    let waiter_pids = table.waiters.lock().unwrap().remove(&finished_pid);
    let waiter_pids = match waiter_pids {
        Some(w) => w,
        None => return,
    };

    // Serialize result once
    let delivery = {
        let procs = table.processes.lock().unwrap();
        let child = &procs[&finished_pid];
        match &child.status {
            ProcessStatus::Done => {
                let val = child.vm.stack.last().copied().unwrap_or(Value::Unit);
                Ok(serialize(val, &child.vm.heap).unwrap_or(SendableValue::Unit))
            }
            ProcessStatus::Failed(msg) => Err(msg.clone()),
            _ => Err("child not finished".into()),
        }
    };

    for waiter_pid in waiter_pids {
        let mut procs = table.processes.lock().unwrap();
        if let Some(waiter) = procs.get_mut(&waiter_pid) {
            match &delivery {
                Ok(sendable) => {
                    waiter.vm.stack.pop();
                    let val = deserialize(sendable.clone(), &mut waiter.vm.heap);
                    waiter.vm.push_value(val);
                    waiter.status = ProcessStatus::Runnable;
                    drop(procs);
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
        // Spawn 10 children that each compute a value
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
