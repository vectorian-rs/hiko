//! Multi-threaded runtime: N worker threads executing hiko processes in parallel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;

use crate::process::{BlockReason, Pid, Process, ProcessStatus};
use crate::runtime_ops::{
    self, ChildState, check_child_state, create_child_vm, deliver_message,
    deliver_result_to_parent, prepare_delivery,
};
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::sendable::SendableValue;
use crate::value::Value;
use crate::vm::{RunResult, VM};
use hiko_compile::chunk::CompiledProgram;

/// Thread-safe process table using DashMap for fine-grained locking.
/// Each entry is independently lockable — workers accessing different
/// processes don't block each other.
struct ProcessTable {
    processes: DashMap<Pid, Process>,
    waiters: Mutex<HashMap<Pid, Vec<Pid>>>,
}

impl ProcessTable {
    fn new() -> Self {
        Self {
            processes: DashMap::new(),
            waiters: Mutex::new(HashMap::new()),
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
                let program = process.vm.get_program();
                let child_vm = create_child_vm(program, proto_idx, captures);
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

    // Self-send: put message in own mailbox
    if target_pid == sender_pid {
        sender.mailbox.push_back(value);
        table.return_process(sender);
        scheduler.enqueue(sender_pid);
        return;
    }

    // Return sender first so the table has it
    table.return_process(sender);

    match table.processes.get_mut(&target_pid) {
        Some(mut target) => {
            if matches!(target.status, ProcessStatus::Blocked(BlockReason::Receive)) {
                target.status = ProcessStatus::Runnable;
                deliver_message(&mut target.vm, value);
                drop(target);
                scheduler.enqueue(target_pid);
            } else {
                target.mailbox.push_back(value);
            }
            scheduler.enqueue(sender_pid);
        }
        None => {
            // Target doesn't exist — fail sender
            if let Some(mut sender) = table.processes.get_mut(&sender_pid) {
                sender.status = ProcessStatus::Failed(format!(
                    "send_message: unknown process {:?}",
                    target_pid
                ));
            }
        }
    }
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
    let waiter_pids = table.waiters.lock().unwrap().remove(&finished_pid);
    let waiter_pids = match waiter_pids {
        Some(w) => w,
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
