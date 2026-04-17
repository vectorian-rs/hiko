//! Runtime: single-threaded scheduler loop for running multiple hiko processes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::process::{BlockReason, Pid, Process, ProcessStatus};
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::sendable::{SendableValue, deserialize, serialize};
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

impl Default for Runtime {
    fn default() -> Self {
        Self {
            next_pid: AtomicU64::new(1),
            processes: HashMap::new(),
            scheduler: Box::new(FifoScheduler::new(1000)),
            waiters: HashMap::new(),
        }
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
        while let Some(pid) = self.try_dequeue() {
            let reductions = self.scheduler.reductions(pid);

            let result = {
                let process = self.processes.get_mut(&pid).expect("process not in table");
                process.vm.run_slice(reductions)
            };

            match result {
                RunResult::Done => {
                    let process = self.processes.get_mut(&pid).unwrap();
                    if process.parent.is_some() {
                        // Child results cross a process boundary when awaited, so they must be
                        // sendable. Root processes run in-place and may finish with local values.
                        let val = process.vm.stack.last().copied().unwrap_or(Value::Unit);
                        match serialize(val, &process.vm.heap) {
                            Ok(sv) => process.result = Some(sv),
                            Err(e) => {
                                process.status = ProcessStatus::Failed(format!(
                                    "child result not sendable: {e}"
                                ));
                                self.scheduler.remove(pid);
                                self.wake_and_deliver_results(pid);
                                continue;
                            }
                        }
                    }
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
                    process.vm.push_value(Value::Pid(child_pid.0));
                    self.scheduler.enqueue(pid);
                }
                RunResult::Await(child_pid_val) => {
                    let child_pid = Pid(child_pid_val);
                    self.handle_await(pid, child_pid);
                }
                RunResult::Io(_req) => {
                    let process = self.processes.get_mut(&pid).unwrap();
                    process.status =
                        ProcessStatus::Failed("async I/O requires ThreadedRuntime".into());
                }
                RunResult::Cancelled => {
                    let process = self.processes.get_mut(&pid).unwrap();
                    process.status = ProcessStatus::Failed("cancelled".into());
                    self.scheduler.remove(pid);
                    self.wake_and_deliver_results(pid);
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
        captures: Vec<SendableValue>,
    ) -> Pid {
        let child_pid = self.new_pid();
        let parent = self.processes.get(&parent_pid).unwrap();
        let child_vm =
            crate::runtime_ops::create_child_vm_from_parent(&parent.vm, proto_idx, captures);
        let child = Process::new(child_pid, child_vm, Some(parent_pid));
        self.processes.insert(child_pid, child);
        self.scheduler.enqueue(child_pid);
        child_pid
    }

    /// Handle an await request: block parent or resume with result.
    fn handle_await(&mut self, parent_pid: Pid, child_pid: Pid) {
        // Extract child state as an owned value to avoid borrow conflicts
        enum ChildState {
            Done,
            Failed(String),
            Running,
            NotFound,
            NotChild,
        }

        let child_state = match self.processes.get(&child_pid) {
            None => ChildState::NotFound,
            Some(child) => {
                // Parent-only await: only the spawning parent may await
                if child.parent != Some(parent_pid) {
                    ChildState::NotChild
                } else {
                    match &child.status {
                        ProcessStatus::Done => ChildState::Done,
                        ProcessStatus::Failed(msg) => ChildState::Failed(msg.clone()),
                        _ => ChildState::Running,
                    }
                }
            }
        };

        match child_state {
            ChildState::Done => {
                // Take result (consume once — second await will fail)
                let sendable = match self
                    .processes
                    .get_mut(&child_pid)
                    .and_then(|c| c.result.take())
                {
                    Some(sv) => sv,
                    None => {
                        let parent = self.processes.get_mut(&parent_pid).unwrap();
                        parent.status =
                            ProcessStatus::Failed("await: child result already consumed".into());
                        return;
                    }
                };
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.vm.stack.pop();
                let val = deserialize(sendable, &mut parent.vm.heap);
                parent.vm.push_value(val);
                self.scheduler.enqueue(parent_pid);
                self.processes.remove(&child_pid);
                self.waiters.remove(&child_pid);
            }
            ChildState::Failed(msg) => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Failed(format!("child process failed: {msg}"));
                self.scheduler.remove(parent_pid);
                self.processes.remove(&child_pid);
                self.waiters.remove(&child_pid);
            }
            ChildState::Running => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Blocked(BlockReason::Await(child_pid));
                self.waiters.entry(child_pid).or_default().push(parent_pid);
            }
            ChildState::NotFound => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status =
                    ProcessStatus::Failed(format!("await: unknown process {:?}", child_pid));
                self.scheduler.remove(parent_pid);
            }
            ChildState::NotChild => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Failed(format!(
                    "await: process {:?} is not a child of {:?}",
                    child_pid, parent_pid
                ));
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

        self.processes.remove(&finished_pid);
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
    use std::path::Path;

    fn compile(source: &str) -> CompiledProgram {
        let tokens = Lexer::new(source, 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile(program).unwrap();
        compiled
    }

    fn compile_example(path: &Path) -> CompiledProgram {
        let source = std::fs::read_to_string(path).unwrap();
        let tokens = Lexer::new(&source, 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile_file(program, path).unwrap();
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
