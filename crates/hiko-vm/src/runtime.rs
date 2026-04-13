//! Runtime: single-threaded scheduler loop for running multiple hiko processes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::process::{BlockReason, Pid, Process, ProcessStatus};
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::sendable::{deserialize, serialize};
use crate::value::Value;
use crate::vm::{RunResult, VM};
use hiko_compile::chunk::CompiledProgram;

/// The hiko runtime — manages processes and scheduling.
pub struct Runtime {
    next_pid: AtomicU64,
    processes: HashMap<Pid, Process>,
    scheduler: Box<dyn Scheduler>,
    /// Processes waiting for another process to finish: child_pid → [waiter_pids]
    waiters: HashMap<Pid, Vec<Pid>>,
}

impl Runtime {
    /// Create a new runtime with the default FIFO scheduler.
    pub fn new() -> Self {
        Self {
            next_pid: AtomicU64::new(1),
            processes: HashMap::new(),
            scheduler: Box::new(FifoScheduler::new(1000)),
            waiters: HashMap::new(),
        }
    }

    /// Create a runtime with a custom scheduler.
    pub fn with_scheduler(scheduler: Box<dyn Scheduler>) -> Self {
        Self {
            next_pid: AtomicU64::new(1),
            processes: HashMap::new(),
            scheduler,
            waiters: HashMap::new(),
        }
    }

    /// Allocate a new process ID.
    fn new_pid(&self) -> Pid {
        Pid(self.next_pid.fetch_add(1, Ordering::Relaxed))
    }

    /// Spawn a root process from a compiled program.
    /// Returns the Pid.
    pub fn spawn_root(&mut self, program: CompiledProgram) -> Pid {
        let pid = self.new_pid();
        let vm = VM::new(program);
        let process = Process::new(pid, vm, None);
        self.processes.insert(pid, process);
        self.scheduler.enqueue(pid);
        pid
    }

    /// Run all processes to completion (single-threaded).
    /// Returns the root process's output lines.
    pub fn run_to_completion(&mut self) -> Result<Vec<String>, String> {
        loop {
            let pid = match self.try_dequeue() {
                Some(pid) => pid,
                None => break,
            };

            let reductions = self.scheduler.reductions(pid);

            let result = {
                let process = self.processes.get_mut(&pid).expect("process not in table");
                process.vm.run_slice(reductions)
            };

            match result {
                RunResult::Done => {
                    // Serialize the return value (top of stack)
                    let process = self.processes.get_mut(&pid).unwrap();
                    process.status = ProcessStatus::Done;
                    self.scheduler.remove(pid);
                    self.wake_and_deliver_results(pid);
                }
                RunResult::Yielded => {
                    self.scheduler.enqueue(pid);
                }
                RunResult::Failed(msg) => {
                    let process = self.processes.get_mut(&pid).unwrap();
                    process.status = ProcessStatus::Failed(msg);
                    self.scheduler.remove(pid);
                    self.wake_and_deliver_results(pid);
                }
                RunResult::Spawn {
                    proto_idx,
                    captures,
                } => {
                    let child_pid = self.handle_spawn(pid, proto_idx, captures);
                    // Resume parent with child pid
                    let process = self.processes.get_mut(&pid).unwrap();
                    // Replace the Unit placeholder with the actual Pid
                    process.vm.stack.pop();
                    process.vm.push_value(Value::Int(child_pid.0 as i64));
                    self.scheduler.enqueue(pid);
                }
                RunResult::Await(child_pid_val) => {
                    let child_pid = Pid(child_pid_val);
                    self.handle_await(pid, child_pid);
                }
            }
        }

        // Collect output from all processes (root first)
        let mut all_output = Vec::new();
        for process in self.processes.values() {
            all_output.extend(process.vm.get_output().iter().cloned());
        }
        Ok(all_output)
    }

    /// Handle a spawn request: create child process from closure prototype.
    fn handle_spawn(
        &mut self,
        parent_pid: Pid,
        proto_idx: usize,
        captures: Vec<crate::sendable::SendableValue>,
    ) -> Pid {
        let child_pid = self.new_pid();

        // Clone the parent's compiled program for the child
        let parent = self.processes.get(&parent_pid).unwrap();
        let program = parent.vm.get_program();

        // Create a child VM with all builtins
        let mut child_vm = VM::new(program);

        // Deserialize captures into child's heap
        let child_captures: Vec<Value> = captures
            .into_iter()
            .map(|v| deserialize(v, &mut child_vm.heap))
            .collect();

        // Set up the child VM to execute the closure's prototype
        // The closure takes Unit as argument (fn () => ...)
        child_vm.setup_closure_call(proto_idx, &child_captures);

        let child = Process::new(child_pid, child_vm, Some(parent_pid));
        self.processes.insert(child_pid, child);
        self.scheduler.enqueue(child_pid);
        child_pid
    }

    /// Handle an await request: block parent or resume with result.
    fn handle_await(&mut self, parent_pid: Pid, child_pid: Pid) {
        // Check child status without holding a mutable borrow
        let child_status = self.processes.get(&child_pid).map(|c| match &c.status {
            ProcessStatus::Done => "done",
            ProcessStatus::Failed(_) => "failed",
            _ => "running",
        });

        match child_status {
            Some("done") => {
                // Serialize child's return value
                let sendable = {
                    let child = self.processes.get(&child_pid).unwrap();
                    let child_val = child.vm.stack.last().copied().unwrap_or(Value::Unit);
                    serialize(child_val, &child.vm.heap)
                        .unwrap_or(crate::sendable::SendableValue::Unit)
                };
                // Deserialize into parent's heap
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.vm.stack.pop(); // remove Unit placeholder
                let val = deserialize(sendable, &mut parent.vm.heap);
                parent.vm.push_value(val);
                self.scheduler.enqueue(parent_pid);
            }
            Some("failed") => {
                let msg = match &self.processes[&child_pid].status {
                    ProcessStatus::Failed(m) => m.clone(),
                    _ => "unknown".into(),
                };
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Failed(format!("child process failed: {msg}"));
                self.scheduler.remove(parent_pid);
            }
            Some(_) => {
                // Child still running — block parent
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Blocked(BlockReason::Await(child_pid));
                self.waiters.entry(child_pid).or_default().push(parent_pid);
            }
            None => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status =
                    ProcessStatus::Failed(format!("await: unknown process {:?}", child_pid));
                self.scheduler.remove(parent_pid);
            }
        }
    }

    /// When a child finishes, wake blocked parents and give them the result.
    fn wake_and_deliver_results(&mut self, finished_pid: Pid) {
        let waiter_pids = match self.waiters.remove(&finished_pid) {
            Some(w) => w,
            None => return,
        };

        // Serialize the finished process's result once, before borrowing waiters
        let child = &self.processes[&finished_pid];
        let delivery = match &child.status {
            ProcessStatus::Done => {
                let val = child.vm.stack.last().copied().unwrap_or(Value::Unit);
                let sendable =
                    serialize(val, &child.vm.heap).unwrap_or(crate::sendable::SendableValue::Unit);
                Ok(sendable)
            }
            ProcessStatus::Failed(msg) => Err(msg.clone()),
            _ => Err("child not finished".into()),
        };

        for waiter in waiter_pids {
            if let Some(process) = self.processes.get_mut(&waiter) {
                match &delivery {
                    Ok(sendable) => {
                        process.vm.stack.pop();
                        let val = deserialize(sendable.clone(), &mut process.vm.heap);
                        process.vm.push_value(val);
                        process.status = ProcessStatus::Runnable;
                        self.scheduler.enqueue(waiter);
                    }
                    Err(msg) => {
                        process.status =
                            ProcessStatus::Failed(format!("child process failed: {msg}"));
                    }
                }
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

    /// Get the number of processes.
    pub fn process_count(&self) -> usize {
        self.processes.len()
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
            ProcessStatus::Failed(msg) => assert!(msg.contains("boom")),
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
}
