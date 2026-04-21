//! Runtime: single-threaded scheduler loop for running multiple hiko processes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::process::{AwaitKind, BlockReason, FiberJoinError, Pid, Process, ProcessFailure, ProcessStatus};
use crate::scheduler::{FifoScheduler, Scheduler};
use crate::sendable::{SendableValue, serialize};
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
    /// Processes waiting for any child in a set to finish: child_pid → [waiter_pids]
    any_waiters: HashMap<Pid, Vec<Pid>>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            next_pid: AtomicU64::new(1),
            processes: HashMap::new(),
            scheduler: Box::new(FifoScheduler::new(1000)),
            waiters: HashMap::new(),
            any_waiters: HashMap::new(),
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
            any_waiters: HashMap::new(),
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
                                process.status = ProcessStatus::Failed(ProcessFailure::runtime(
                                    format!("child result not sendable: {e}"),
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
                RunResult::Failed(failure) => {
                    let process = self.processes.get_mut(&pid).unwrap();
                    process.status = ProcessStatus::Failed(failure);
                    self.scheduler.remove(pid);
                    self.wake_and_deliver_results(pid);
                }
                RunResult::Spawn {
                    proto_idx,
                    captures,
                } => {
                    match self.handle_spawn(pid, proto_idx, captures) {
                        Ok(child_pid) => {
                            // Resume parent with child pid
                            let process = self.processes.get_mut(&pid).unwrap();
                            // Replace the Unit placeholder with the actual Pid
                            process.vm.stack.pop();
                            process.vm.push_value(Value::Pid(child_pid.0));
                            self.scheduler.enqueue(pid);
                        }
                        Err(failure) => {
                            let process = self.processes.get_mut(&pid).unwrap();
                            process.status = ProcessStatus::Failed(failure);
                            self.scheduler.remove(pid);
                            self.wake_and_deliver_results(pid);
                        }
                    }
                }
                RunResult::Await(child_pid_val) => {
                    let child_pid = Pid(child_pid_val);
                    self.handle_await(pid, child_pid, AwaitKind::Raw);
                }
                RunResult::AwaitResult(child_pid_val) => {
                    let child_pid = Pid(child_pid_val);
                    self.handle_await(pid, child_pid, AwaitKind::Result);
                }
                RunResult::Cancel(child_pid_val) => {
                    let child_pid = Pid(child_pid_val);
                    self.handle_cancel(pid, child_pid);
                }
                RunResult::WaitAny(child_pid_vals) => {
                    let child_pids = child_pid_vals.into_iter().map(Pid).collect();
                    self.handle_wait_any(pid, child_pids);
                }
                RunResult::Io(_req) => {
                    let process = self.processes.get_mut(&pid).unwrap();
                    process.status = ProcessStatus::Failed(ProcessFailure::runtime(
                        "async I/O requires ThreadedRuntime",
                    ));
                }
                RunResult::Cancelled => {
                    let process = self.processes.get_mut(&pid).unwrap();
                    process.status = ProcessStatus::Failed(ProcessFailure::Cancelled);
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
    ) -> Result<Pid, ProcessFailure> {
        let child_pid = self.new_pid();
        let parent = self.processes.get(&parent_pid).unwrap();
        let child_vm =
            crate::runtime_ops::create_child_vm_from_parent(&parent.vm, proto_idx, captures)?;
        let child = Process::new(child_pid, child_vm, Some(parent_pid));
        self.processes.insert(child_pid, child);
        self.scheduler.enqueue(child_pid);
        Ok(child_pid)
    }

    /// Handle an await request: block parent or resume with result.
    fn handle_await(&mut self, parent_pid: Pid, child_pid: Pid, await_kind: AwaitKind) {
        // Extract child state as an owned value to avoid borrow conflicts
        enum ChildState {
            Done,
            Failed(ProcessFailure),
            Running,
            NotFound,
            NotChild,
        }

        if self
            .processes
            .get(&parent_pid)
            .is_some_and(|parent| parent.consumed_children.contains(&child_pid))
        {
            let parent = self.processes.get_mut(&parent_pid).unwrap();
            match await_kind {
                AwaitKind::Raw => {
                    parent.status = ProcessStatus::Failed(ProcessFailure::runtime(
                        "await: child result already consumed",
                    ));
                    self.scheduler.remove(parent_pid);
                }
                AwaitKind::Result => {
                    match crate::runtime_ops::deliver_join_result_to_parent(
                        &mut parent.vm,
                        Err(FiberJoinError::AlreadyJoined),
                    ) {
                        Ok(()) => {
                            parent.status = ProcessStatus::Runnable;
                            self.scheduler.enqueue(parent_pid);
                        }
                        Err(failure) => {
                            parent.status = ProcessStatus::Failed(failure);
                            self.scheduler.remove(parent_pid);
                        }
                    }
                }
            }
            return;
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
                        match await_kind {
                            AwaitKind::Raw => {
                                parent.status = ProcessStatus::Failed(ProcessFailure::runtime(
                                    "await: child result already consumed",
                                ));
                                self.scheduler.remove(parent_pid);
                            }
                            AwaitKind::Result => {
                                match crate::runtime_ops::deliver_join_result_to_parent(
                                    &mut parent.vm,
                                    Err(FiberJoinError::AlreadyJoined),
                                ) {
                                    Ok(()) => {
                                        parent.status = ProcessStatus::Runnable;
                                        parent.consumed_children.insert(child_pid);
                                        self.scheduler.enqueue(parent_pid);
                                    }
                                    Err(failure) => {
                                        parent.status = ProcessStatus::Failed(failure);
                                        self.scheduler.remove(parent_pid);
                                    }
                                }
                            }
                        }
                        return;
                    }
                };
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                let delivery = match await_kind {
                    AwaitKind::Raw => {
                        crate::runtime_ops::deliver_result_to_parent(&mut parent.vm, sendable)
                    }
                    AwaitKind::Result => crate::runtime_ops::deliver_join_result_to_parent(
                        &mut parent.vm,
                        Ok(sendable),
                    ),
                };
                match delivery {
                    Ok(()) => {
                        parent.status = ProcessStatus::Runnable;
                        parent.consumed_children.insert(child_pid);
                        self.scheduler.enqueue(parent_pid);
                        self.processes.remove(&child_pid);
                        self.waiters.remove(&child_pid);
                    }
                    Err(failure) => {
                        parent.status = ProcessStatus::Failed(failure);
                        self.scheduler.remove(parent_pid);
                        self.processes.remove(&child_pid);
                        self.waiters.remove(&child_pid);
                    }
                }
            }
            ChildState::Failed(failure) => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                match await_kind {
                    AwaitKind::Raw => {
                        parent.status = ProcessStatus::Failed(ProcessFailure::ChildProcessFailed(
                            Box::new(failure),
                        ));
                        self.scheduler.remove(parent_pid);
                    }
                    AwaitKind::Result => {
                        match crate::runtime_ops::deliver_join_result_to_parent(
                            &mut parent.vm,
                            Err(FiberJoinError::from_process_failure(failure)),
                        ) {
                            Ok(()) => {
                                parent.status = ProcessStatus::Runnable;
                                parent.consumed_children.insert(child_pid);
                                self.scheduler.enqueue(parent_pid);
                            }
                            Err(delivery_failure) => {
                                parent.status = ProcessStatus::Failed(delivery_failure);
                                self.scheduler.remove(parent_pid);
                            }
                        }
                    }
                }
                self.processes.remove(&child_pid);
                self.waiters.remove(&child_pid);
            }
            ChildState::Running => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Blocked(BlockReason::Await {
                    child: child_pid,
                    kind: await_kind,
                });
                self.waiters.entry(child_pid).or_default().push(parent_pid);
            }
            ChildState::NotFound => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                    "await: unknown process {:?}",
                    child_pid
                )));
                self.scheduler.remove(parent_pid);
            }
            ChildState::NotChild => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                    "await: process {:?} is not a child of {:?}",
                    child_pid, parent_pid
                )));
                self.scheduler.remove(parent_pid);
            }
        }
    }

    fn handle_cancel(&mut self, parent_pid: Pid, child_pid: Pid) {
        enum CancelState {
            Running,
            Done,
            Failed,
            NotFound,
            NotChild,
        }

        let child_state = match self.processes.get(&child_pid) {
            None => CancelState::NotFound,
            Some(child) if child.parent != Some(parent_pid) => CancelState::NotChild,
            Some(child) => match child.status {
                ProcessStatus::Done => CancelState::Done,
                ProcessStatus::Failed(_) => CancelState::Failed,
                _ => CancelState::Running,
            },
        };

        match child_state {
            CancelState::Running => {
                self.cancel_process(child_pid);
                if let Some(parent) = self.processes.get_mut(&parent_pid) {
                    self.scheduler.enqueue(parent_pid);
                    parent.status = ProcessStatus::Runnable;
                }
            }
            CancelState::Done | CancelState::Failed => {
                if let Some(parent) = self.processes.get_mut(&parent_pid) {
                    parent.vm.stack.pop();
                    parent.vm.push_value(Value::Unit);
                    parent.status = ProcessStatus::Runnable;
                    self.scheduler.enqueue(parent_pid);
                }
            }
            CancelState::NotFound => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                    "cancel: unknown process {:?}",
                    child_pid
                )));
                self.scheduler.remove(parent_pid);
            }
            CancelState::NotChild => {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                    "cancel: process {:?} is not a child of {:?}",
                    child_pid, parent_pid
                )));
                self.scheduler.remove(parent_pid);
            }
        }
    }

    fn handle_wait_any(&mut self, parent_pid: Pid, child_pids: Vec<Pid>) {
        let child_pids = dedup_pids(child_pids);
        if child_pids.is_empty() {
            let parent = self.processes.get_mut(&parent_pid).unwrap();
            parent.status = ProcessStatus::Failed(ProcessFailure::runtime(
                "wait_any: expected non-empty pid list",
            ));
            self.scheduler.remove(parent_pid);
            return;
        }

        let mut first_finished = None;
        for &child_pid in &child_pids {
            let Some(child) = self.processes.get(&child_pid) else {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                    "wait_any: unknown process {:?}",
                    child_pid
                )));
                self.scheduler.remove(parent_pid);
                return;
            };
            if child.parent != Some(parent_pid) {
                let parent = self.processes.get_mut(&parent_pid).unwrap();
                parent.status = ProcessStatus::Failed(ProcessFailure::runtime(format!(
                    "wait_any: process {:?} is not a child of {:?}",
                    child_pid, parent_pid
                )));
                self.scheduler.remove(parent_pid);
                return;
            }
            if matches!(child.status, ProcessStatus::Done | ProcessStatus::Failed(_)) {
                first_finished = Some(child_pid);
                break;
            }
        }

        if let Some(winner) = first_finished {
            let parent = self.processes.get_mut(&parent_pid).unwrap();
            crate::runtime_ops::deliver_pid_to_parent(&mut parent.vm, winner);
            parent.status = ProcessStatus::Runnable;
            self.scheduler.enqueue(parent_pid);
            return;
        }

        let parent = self.processes.get_mut(&parent_pid).unwrap();
        parent.status = ProcessStatus::Blocked(BlockReason::WaitAny(child_pids.clone()));
        for child_pid in child_pids {
            self.any_waiters
                .entry(child_pid)
                .or_default()
                .push(parent_pid);
        }
    }

    fn cancel_process(&mut self, pid: Pid) {
        let block_reason = match self.processes.get_mut(&pid) {
            Some(process) => {
                let block_reason = match &process.status {
                    ProcessStatus::Blocked(reason) => Some(reason.clone()),
                    _ => None,
                };
                if block_reason.is_some() {
                    process.status = ProcessStatus::Failed(ProcessFailure::Cancelled);
                } else {
                    process.cancelled = true;
                }
                block_reason
            }
            None => return,
        };

        if let Some(reason) = block_reason {
            self.scheduler.remove(pid);
            match reason {
                BlockReason::Await { child, .. } => {
                    remove_waiter(&mut self.waiters, child, pid);
                }
                BlockReason::WaitAny(child_pids) => {
                    self.remove_wait_any_registration(pid, &child_pids);
                }
                BlockReason::Io(_) => {}
            }
            self.wake_any_waiters(pid);
            self.wake_join_waiters(pid);
        }
    }

    fn remove_wait_any_registration(&mut self, parent_pid: Pid, child_pids: &[Pid]) {
        for &child_pid in child_pids {
            remove_waiter(&mut self.any_waiters, child_pid, parent_pid);
        }
    }

    fn wake_any_waiters(&mut self, finished_pid: Pid) {
        let waiter_pids = match self.any_waiters.remove(&finished_pid) {
            Some(waiters) => waiters,
            None => return,
        };

        for waiter_pid in waiter_pids {
            let child_pids = match self.processes.get(&waiter_pid) {
                Some(process) => match &process.status {
                    ProcessStatus::Blocked(BlockReason::WaitAny(child_pids)) => child_pids.clone(),
                    _ => continue,
                },
                None => continue,
            };

            self.remove_wait_any_registration(waiter_pid, &child_pids);

            if let Some(process) = self.processes.get_mut(&waiter_pid) {
                crate::runtime_ops::deliver_pid_to_parent(&mut process.vm, finished_pid);
                process.status = ProcessStatus::Runnable;
                self.scheduler.enqueue(waiter_pid);
            }
        }
    }

    /// When a child finishes, wake blocked parents and give them the result.
    fn wake_and_deliver_results(&mut self, finished_pid: Pid) {
        self.wake_any_waiters(finished_pid);
        self.wake_join_waiters(finished_pid);
    }

    fn wake_join_waiters(&mut self, finished_pid: Pid) {
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
            _ => Err(ProcessFailure::runtime("child not finished")),
        };

        for waiter in waiter_pids {
            let await_kind = match self.processes.get(&waiter) {
                Some(process) => match &process.status {
                    ProcessStatus::Blocked(BlockReason::Await { child, kind })
                        if *child == finished_pid =>
                    {
                        *kind
                    }
                    _ => continue,
                },
                None => continue,
            };

            if let Some(process) = self.processes.get_mut(&waiter) {
                match (await_kind, &delivery) {
                    (AwaitKind::Raw, Ok(sendable)) => {
                        match crate::runtime_ops::deliver_result_to_parent(
                            &mut process.vm,
                            sendable.clone(),
                        ) {
                            Ok(()) => {
                                process.status = ProcessStatus::Runnable;
                                process.consumed_children.insert(finished_pid);
                                self.scheduler.enqueue(waiter);
                            }
                            Err(failure) => {
                                process.status = ProcessStatus::Failed(failure);
                            }
                        }
                    }
                    (AwaitKind::Raw, Err(msg)) => {
                        process.status = ProcessStatus::Failed(ProcessFailure::ChildProcessFailed(
                            Box::new(msg.clone()),
                        ));
                    }
                    (AwaitKind::Result, Ok(sendable)) => {
                        match crate::runtime_ops::deliver_join_result_to_parent(
                            &mut process.vm,
                            Ok(sendable.clone()),
                        ) {
                            Ok(()) => {
                                process.status = ProcessStatus::Runnable;
                                process.consumed_children.insert(finished_pid);
                                self.scheduler.enqueue(waiter);
                            }
                            Err(failure) => {
                                process.status = ProcessStatus::Failed(failure);
                            }
                        }
                    }
                    (AwaitKind::Result, Err(msg)) => {
                        match crate::runtime_ops::deliver_join_result_to_parent(
                            &mut process.vm,
                            Err(FiberJoinError::from_process_failure(msg.clone())),
                        ) {
                            Ok(()) => {
                                process.status = ProcessStatus::Runnable;
                                process.consumed_children.insert(finished_pid);
                                self.scheduler.enqueue(waiter);
                            }
                            Err(failure) => {
                                process.status = ProcessStatus::Failed(failure);
                            }
                        }
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

fn dedup_pids(child_pids: Vec<Pid>) -> Vec<Pid> {
    let mut unique = Vec::with_capacity(child_pids.len());
    for pid in child_pids {
        if !unique.contains(&pid) {
            unique.push(pid);
        }
    }
    unique
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
    use hiko_common::blake3_hex;
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn compile(source: &str) -> CompiledProgram {
        let tokens = Lexer::new(source, 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile(program).unwrap();
        compiled
    }

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hiko-runtime-{prefix}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn spawn_single_response_server(
        expected_path: &'static str,
        body: String,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let request_line = String::from_utf8_lossy(&request);
            let path = request_line
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let (status, response_body) = if path == expected_path {
                ("200 OK", body)
            } else {
                ("404 Not Found", "not found".to_string())
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            stream.flush().expect("flush response");
        });
        (format!("http://{addr}{expected_path}"), handle)
    }

    fn compile_example_with_std_lockfile(path: &Path) -> CompiledProgram {
        let source = std::fs::read_to_string(path).expect("read example source");
        let list_module_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../libraries/Std-v0.1.0/modules/List.hml");
        let list_module_source =
            std::fs::read_to_string(&list_module_path).expect("read Std.List source");
        let list_module_hash = blake3_hex(list_module_source.as_bytes());
        let (list_url, server) =
            spawn_single_response_server("/modules/List.hml", list_module_source);
        let base_url = list_url.trim_end_matches("/modules/List.hml").to_string();

        let project_dir = unique_temp_dir("spawn-stress-example");
        let entry_path = project_dir.join(
            path.file_name()
                .and_then(|name| name.to_str())
                .expect("example filename"),
        );
        std::fs::write(
            project_dir.join("hiko.lock.toml"),
            format!(
                "schema_version = 1\n\n[packages.Std]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Std.modules]\nList = \"blake3:{list_module_hash}\"\n"
            ),
        )
        .expect("write lockfile");
        std::fs::write(&entry_path, source).expect("write copied example");

        let tokens = Lexer::new(
            &std::fs::read_to_string(&entry_path).expect("read copied example"),
            0,
        )
        .tokenize()
        .unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile_file(program, &entry_path).unwrap();

        server.join().expect("join module server");
        std::fs::remove_dir_all(&project_dir).ok();
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
    fn test_spawn_stress_example() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/spawn_stress.hml");
        let program = compile_example_with_std_lockfile(&path);
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root(program);
        runtime.run_to_completion().unwrap();

        let output = runtime.get_output(pid).join("");
        assert!(output.contains("spawn_stress ok: 6000 children"));
    }

    #[test]
    fn test_spawn_stress_example_verifies() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/spawn_stress.hml");
        let program = compile_example_with_std_lockfile(&path);
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
