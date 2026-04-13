//! Runtime: single-threaded scheduler loop for running multiple hiko processes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::process::{Pid, Process, ProcessStatus};
use crate::scheduler::{FifoScheduler, Scheduler};
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
                    let process = self.processes.get_mut(&pid).unwrap();
                    process.status = ProcessStatus::Done;
                    self.scheduler.remove(pid);
                    self.wake_waiters(pid);
                }
                RunResult::Yielded => {
                    self.scheduler.enqueue(pid);
                }
                RunResult::Failed(msg) => {
                    let process = self.processes.get_mut(&pid).unwrap();
                    process.status = ProcessStatus::Failed(msg);
                    self.scheduler.remove(pid);
                    self.wake_waiters(pid);
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

    /// Try to dequeue a runnable process without blocking.
    /// Returns None when no runnable processes remain.
    fn try_dequeue(&self) -> Option<Pid> {
        // For single-threaded runtime: check if any process is runnable
        for process in self.processes.values() {
            if process.is_runnable() {
                // Drain from the scheduler
                // Since we're single-threaded, we can check directly
            }
        }
        // Use a non-blocking approach: if scheduler has something, take it
        // For the FIFO scheduler, we need to check if the queue has items
        // without blocking. Use a simple approach:
        let has_runnable = self.processes.values().any(|p| p.is_runnable());
        if !has_runnable {
            return None;
        }
        // The scheduler was already notified via enqueue.
        // For single-threaded use, directly pop from processes.
        self.processes
            .values()
            .find(|p| p.is_runnable())
            .map(|p| p.pid)
    }

    /// Wake all processes waiting for the given pid to finish.
    fn wake_waiters(&mut self, finished_pid: Pid) {
        if let Some(waiter_pids) = self.waiters.remove(&finished_pid) {
            for waiter in waiter_pids {
                if let Some(process) = self.processes.get_mut(&waiter) {
                    process.status = ProcessStatus::Runnable;
                    self.scheduler.enqueue(waiter);
                }
            }
        }
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
        assert_eq!(result, RunResult::Yielded);

        // Continue running with more fuel — should complete
        let result = vm.run_slice(1_000_000);
        assert_eq!(result, RunResult::Done);
    }

    #[test]
    fn test_run_slice_completes() {
        let program = compile("val x = 1 + 1");
        let mut vm = VM::new(program);
        let result = vm.run_slice(1000);
        assert_eq!(result, RunResult::Done);
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
}
