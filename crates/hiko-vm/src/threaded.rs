//! Multi-threaded runtime: N worker threads executing hiko processes in parallel.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;

use crate::io_backend::{IoBackend, IoToken, ThreadPoolIoBackend};
use crate::process::{BlockReason, Pid, Process, ProcessFailure, ProcessStatus, Scope, ScopeId};
use crate::runtime_ops::deliver_result_to_parent;
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::value::Value;
use crate::vm::{RunResult, VM};
use hiko_compile::chunk::CompiledProgram;

/// Thread-safe process table using DashMap for fine-grained locking.
struct ProcessTable {
    processes: DashMap<Pid, Process>,
    waiters: DashMap<Pid, Vec<Pid>>,
    io_waiters: DashMap<IoToken, Pid>,
    #[allow(dead_code)] // scaffolding for structured concurrency
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
        self.processes
            .iter()
            .any(|e| matches!(e.status, ProcessStatus::Blocked(BlockReason::Await(_))))
            && !self.processes.iter().any(|e| e.is_runnable())
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
        vm.async_io = true;
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
                        crate::io_backend::IoResult::Ok(sv) => {
                            let val = crate::sendable::deserialize(sv, &mut process.vm.heap);
                            process.vm.stack.pop(); // remove placeholder
                            process.vm.push_value(val);
                            process.status = ProcessStatus::Runnable;
                            drop(process);
                            self.scheduler.enqueue(pid);
                        }
                        crate::io_backend::IoResult::Err(msg) => {
                            process.status = ProcessStatus::Failed(ProcessFailure::runtime(msg));
                            drop(process);
                            self.scheduler.remove(pid);
                            wake_waiters(&self.table, &*self.scheduler, pid);
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
                                matches!(e.status, ProcessStatus::Blocked(BlockReason::Await(_)))
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

fn worker_loop(
    table: &ProcessTable,
    scheduler: &dyn Scheduler,
    next_pid: &AtomicU64,
    next_io_token: &AtomicU64,
    active_workers: &std::sync::atomic::AtomicUsize,
    io_backend: &dyn IoBackend,
) {
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

        let result = process.vm.run_slice(reductions);

        match result {
            RunResult::Done => {
                if process.parent.is_some() {
                    // Child results cross a process boundary when awaited, so they must be
                    // sendable. Root processes run in-place and may finish with local values.
                    let val = process.vm.stack.last().copied().unwrap_or(Value::Unit);
                    match crate::sendable::serialize(val, &process.vm.heap) {
                        Ok(sv) => process.result = Some(sv),
                        Err(e) => {
                            process.status = ProcessStatus::Failed(ProcessFailure::runtime(
                                format!("child result not sendable: {e}"),
                            ));
                            table.return_process(process);
                            scheduler.remove(pid);
                            wake_waiters(table, scheduler, pid);
                            continue;
                        }
                    }
                }
                process.status = ProcessStatus::Done;
                table.return_process(process);
                scheduler.remove(pid);
                wake_waiters(table, scheduler, pid);
            }
            RunResult::Yielded => {
                table.return_process(process);
                scheduler.enqueue(pid);
            }
            RunResult::Failed(failure) => {
                process.status = ProcessStatus::Failed(failure);
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
                process.vm.push_value(Value::Pid(child_pid.0));
                table.return_process(process);
                scheduler.enqueue(pid);
            }
            RunResult::Await(child_pid_val) => {
                let child_pid = Pid(child_pid_val);
                handle_await(table, scheduler, process, child_pid);
            }
            RunResult::Io(request) => {
                // Register I/O with backend, block process
                let token = IoToken(next_io_token.fetch_add(1, Ordering::Relaxed));
                process.status = ProcessStatus::Blocked(BlockReason::Io(token));
                table.return_process(process);
                table.io_waiters.insert(token, pid);
                io_backend.register(token, request);
            }
            RunResult::Cancelled => {
                process.status = ProcessStatus::Failed(ProcessFailure::Cancelled);
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

    // Use get_mut to allow take() on result for single-consumption
    match table.processes.get_mut(&child_pid) {
        None => {
            parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                "await: unknown process {:?}",
                child_pid
            )));
            table.return_process(parent);
        }
        Some(child) if child.parent != Some(parent_pid) => {
            parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                "await: process {:?} is not a child of {:?}",
                child_pid, parent_pid
            )));
            table.return_process(parent);
        }
        Some(mut child) => match &child.status {
            ProcessStatus::Done => {
                // Consume result once (take), matching single-threaded semantics
                match child.result.take() {
                    Some(sv) => {
                        drop(child);
                        deliver_result_to_parent(&mut parent.vm, sv);
                        table.processes.remove(&child_pid);
                        table.waiters.remove(&child_pid);
                        table.return_process(parent);
                        scheduler.enqueue(parent_pid);
                    }
                    None => {
                        drop(child);
                        parent.status = ProcessStatus::Failed(ProcessFailure::runtime(
                            "await: result already consumed",
                        ));
                        table.return_process(parent);
                    }
                }
            }
            ProcessStatus::Failed(msg) => {
                let failure = msg.clone();
                drop(child);
                parent.status =
                    ProcessStatus::Failed(ProcessFailure::ChildProcessFailed(Box::new(failure)));
                table.processes.remove(&child_pid);
                table.waiters.remove(&child_pid);
                table.return_process(parent);
            }
            _ => {
                drop(child);
                parent.status = ProcessStatus::Blocked(BlockReason::Await(child_pid));
                table.return_process(parent);
                table.waiters.entry(child_pid).or_default().push(parent_pid);
            }
        },
    }
}

fn wake_waiters(table: &ProcessTable, scheduler: &dyn Scheduler, finished_pid: Pid) {
    let waiter_pids = match table.waiters.remove(&finished_pid) {
        Some((_, w)) => w,
        None => return,
    };

    // Read the finished process's status and pre-serialized result
    let delivery = match table.processes.get(&finished_pid) {
        Some(p) => match &p.status {
            ProcessStatus::Done => match &p.result {
                Some(sv) => Ok(sv.clone()),
                None => Err(ProcessFailure::runtime("child result already consumed")),
            },
            ProcessStatus::Failed(msg) => Err(msg.clone()),
            _ => Err(ProcessFailure::runtime("child not finished")),
        },
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
                    waiter.status = ProcessStatus::Failed(ProcessFailure::ChildProcessFailed(
                        Box::new(msg.clone()),
                    ));
                }
            }
        }
    }

    table.processes.remove(&finished_pid);
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn compile(source: &str) -> CompiledProgram {
        let tokens = Lexer::new(source, 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile(program).unwrap();
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
        vm.async_io = true;
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
}
