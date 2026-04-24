//! Runtime-facing VM protocol and state transitions.
//!
//! This module owns the narrow seam between the bytecode interpreter and the
//! process runtimes. The runtime can run a slice, observe the resulting
//! transition, inject a resume value, or request cancellation; it does not need
//! direct knowledge of the interpreter's internal frame/handler machinery.

use super::*;

/// Outcome of a `run_slice` call.
///
/// `RunResult` is the only runtime-visible execution transition. A process is
/// runnable when the runtime calls `run_slice()`. After the slice completes,
/// the runtime decides what to do next based solely on this enum:
///
/// - `Done`: execution reached `Halt` or returned from the outermost frame
/// - `Yielded`: the slice budget was exhausted without a runtime boundary
/// - `Failed`: execution hit a runtime error
/// - `Spawn`/`Await`/`AwaitResult`/`Cancel`/`WaitAny`: execution suspended at a
///   runtime-managed process boundary
/// - `Io`: execution suspended at a runtime-managed I/O boundary
/// - `Cancelled`: a previously requested cooperative cancellation was observed
#[derive(Debug)]
pub enum RunResult {
    /// Program completed normally.
    Done,
    /// Reduction budget exhausted; can be resumed.
    Yielded,
    /// Program failed with an error.
    Failed(ProcessFailure),
    /// Process requested to spawn a child.
    /// Contains `(proto_idx, serialized_captures)`.
    Spawn {
        proto_idx: usize,
        captures: Vec<crate::sendable::SendableValue>,
    },
    /// Process requested to await a child result.
    Await(u64),
    /// Process requested to await a child result as a `Result` value.
    AwaitResult(u64),
    /// Process requested to cooperatively cancel a child.
    Cancel(u64),
    /// Process requested to wait for any child in the set to complete.
    WaitAny(Vec<u64>),
    /// Process requested an async I/O operation.
    Io(crate::io_backend::IoRequest),
    /// Process was cancelled at a suspension point.
    Cancelled,
}

/// Request emitted by runtime-aware builtins during interpreter execution.
///
/// This is intentionally narrower than `VM` itself. Builtins can describe
/// *what* the runtime should do next, but they do not mutate process-table
/// state directly.
#[derive(Debug)]
pub enum RuntimeRequest {
    Spawn {
        proto_idx: usize,
        captures: Vec<crate::sendable::SendableValue>,
    },
    Await(u64),
    AwaitResult(u64),
    Cancel(u64),
    WaitAny(Vec<u64>),
    Io(crate::io_backend::IoRequest),
}

impl VM {
    /// Set up a fresh child process to call a closure body.
    ///
    /// This is the last step of process creation on the VM side. The caller is
    /// expected to deserialize captures first, then hand them to the child VM.
    ///
    /// Cost notes:
    /// - pushes one `Value::Unit` argument onto the stack
    /// - appends one `CallFrame`
    /// - allocates one `Arc<[Value]>` to own the captures slice
    pub fn setup_closure_call(&mut self, proto_idx: usize, captures: &[Value]) {
        self.stack.push(Value::Unit);
        self.frames.push(CallFrame {
            proto_idx,
            ip: 0,
            base: 0,
            captures: Arc::from(captures),
        });
    }

    /// Resume a blocked continuation with a runtime-supplied result.
    ///
    /// This is used by the threaded runtime after an async I/O completion.
    /// The transition is:
    ///
    /// 1. take the saved continuation root from `blocked_continuation`
    /// 2. restore the saved stack segment
    /// 3. restore saved frames relative to the new stack base
    /// 4. push the resumed result as the return value of `perform`
    pub fn resume_blocked(&mut self, result: Value) -> Result<(), RuntimeError> {
        if let Some(cont_ref) = self.blocked_continuation.take() {
            let (saved_frames, saved_stack) = match self.heap.get(cont_ref) {
                Ok(HeapObject::Continuation {
                    saved_frames,
                    saved_stack,
                    ..
                }) => (saved_frames.clone(), saved_stack.clone()),
                Ok(_) => {
                    return Err(RuntimeError {
                        message: "resume_blocked: expected continuation".into(),
                    });
                }
                Err(e) => {
                    return Err(RuntimeError {
                        message: format!("resume_blocked: {e}"),
                    });
                }
            };

            let stack_base = self.stack.len();
            self.stack.extend_from_slice(&saved_stack);

            let main_base = if self.frames.is_empty() {
                0
            } else {
                self.frames[0].base
            };

            self.frames.truncate(1);

            for (i, sf) in saved_frames.iter().enumerate() {
                let frame_base = if i == 0 {
                    main_base
                } else {
                    stack_base
                        .checked_add(sf.base_offset)
                        .ok_or_else(|| RuntimeError {
                            message: "resume_blocked: saved frame base overflow".into(),
                        })?
                };
                if i == 0 && !self.frames.is_empty() {
                    self.frames[0] = CallFrame {
                        proto_idx: sf.proto_idx,
                        ip: sf.ip,
                        base: frame_base,
                        captures: sf.captures.clone(),
                    };
                } else {
                    self.frames.push(CallFrame {
                        proto_idx: sf.proto_idx,
                        ip: sf.ip,
                        base: frame_base,
                        captures: sf.captures.clone(),
                    });
                }
            }

            self.stack.push(result);
        }
        Ok(())
    }

    /// Take ownership of the next pending runtime request.
    pub fn take_runtime_request(&mut self) -> Option<RuntimeRequest> {
        self.pending_runtime_request.take()
    }

    /// Mark this VM so the next slice exits with `RunResult::Cancelled`.
    ///
    /// Cancellation is deliberately owned by the VM. Runtimes can request
    /// cancellation, but the interpreter decides when it becomes observable.
    pub fn request_cancellation(&mut self) {
        self.cancelled = true;
    }

    /// Enable or disable runtime-managed async I/O suspension.
    pub fn set_async_io(&mut self, enabled: bool) {
        self.async_io = enabled;
    }

    /// Create a child VM with the same builtins and capabilities as this VM.
    ///
    /// This is the expensive part of process creation today. The child VM
    /// reuses immutable compiled program data via `Arc`, but it rebuilds its
    /// builtin/global tables and clones capability configuration.
    ///
    /// Cost model:
    /// - `O(b)` builtin/global registration where `b = builtins.len()`
    /// - `O(e + f + h)` capability cloning for exec/fs/http allowlists
    /// - `O(1)` for fresh execution state (`Heap`, stack, frames, handlers)
    /// - no Hiko heap objects are allocated here; heap objects appear later
    ///   when captures are deserialized and the closure call is installed
    ///
    /// The wall-clock cost is therefore dominated by builtin table rebuilds and
    /// capability cloning, not by compiled-program cloning.
    pub fn create_child(&self) -> VM {
        let mut child = Self::from_verified_program(self.get_program(), self.startup_error.clone());
        child.builtins.reserve(self.builtins.len());
        child.globals.reserve(self.globals.len());
        child.global_names.reserve(self.global_names.len());
        for entry in &self.builtins {
            child.register_builtin(entry.name.clone(), entry.func);
        }
        // `exec` permissions are immutable capability data, so a child can
        // inherit the parent's resolved allowlist directly instead of
        // re-resolving every command through PATH on each spawn.
        child.exec_allowed = self.exec_allowed.clone();
        child.exec_allowed_paths = self.exec_allowed_paths.clone();
        child.exec_allowed_resolution_errors = self.exec_allowed_resolution_errors.clone();
        child.exec_timeout = self.exec_timeout;
        child.set_fs_root(self.fs_root.clone());
        child.set_fs_builtin_folders(self.fs_builtin_folders.clone());
        child.set_http_allowed_hosts(self.http_allowed_hosts.clone());
        child.set_http_allowed_hosts_by_builtin(self.http_allowed_hosts_by_builtin.clone());
        if let Some(max_heap) = self.heap.max_objects() {
            child.set_max_heap(max_heap);
        }
        if let Some(max_memory_bytes) = self.heap.max_bytes() {
            child.set_max_memory_bytes(max_memory_bytes);
        }
        if let Some(max_io_bytes) = self.heap.max_io_bytes() {
            child.set_max_io_bytes(max_io_bytes);
        }
        if self.output.is_some() {
            child.enable_output_capture();
        }
        if let Some(sink) = &self.output_sink {
            child.set_output_sink(sink.clone());
        }
        if let Some(remaining) = self.max_fuel_remaining {
            child.max_fuel_remaining = Some(remaining);
        }
        child.set_async_io(self.async_io);
        child
    }

    /// Get the compiled program for child-process creation.
    pub fn get_program(&self) -> CompiledProgram {
        CompiledProgram {
            main: self.main_chunk.clone(),
            functions: self.protos.clone(),
            effects: self.effect_metadata.clone(),
        }
    }

    pub fn run(&mut self) -> Result<(), RuntimeError> {
        if let Some(message) = self.startup_error.as_ref() {
            return Err(RuntimeError {
                message: format!("program verification failed: {message}"),
            });
        }
        self.frames.push(CallFrame {
            proto_idx: usize::MAX,
            ip: 0,
            base: 0,
            captures: Arc::from([]),
        });
        self.dispatch()
    }

    /// Run one scheduling slice and report the resulting execution transition.
    ///
    /// Slice lifecycle:
    /// - validate startup state
    /// - cap the slice by the remaining persistent fuel budget
    /// - observe any previously requested cancellation
    /// - ensure the outermost frame exists
    /// - interpret until halt, failure, fuel exhaustion, or runtime request
    /// - translate any pending `RuntimeRequest` into a `RunResult`
    ///
    /// The runtime should treat `RunResult` as the authoritative state
    /// transition for the process.
    pub fn run_slice(&mut self, reductions: u64) -> RunResult {
        if let Some(message) = self.startup_error.as_ref() {
            return RunResult::Failed(ProcessFailure::runtime(format!(
                "program verification failed: {message}"
            )));
        }
        if let Some(ref remaining) = self.max_fuel_remaining
            && *remaining == 0
        {
            return RunResult::Failed(ProcessFailure::FuelExhausted);
        }
        let effective = match self.max_fuel_remaining {
            Some(remaining) => remaining.min(reductions),
            None => reductions,
        };
        self.fuel = Some(effective);

        if self.cancelled {
            return RunResult::Cancelled;
        }

        if self.frames.is_empty() {
            self.frames.push(CallFrame {
                proto_idx: usize::MAX,
                ip: 0,
                base: 0,
                captures: Arc::from([]),
            });
        }

        let result = self.dispatch();

        if let Some(ref mut remaining) = self.max_fuel_remaining {
            let consumed = effective.saturating_sub(self.fuel.unwrap_or(0));
            *remaining = remaining.saturating_sub(consumed);
        }
        self.fuel = None;

        if let Some(req) = self.pending_runtime_request.take() {
            self.gc_collect_at_boundary_if_needed();
            return match req {
                RuntimeRequest::Spawn {
                    proto_idx,
                    captures,
                } => RunResult::Spawn {
                    proto_idx,
                    captures,
                },
                RuntimeRequest::Await(pid) => RunResult::Await(pid),
                RuntimeRequest::AwaitResult(pid) => RunResult::AwaitResult(pid),
                RuntimeRequest::Cancel(pid) => RunResult::Cancel(pid),
                RuntimeRequest::WaitAny(pids) => RunResult::WaitAny(pids),
                RuntimeRequest::Io(req) => RunResult::Io(req),
            };
        }

        match result {
            Ok(()) => RunResult::Done,
            Err(e) if e.is_runtime_request() => RunResult::Yielded,
            Err(e) if e.is_fuel_exhausted() => {
                self.gc_collect_at_boundary_if_needed();
                RunResult::Yielded
            }
            Err(e) => RunResult::Failed(ProcessFailure::from_runtime_message(e.message)),
        }
    }
}
