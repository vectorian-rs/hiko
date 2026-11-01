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
        captures: Vec<Arc<crate::sendable::SendableValue>>,
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
        captures: Vec<Arc<crate::sendable::SendableValue>>,
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

    #[cfg(all(test, loom))]
    pub(crate) fn cancellation_requested(&self) -> bool {
        self.cancelled
    }

    /// Enable or disable runtime-managed async I/O suspension.
    pub fn set_async_io(&mut self, enabled: bool) {
        self.async_io = enabled;
    }

    /// Create a child VM with the same builtins and capabilities as this VM.
    ///
    /// Cost model:
    /// - `O(1)` to share the compiled program, the builtin table and the
    ///   filesystem policy (with its open directories) through `Arc`
    /// - `O(e + h)` to clone the exec and HTTP allowlists
    /// - `O(1)` for fresh execution state (`Heap`, stack, frames, handlers)
    /// - `O(1)` to share the parent's top-level snapshot (built by the spawn
    ///   builtin; see `child_globals`)
    /// - no Hiko heap objects are allocated here; heap objects appear later
    ///   when captures are deserialized and the closure call is installed
    ///
    /// See `docs/architecture/performance.md` for measured numbers.
    pub fn create_child(&self) -> VM {
        let mut child = Self::from_verified_program(self.get_program(), self.startup_error.clone());
        child.builtins = self.builtins.clone();
        child.inherit_globals(self.spawn_snapshot.clone());
        // `exec` permissions are immutable capability data, so a child can
        // inherit the parent's resolved allowlist directly instead of
        // re-resolving every command through PATH on each spawn.
        child.exec_allowed = self.exec_allowed.clone();
        child.exec_allowed_paths = self.exec_allowed_paths.clone();
        child.exec_allowed_resolution_errors = self.exec_allowed_resolution_errors.clone();
        child.set_io_limits(self.heap.io_limits());
        child.set_max_message_bytes(self.heap.max_message_bytes());
        child.heap.share_fs_policy(&self.heap);
        #[cfg(feature = "builtin-regex")]
        child.heap.share_regex_cache(&self.heap);
        child.set_http_allowed_hosts(self.heap.http_allowed_hosts().to_vec());
        child.set_http_allowed_hosts_by_builtin(self.heap.http_allowed_hosts_by_builtin().clone());
        #[cfg(feature = "builtin-github")]
        {
            child.set_github_issue_allowed_repos(self.heap.github_issue_allowed_repos().clone());
        }
        #[cfg(feature = "builtin-aws-config")]
        {
            child.set_aws_sso_profiles(self.heap.aws_sso_profiles().to_vec());
            child.set_aws_allow_instance_profile(self.heap.aws_allow_instance_profile());
        }
        if let Some(max_heap) = self.heap.max_objects() {
            child.set_max_heap(max_heap);
        }
        // Memory, I/O, host work, host resources, fuel and the spawn limits
        // are budgets for the whole process tree: the child draws from the
        // parent's counters.
        child.heap.share_tree_budgets(&self.heap);
        child.fuel_budget = self.fuel_budget.clone();
        child.process_budgets = self.process_budgets.clone();
        if self.output.is_some() {
            child.enable_output_capture();
        }
        if let Some(sink) = &self.output_sink {
            child.set_output_sink(sink.clone());
        }
        // The active `fuel` field is a slice-local dispatch counter installed
        // by `run_slice`, so the child does not copy it.
        child.set_async_io(self.async_io);
        child
    }

    /// Count a spawn against the tree's process limits. The returned slot
    /// belongs to the child VM; see [`VM::set_live_process_slot`].
    pub fn admit_child(&self) -> Result<Option<crate::budget::BudgetSlot>, String> {
        self.process_budgets.admit_child()
    }

    /// Hold a live-process slot until this VM is dropped.
    pub fn set_live_process_slot(&mut self, slot: Option<crate::budget::BudgetSlot>) {
        self.live_process_slot = slot;
    }

    /// Get the compiled program for child-process creation.
    pub fn get_program(&self) -> CompiledProgram {
        CompiledProgram {
            main: self.main_chunk.clone(),
            main_n_locals: self.main_n_locals,
            functions: self.protos.clone(),
            effects: self.effect_metadata.clone(),
            globals: self.global_names.clone(),
        }
    }

    /// Run until the program halts, fails, exhausts configured fuel, or emits a runtime request.
    ///
    /// When no fuel limit is configured, this call is non-preemptive: the dispatch loop has no
    /// reduction boundary and can run indefinitely for non-terminating programs. Embedders that
    /// need bounded execution or scheduler fairness should call [`VM::run_slice`] instead.
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
        if self.cancelled {
            return RunResult::Cancelled;
        }
        // Reserve this slice's fuel from the tree's budget up front and
        // return what the slice did not use, so concurrent processes in one
        // tree can never spend more than the budget between them.
        let effective = match &self.fuel_budget {
            Some(budget) => match budget.reserve_up_to(reductions) {
                0 => return RunResult::Failed(ProcessFailure::FuelExhausted),
                reserved => reserved,
            },
            None => reductions,
        };
        self.fuel = Some(effective);

        if self.frames.is_empty() {
            self.frames.push(CallFrame {
                proto_idx: usize::MAX,
                ip: 0,
                base: 0,
                captures: Arc::from([]),
            });
        }

        let result = self.dispatch();

        if let Some(budget) = &self.fuel_budget {
            budget.refund(self.fuel.unwrap_or(0));
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
