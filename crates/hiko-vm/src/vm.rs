use smallvec::smallvec;
use std::any::Any;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use hiko_compile::chunk::{Chunk, CompiledProgram, Constant, EffectMeta, FunctionProto};
use hiko_compile::op::Op;

use crate::heap::Heap;
use crate::process::ProcessFailure;
use crate::value::{
    BuiltinEntry, BuiltinFn, Fields, GcRef, HeapObject, SavedFrame, SavedHandler, Value,
};
use crate::verify::{VerificationError, verify_program};

/// Default hard limit on the VM value stack, measured in `Value` slots.
///
/// This is a fixed runtime guard today; unlike heap and fuel, it is not yet
/// configurable through `VMBuilder`.
pub const DEFAULT_MAX_STACK_SLOTS: usize = 64 * 1024;

/// Default hard limit on the VM call-frame stack.
///
/// This is a fixed runtime guard today; unlike heap and fuel, it is not yet
/// configurable through `VMBuilder`.
pub const DEFAULT_MAX_CALL_FRAMES: usize = 65_536;

/// Default hard limit on installed effect handlers.
pub const DEFAULT_MAX_HANDLER_FRAMES: usize = 65_536;

pub(crate) const TAG_NIL: u16 = 0;
pub(crate) const TAG_CONS: u16 = 1;

/// Outcome of a run_slice call.
#[derive(Debug)]
pub enum RunResult {
    /// Program completed normally.
    Done,
    /// Reduction budget exhausted; can be resumed.
    Yielded,
    /// Program failed with an error.
    Failed(ProcessFailure),
    /// Process requested to spawn a child.
    /// Contains (proto_idx, serialized_captures).
    Spawn {
        proto_idx: usize,
        captures: Vec<crate::sendable::SendableValue>,
    },
    /// Process requested to await a child result.
    Await(u64),
    /// Process requested to await a child result as a Result value.
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

#[derive(Debug)]
pub struct RuntimeError {
    pub message: String,
}

impl RuntimeError {
    pub fn is_fuel_exhausted(&self) -> bool {
        self.message.starts_with("fuel exhausted")
    }

    pub fn is_runtime_request(&self) -> bool {
        self.message == "runtime request"
    }

    pub fn is_heap_limit(&self) -> bool {
        self.message.starts_with("heap limit exceeded")
    }
}

fn heap_limit_panic_message(payload: &(dyn Any + Send)) -> Option<String> {
    let message = if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        message
    } else {
        return None;
    };

    message
        .starts_with("heap limit exceeded")
        .then(|| message.to_string())
}

struct ResolvedExec {
    display_command: String,
    resolved_path: PathBuf,
    args: Vec<String>,
}

struct CallFrame {
    proto_idx: usize,
    ip: usize,
    base: usize,
    captures: Arc<[Value]>,
}

struct HandlerFrame {
    call_frame_idx: usize,
    stack_base: usize,
    clauses: Vec<(u16, usize)>, // (effect_tag, absolute_ip)
    proto_idx: usize,
    captures: Arc<[Value]>,
}

fn extract_exec_command_and_args(heap: &Heap, arg: Value) -> Result<(String, Vec<String>), String> {
    let (v0, v1) = match arg {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("exec: expected (String, String list)".into()),
        },
        _ => return Err("exec: expected (String, String list)".into()),
    };

    let command = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("exec: expected String for command".into()),
        },
        _ => return Err("exec: expected String for command".into()),
    };

    let mut args = Vec::new();
    let mut cur = v1;
    loop {
        match cur {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Data { tag, .. } if *tag == TAG_NIL => break,
                HeapObject::Data { tag, fields } if *tag == TAG_CONS && fields.len() == 2 => {
                    match fields[0] {
                        Value::Heap(sr) => match heap.get(sr).map_err(|e| e.to_string())? {
                            HeapObject::String(s) => args.push(s.clone()),
                            _ => return Err("exec: args must be strings".into()),
                        },
                        _ => return Err("exec: args must be strings".into()),
                    }
                    cur = fields[1];
                }
                _ => return Err("exec: expected String list for args".into()),
            },
            _ => return Err("exec: expected String list for args".into()),
        }
    }

    Ok((command, args))
}

fn command_has_explicit_path(command: &str) -> bool {
    let path = Path::new(command);
    path.is_absolute() || path.components().count() > 1
}

fn canonicalize_exec_candidate(path: &Path) -> Option<PathBuf> {
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    std::fs::canonicalize(path).ok()
}

fn resolve_exec_command_path(command: &str) -> Result<PathBuf, String> {
    let path = Path::new(command);
    if command_has_explicit_path(command) {
        return canonicalize_exec_candidate(path)
            .ok_or_else(|| format!("cannot resolve executable '{}'", path.display()));
    }

    let path_env = std::env::var_os("PATH")
        .ok_or_else(|| format!("cannot resolve executable '{command}': PATH is not set"))?;

    for dir in std::env::split_paths(&path_env) {
        if let Some(resolved) = canonicalize_exec_candidate(&dir.join(command)) {
            return Ok(resolved);
        }

        #[cfg(windows)]
        {
            let pathext =
                std::env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
            for ext in pathext
                .to_string_lossy()
                .split(';')
                .filter(|s| !s.is_empty())
            {
                let trimmed = ext.trim_start_matches('.');
                if let Some(resolved) =
                    canonicalize_exec_candidate(&dir.join(format!("{command}.{trimmed}")))
                {
                    return Ok(resolved);
                }
            }
        }
    }

    Err(format!(
        "cannot resolve executable '{command}' from the current PATH"
    ))
}

pub trait OutputSink: Send + Sync {
    fn write(&self, text: &str) -> std::io::Result<()>;
}

#[derive(Default)]
pub struct StdoutOutputSink {
    lock: Mutex<()>,
}

impl OutputSink for StdoutOutputSink {
    fn write(&self, text: &str) -> std::io::Result<()> {
        use std::io::Write as _;

        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(text.as_bytes())?;
        stdout.flush()
    }
}

pub struct VM {
    pub heap: Heap,
    pub stack: Vec<Value>,
    frames: Vec<CallFrame>,
    globals: Vec<Value>,
    global_names: HashMap<String, usize>,
    protos: Arc<[FunctionProto]>,
    main_chunk: Arc<Chunk>,
    output: Option<Vec<String>>,
    builtins: Vec<BuiltinEntry>,
    handlers: Vec<HandlerFrame>,
    string_cache: HashMap<(usize, usize), GcRef>,
    fuel: Option<u64>,
    /// Persistent total fuel budget (from VMBuilder.max_fuel). Not reset per slice.
    max_fuel_remaining: Option<u64>,
    exec_allowed: Vec<String>,
    exec_allowed_paths: Vec<PathBuf>,
    exec_allowed_resolution_errors: Vec<String>,
    exec_timeout: u64,
    /// Filesystem root for path enforcement. Empty means no restriction.
    fs_root: String,
    /// Per-builtin filesystem folder allowlists.
    fs_builtin_folders: HashMap<String, Vec<String>>,
    /// Allowed HTTP hosts. Empty means no restriction.
    http_allowed_hosts: Vec<String>,
    /// Per-builtin HTTP host allowlists.
    http_allowed_hosts_by_builtin: HashMap<String, Vec<String>>,
    exec_builtin_id: Option<u16>,
    print_builtin_id: Option<u16>,
    println_builtin_id: Option<u16>,
    spawn_builtin_id: Option<u16>,
    await_builtin_id: Option<u16>,
    await_result_builtin_id: Option<u16>,
    cancel_builtin_id: Option<u16>,
    wait_any_builtin_id: Option<u16>,
    sleep_builtin_id: Option<u16>,
    http_get_builtin_id: Option<u16>,
    http_builtin_id: Option<u16>,
    http_json_builtin_id: Option<u16>,
    http_msgpack_builtin_id: Option<u16>,
    http_bytes_builtin_id: Option<u16>,
    read_file_builtin_id: Option<u16>,
    /// When true, I/O builtins suspend via RuntimeRequest::Io instead of blocking.
    pub async_io: bool,
    /// Pending runtime request from a process/runtime builtin.
    pending_runtime_request: Option<RuntimeRequest>,
    /// Effect metadata from compiled program (name → tag).
    pub effect_metadata: Arc<[EffectMeta]>,
    /// Saved continuation when blocked on runtime I/O. GC root.
    pub blocked_continuation: Option<GcRef>,
    /// Cooperative cancellation flag. Checked at suspension points.
    pub cancelled: bool,
    startup_error: Option<String>,
    output_sink: Option<Arc<dyn OutputSink>>,
}

/// A request from a builtin to the runtime.
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

pub(crate) fn values_equal(a: Value, b: Value, heap: &Heap) -> bool {
    let mut worklist = vec![(a, b)];

    while let Some((left, right)) = worklist.pop() {
        match (left, right) {
            (Value::Int(x), Value::Int(y)) if x == y => {}
            (Value::Pid(x), Value::Pid(y)) if x == y => {}
            (Value::Float(x), Value::Float(y)) if x == y => {}
            (Value::Bool(x), Value::Bool(y)) if x == y => {}
            (Value::Char(x), Value::Char(y)) if x == y => {}
            (Value::Unit, Value::Unit) => {}
            (Value::Heap(ra), Value::Heap(rb)) => {
                if ra == rb {
                    continue;
                }
                let (Ok(obj_a), Ok(obj_b)) = (heap.get(ra), heap.get(rb)) else {
                    return false;
                };
                match (obj_a, obj_b) {
                    (HeapObject::String(sa), HeapObject::String(sb)) if sa == sb => {}
                    (HeapObject::Tuple(ta), HeapObject::Tuple(tb)) => {
                        if ta.len() != tb.len() {
                            return false;
                        }
                        worklist.extend(ta.iter().copied().zip(tb.iter().copied()));
                    }
                    (
                        HeapObject::Data {
                            tag: ta,
                            fields: fa,
                        },
                        HeapObject::Data {
                            tag: tb,
                            fields: fb,
                        },
                    ) => {
                        if ta != tb || fa.len() != fb.len() {
                            return false;
                        }
                        worklist.extend(fa.iter().copied().zip(fb.iter().copied()));
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
    }

    true
}

// Compile-time assertion: VM must be Send for multi-threaded scheduling.
const _: () = {
    fn _assert_send<T: Send>() {}
    fn _check() {
        _assert_send::<VM>();
    }
};

impl VM {
    /// Create a VM with all builtins enabled (convenience for CLI use).
    pub fn new(program: CompiledProgram) -> Self {
        let mut vm = Self::from_program(program);
        vm.register_builtins();
        vm
    }

    /// Create a VM with all builtins enabled after validating the compiled program.
    pub fn try_new(program: CompiledProgram) -> Result<Self, VerificationError> {
        let mut vm = Self::try_from_program(program)?;
        vm.register_builtins();
        Ok(vm)
    }

    /// Create a VM with no builtins (for builder/embedding use).
    pub fn from_program(program: CompiledProgram) -> Self {
        match verify_program(&program) {
            Ok(()) => Self::from_verified_program(program, None),
            Err(err) => Self::from_verified_program(program, Some(err.to_string())),
        }
    }

    /// Create a VM with no builtins (for builder/embedding use) after validation.
    pub fn try_from_program(program: CompiledProgram) -> Result<Self, VerificationError> {
        verify_program(&program)?;
        Ok(Self::from_verified_program(program, None))
    }

    fn from_verified_program(program: CompiledProgram, startup_error: Option<String>) -> Self {
        VM {
            heap: Heap::new(),
            stack: Vec::with_capacity(32),
            frames: Vec::new(),
            globals: Vec::new(),
            global_names: HashMap::new(),
            effect_metadata: program.effects,
            protos: program.functions,
            main_chunk: program.main,
            output: None,
            builtins: Vec::new(),
            handlers: Vec::new(),
            string_cache: HashMap::new(),
            fuel: None,
            max_fuel_remaining: None,
            exec_allowed: Vec::new(),
            exec_allowed_paths: Vec::new(),
            exec_allowed_resolution_errors: Vec::new(),
            exec_timeout: 30,
            fs_root: String::new(),
            fs_builtin_folders: HashMap::new(),
            http_allowed_hosts: Vec::new(),
            http_allowed_hosts_by_builtin: HashMap::new(),
            exec_builtin_id: None,
            print_builtin_id: None,
            println_builtin_id: None,
            spawn_builtin_id: None,
            await_builtin_id: None,
            await_result_builtin_id: None,
            cancel_builtin_id: None,
            wait_any_builtin_id: None,
            sleep_builtin_id: None,
            http_get_builtin_id: None,
            http_builtin_id: None,
            http_json_builtin_id: None,
            http_msgpack_builtin_id: None,
            http_bytes_builtin_id: None,
            read_file_builtin_id: None,
            async_io: false,
            pending_runtime_request: None,
            blocked_continuation: None,
            cancelled: false,
            startup_error,
            output_sink: None,
        }
    }

    /// Set up the VM to execute a closure prototype with the given captures.
    /// Used by the runtime to start child processes.
    pub fn setup_closure_call(&mut self, proto_idx: usize, captures: &[Value]) {
        // Push Unit as the argument (fn () => ...)
        self.stack.push(Value::Unit);
        // Push a call frame for the closure
        self.frames.push(CallFrame {
            proto_idx,
            ip: 0,
            base: 0,
            captures: Arc::from(captures),
        });
    }

    /// Resume a blocked continuation with a result value.
    /// Used by the runtime after I/O completion.
    /// Restores the saved frames and stack, then pushes the result
    /// as the return value of `perform`.
    pub fn resume_blocked(&mut self, result: Value) -> Result<(), RuntimeError> {
        if let Some(cont_ref) = self.blocked_continuation.take() {
            // Get the saved continuation
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

            // Restore saved stack
            let stack_base = self.stack.len();
            self.stack.extend_from_slice(&saved_stack);

            // Restore saved frames
            // The first saved frame was the main frame at suspension time.
            // Use the current main frame's base as the anchor.
            let main_base = if self.frames.is_empty() {
                0
            } else {
                self.frames[0].base
            };

            // Remove all frames above the main frame
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
                    // Overwrite the main frame with the saved one
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

            // Push the result as the return value of `perform`
            self.stack.push(result);
        }
        Ok(())
    }

    /// Take a pending runtime request (if any).
    pub fn take_runtime_request(&mut self) -> Option<RuntimeRequest> {
        self.pending_runtime_request.take()
    }

    /// Push a value onto the stack (used by runtime to inject results).
    pub fn push_value(&mut self, value: Value) {
        self.stack.push(value);
    }

    /// Create a child VM with the same builtins and capabilities as this VM.
    pub fn create_child(&self) -> VM {
        let mut child = Self::from_verified_program(self.get_program(), self.startup_error.clone());
        // Copy all registered builtins
        for entry in &self.builtins {
            child.register_builtin(entry.name.clone(), entry.func);
        }
        // Copy capability settings
        child.set_exec_allowed(self.exec_allowed.clone());
        child.set_exec_timeout(self.exec_timeout);
        child.set_fs_root(self.fs_root.clone());
        child.set_fs_builtin_folders(self.fs_builtin_folders.clone());
        child.set_http_allowed_hosts(self.http_allowed_hosts.clone());
        child.set_http_allowed_hosts_by_builtin(self.http_allowed_hosts_by_builtin.clone());
        if let Some(max_heap) = self.heap.max_objects() {
            child.set_max_heap(max_heap);
        }
        if self.output.is_some() {
            child.enable_output_capture();
        }
        if let Some(sink) = &self.output_sink {
            child.set_output_sink(sink.clone());
        }
        // Copy fuel budget
        if let Some(remaining) = self.max_fuel_remaining {
            child.max_fuel_remaining = Some(remaining);
        }
        // Copy async I/O mode
        child.async_io = self.async_io;
        child
    }

    /// Get the compiled program (for cloning to child processes).
    pub fn get_program(&self) -> CompiledProgram {
        CompiledProgram {
            main: self.main_chunk.clone(),
            functions: self.protos.clone(),
            effects: self.effect_metadata.clone(),
        }
    }

    /// Look up an effect tag by name from compiled metadata.
    pub fn effect_tag_by_name(&self, name: &str) -> Option<u16> {
        self.effect_metadata
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.tag)
    }

    /// Set the filesystem root for path enforcement.
    pub fn set_fs_root(&mut self, root: String) {
        self.fs_root = root.clone();
        self.heap.fs_root = root;
    }

    /// Set per-builtin filesystem folder allowlists.
    pub fn set_fs_builtin_folders(&mut self, folders: HashMap<String, Vec<String>>) {
        self.fs_builtin_folders = folders.clone();
        self.heap.fs_builtin_folders = folders;
    }

    /// Set allowed HTTP hosts.
    pub fn set_http_allowed_hosts(&mut self, hosts: Vec<String>) {
        self.http_allowed_hosts = hosts.clone();
        self.heap.http_allowed_hosts = hosts;
    }

    /// Set per-builtin HTTP host allowlists.
    pub fn set_http_allowed_hosts_by_builtin(&mut self, hosts: HashMap<String, Vec<String>>) {
        self.http_allowed_hosts_by_builtin = hosts.clone();
        self.heap.http_allowed_hosts_by_builtin = hosts;
    }

    /// Check if a filesystem path is within the allowed root.
    /// Returns the canonicalized path or an error.
    pub fn check_fs_path(&self, path: &str) -> Result<std::path::PathBuf, String> {
        crate::heap::resolve_fs_path(&self.fs_root, path)
    }

    /// Check if a URL's host is in the allowed hosts list.
    pub fn check_http_host(&self, url: &str) -> Result<(), String> {
        self.heap.check_http_host(url)
    }

    /// Set the allowed commands for the exec builtin.
    pub fn set_exec_allowed(&mut self, allowed: Vec<String>) {
        let mut resolved = Vec::new();
        let mut errors = Vec::new();
        for command in &allowed {
            match resolve_exec_command_path(command) {
                Ok(path) => resolved.push(path),
                Err(err) => errors.push(format!("{command}: {err}")),
            }
        }
        self.exec_allowed = allowed;
        self.exec_allowed_paths = resolved;
        self.exec_allowed_resolution_errors = errors;
    }

    /// Set the timeout for exec calls (in seconds).
    pub fn set_exec_timeout(&mut self, timeout: u64) {
        self.exec_timeout = timeout;
    }

    /// Register a single builtin function by name.
    pub fn register_builtin(&mut self, name: impl Into<Arc<str>>, func: BuiltinFn) {
        let name: Arc<str> = name.into();
        let idx = self.builtins.len() as u16;
        self.builtins.push(BuiltinEntry {
            name: name.clone(),
            func,
        });
        let slot = self.global_slot(name.to_string());
        self.globals[slot] = Value::Builtin(idx);
        match name.as_ref() {
            "print" => self.print_builtin_id = Some(idx),
            "println" => self.println_builtin_id = Some(idx),
            "exec" => self.exec_builtin_id = Some(idx),
            "spawn" => self.spawn_builtin_id = Some(idx),
            "await_process" => self.await_builtin_id = Some(idx),
            "await_process_result" => self.await_result_builtin_id = Some(idx),
            "cancel" => self.cancel_builtin_id = Some(idx),
            "wait_any" => self.wait_any_builtin_id = Some(idx),
            "sleep" => self.sleep_builtin_id = Some(idx),
            "http_get" => self.http_get_builtin_id = Some(idx),
            "http" => self.http_builtin_id = Some(idx),
            "http_json" => self.http_json_builtin_id = Some(idx),
            "http_msgpack" => self.http_msgpack_builtin_id = Some(idx),
            "http_bytes" => self.http_bytes_builtin_id = Some(idx),
            "read_file" => self.read_file_builtin_id = Some(idx),
            _ => {}
        }
    }

    /// Register a builtin with an owned name string.
    pub fn register_builtin_owned(&mut self, name: String, func: BuiltinFn) {
        self.register_builtin(name, func);
    }

    /// Set the maximum heap size (in number of objects).
    pub fn set_max_heap(&mut self, max: usize) {
        self.heap.set_max_objects(max);
    }

    /// Set the fuel limit (max opcode executions).
    /// This sets both the per-run fuel and the persistent budget.
    pub fn set_fuel(&mut self, fuel: u64) {
        self.fuel = Some(fuel);
        self.max_fuel_remaining = Some(fuel);
    }

    // ── Heap helpers ─────────────────────────────────────────────────

    fn heap_get(&self, r: GcRef) -> Result<&HeapObject, RuntimeError> {
        self.heap
            .get(r)
            .map_err(|e| RuntimeError { message: e.into() })
    }

    fn alloc(&mut self, obj: HeapObject) -> Value {
        if self.heap.should_collect() {
            let mut extra_roots = Vec::new();
            obj.for_each_gc_ref(|r| extra_roots.push(r));
            self.gc_collect_with_extra_roots(extra_roots);
        }
        Value::Heap(self.heap.alloc(obj))
    }

    fn alloc_string(&mut self, s: String) -> Value {
        self.alloc(HeapObject::String(s))
    }

    fn dispatch_catching_heap_limit(&mut self) -> Result<(), RuntimeError> {
        match panic::catch_unwind(AssertUnwindSafe(|| self.dispatch())) {
            Ok(result) => result,
            Err(payload) => {
                if let Some(message) = heap_limit_panic_message(payload.as_ref()) {
                    Err(RuntimeError { message })
                } else {
                    panic::resume_unwind(payload);
                }
            }
        }
    }

    fn checked_relative_ip(
        &self,
        proto_idx: usize,
        base_after_operand: usize,
        offset: i16,
        what: &str,
    ) -> Result<usize, RuntimeError> {
        let target = base_after_operand
            .checked_add_signed(offset as isize)
            .ok_or_else(|| RuntimeError {
                message: format!("{what}: instruction pointer overflow"),
            })?;
        if target > self.chunk_for(proto_idx).code.len() {
            return Err(RuntimeError {
                message: format!("{what}: relative jump target {target} lands outside chunk"),
            });
        }
        Ok(target)
    }

    fn capture_value(
        &self,
        frame_idx: usize,
        is_local: bool,
        index: usize,
    ) -> Result<Value, RuntimeError> {
        if is_local {
            let base = self.frames[frame_idx].base;
            let slot = base.checked_add(index).ok_or_else(|| RuntimeError {
                message: "MakeClosure: local capture index overflow".into(),
            })?;
            self.stack.get(slot).copied().ok_or_else(|| RuntimeError {
                message: format!("MakeClosure: local capture index {index} out of bounds"),
            })
        } else {
            self.frames[frame_idx]
                .captures
                .get(index)
                .copied()
                .ok_or_else(|| RuntimeError {
                    message: format!("MakeClosure: upvalue index {index} out of bounds"),
                })
        }
    }

    fn gc_collect_with_extra_roots(&mut self, extra_roots: impl IntoIterator<Item = GcRef>) {
        let roots = self
            .stack
            .iter()
            .chain(self.frames.iter().flat_map(|f| f.captures.iter()))
            .chain(self.globals.iter())
            .chain(self.handlers.iter().flat_map(|h| h.captures.iter()))
            .filter_map(|v| match v {
                Value::Heap(r) => Some(*r),
                _ => None,
            })
            .chain(self.string_cache.values().copied())
            .chain(self.blocked_continuation.iter().copied())
            .chain(extra_roots);
        self.heap.collect(roots);
    }

    fn gc_collect_at_boundary_if_needed(&mut self) {
        if self.heap.should_collect_at_boundary() {
            self.gc_collect_with_extra_roots(std::iter::empty());
        }
    }

    /// Format a value for display (print/println).
    fn display_value(&self, v: &Value) -> String {
        match v {
            Value::Builtin(id) => {
                format!("<builtin:{}>", self.builtins[*id as usize].name.as_ref())
            }
            Value::Pid(pid) => format!("<pid {pid}>"),
            Value::Heap(r) => match self.heap.get(*r) {
                Ok(HeapObject::String(s)) => s.clone(),
                Ok(HeapObject::Tuple(elems)) => {
                    let parts: Vec<String> = elems.iter().map(|e| self.display_value(e)).collect();
                    format!("({})", parts.join(", "))
                }
                Ok(HeapObject::Data { tag, fields }) => {
                    if *tag == TAG_NIL && fields.is_empty() {
                        "[]".to_string()
                    } else if *tag == TAG_CONS && fields.len() == 2 {
                        // Iterative list printing to avoid stack overflow
                        let mut parts = vec![self.display_value(&fields[0])];
                        let mut tail = fields[1];
                        loop {
                            match tail {
                                Value::Heap(r2) => match self.heap.get(r2) {
                                    Ok(HeapObject::Data { tag: t, fields: f })
                                        if *t == TAG_NIL && f.is_empty() =>
                                    {
                                        break;
                                    }
                                    Ok(HeapObject::Data { tag: t, fields: f })
                                        if *t == TAG_CONS && f.len() == 2 =>
                                    {
                                        parts.push(self.display_value(&f[0]));
                                        tail = f[1];
                                    }
                                    _ => {
                                        parts.push(self.display_value(&tail));
                                        break;
                                    }
                                },
                                _ => {
                                    parts.push(self.display_value(&tail));
                                    break;
                                }
                            }
                        }
                        format!("[{}]", parts.join(", "))
                    } else {
                        format!("Data({tag})")
                    }
                }
                Ok(HeapObject::Bytes(b)) => format!("<bytes:{}>", b.len()),
                Ok(HeapObject::Rng { .. }) => "<rng>".to_string(),
                Ok(HeapObject::Closure { .. }) => "<fn>".to_string(),
                Ok(HeapObject::Continuation { .. }) => "<continuation>".to_string(),
                Err(_) => "<dangling ref>".to_string(),
            },
            other => other.to_string(),
        }
    }

    // ── Builtins ─────────────────────────────────────────────────────

    fn register_builtins(&mut self) {
        for (name, func) in crate::builtins::builtin_entries() {
            self.register_builtin(name, func);
        }
    }

    fn global_slot(&mut self, name: String) -> usize {
        if let Some(&slot) = self.global_names.get(&name) {
            slot
        } else {
            let slot = self.globals.len();
            self.globals.push(Value::Unit);
            self.global_names.insert(name, slot);
            slot
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
        self.dispatch_catching_heap_limit()
    }

    /// Run for up to `reductions` opcodes, then yield.
    /// Respects any existing fuel limit (takes the minimum).
    /// Returns the outcome: Done, Yielded, or Failed.
    pub fn run_slice(&mut self, reductions: u64) -> RunResult {
        if let Some(message) = self.startup_error.as_ref() {
            return RunResult::Failed(ProcessFailure::runtime(format!(
                "program verification failed: {message}"
            )));
        }
        // Check persistent fuel budget
        if let Some(ref remaining) = self.max_fuel_remaining
            && *remaining == 0
        {
            return RunResult::Failed(ProcessFailure::FuelExhausted);
        }
        // Use the minimum of slice reductions and persistent budget
        let effective = match self.max_fuel_remaining {
            Some(remaining) => remaining.min(reductions),
            None => reductions,
        };
        self.fuel = Some(effective);

        // Cancellation check before execution
        if self.cancelled {
            return RunResult::Cancelled;
        }

        // If no frames, this is a fresh start — push the main frame
        if self.frames.is_empty() {
            self.frames.push(CallFrame {
                proto_idx: usize::MAX,
                ip: 0,
                base: 0,
                captures: Arc::from([]),
            });
        }

        let result = self.dispatch_catching_heap_limit();

        // Update persistent fuel budget: deduct consumed reductions
        if let Some(ref mut remaining) = self.max_fuel_remaining {
            let consumed = effective.saturating_sub(self.fuel.unwrap_or(0));
            *remaining = remaining.saturating_sub(consumed);
        }
        // Clear per-slice fuel for next slice
        self.fuel = None;

        // Check for pending runtime request (process/runtime builtins)
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
            Err(e) if e.is_runtime_request() => {
                // Should have been caught by the check above — this is a fallback
                RunResult::Yielded
            }
            Err(e) if e.is_fuel_exhausted() => {
                self.gc_collect_at_boundary_if_needed();
                RunResult::Yielded
            }
            Err(e) => RunResult::Failed(ProcessFailure::from_runtime_message(e.message)),
        }
    }

    pub fn get_global(&self, name: &str) -> Option<&Value> {
        self.global_names.get(name).map(|&slot| &self.globals[slot])
    }

    pub fn get_output(&self) -> &[String] {
        self.output.as_deref().unwrap_or(&[])
    }

    pub fn enable_output_capture(&mut self) {
        if self.output.is_none() {
            self.output = Some(Vec::new());
        }
    }

    /// Inject stdin content for embedders running the VM in-process.
    pub fn set_stdin_override(&mut self, input: String) {
        self.heap.set_stdin_override(input);
    }

    pub fn disable_output_capture(&mut self) {
        self.output = None;
    }

    pub fn set_output_sink(&mut self, sink: Arc<dyn OutputSink>) {
        self.output_sink = Some(sink);
    }

    pub fn clear_output_sink(&mut self) {
        self.output_sink = None;
    }

    pub fn heap_live_count(&self) -> usize {
        self.heap.live_count()
    }

    pub fn error_span(&self) -> Option<hiko_syntax::span::Span> {
        let frame = self.frames.last()?;
        let chunk = self.chunk_for(frame.proto_idx);
        chunk.span_at(frame.ip.saturating_sub(1))
    }

    fn chunk_for(&self, proto_idx: usize) -> &Chunk {
        if proto_idx == usize::MAX {
            &self.main_chunk
        } else {
            &self.protos[proto_idx].chunk
        }
    }

    fn read_const_string(&self, proto_idx: usize, idx: usize) -> &str {
        match &self.chunk_for(proto_idx).constants[idx] {
            Constant::String(s) => s,
            _ => panic!("expected string constant"),
        }
    }

    // ── Builtin call ─────────────────────────────────────────────────

    fn call_builtin(
        &mut self,
        builtin_id: u16,
        callee_pos: usize,
        arity: usize,
    ) -> Result<(), RuntimeError> {
        let first_arg = self.stack[callee_pos + 1];

        // Spawn: extract closure, serialize captures, signal runtime
        if self.spawn_builtin_id == Some(builtin_id) {
            let closure_val = self.stack[callee_pos + 1];
            match closure_val {
                Value::Heap(r) => match self.heap.get(r) {
                    Ok(HeapObject::Closure {
                        proto_idx,
                        captures,
                    }) => {
                        let mut serialized = Vec::new();
                        for &v in captures.iter() {
                            serialized.push(crate::sendable::serialize(v, &self.heap).map_err(
                                |e| RuntimeError {
                                    message: format!("spawn: {e}"),
                                },
                            )?);
                        }
                        self.pending_runtime_request = Some(RuntimeRequest::Spawn {
                            proto_idx: *proto_idx,
                            captures: serialized,
                        });
                        self.stack.truncate(callee_pos);
                        // Push a placeholder — runtime will replace with Pid
                        self.push(Value::Unit)?;
                        return Err(RuntimeError {
                            message: "runtime request".into(),
                        });
                    }
                    _ => {
                        return Err(RuntimeError {
                            message: "spawn: expected a function".into(),
                        });
                    }
                },
                _ => {
                    return Err(RuntimeError {
                        message: "spawn: expected a function".into(),
                    });
                }
            }
        }

        // Await: signal runtime to block until child completes
        if self.await_builtin_id == Some(builtin_id) {
            let pid_val = self.stack[callee_pos + 1];
            match pid_val {
                Value::Pid(pid) => {
                    self.pending_runtime_request = Some(RuntimeRequest::Await(pid));
                    self.stack.truncate(callee_pos);
                    self.push(Value::Unit)?;
                    return Err(RuntimeError {
                        message: "runtime request".into(),
                    });
                }
                _ => {
                    return Err(RuntimeError {
                        message: "await_process: expected Pid".into(),
                    });
                }
            }
        }

        // AwaitResult: signal runtime to block until child completes and return a Result-shaped value
        if self.await_result_builtin_id == Some(builtin_id) {
            let pid_val = self.stack[callee_pos + 1];
            match pid_val {
                Value::Pid(pid) => {
                    self.pending_runtime_request = Some(RuntimeRequest::AwaitResult(pid));
                    self.stack.truncate(callee_pos);
                    self.push(Value::Unit)?;
                    return Err(RuntimeError {
                        message: "runtime request".into(),
                    });
                }
                _ => {
                    return Err(RuntimeError {
                        message: "await_process_result: expected Pid".into(),
                    });
                }
            }
        }

        // Cancel: signal runtime to cooperatively cancel a child
        if self.cancel_builtin_id == Some(builtin_id) {
            let pid_val = self.stack[callee_pos + 1];
            match pid_val {
                Value::Pid(pid) => {
                    self.pending_runtime_request = Some(RuntimeRequest::Cancel(pid));
                    self.stack.truncate(callee_pos);
                    self.push(Value::Unit)?;
                    return Err(RuntimeError {
                        message: "runtime request".into(),
                    });
                }
                _ => {
                    return Err(RuntimeError {
                        message: "cancel: expected Pid".into(),
                    });
                }
            }
        }

        // WaitAny: block until any child in the set finishes, then resume with its Pid
        if self.wait_any_builtin_id == Some(builtin_id) {
            let pids = crate::builtins::extract_pid_list_arg(
                &self.stack[callee_pos + 1..callee_pos + 1 + arity],
                &self.heap,
                "wait_any",
            )
            .map_err(|message| RuntimeError { message })?;
            self.pending_runtime_request = Some(RuntimeRequest::WaitAny(pids));
            self.stack.truncate(callee_pos);
            self.push(Value::Unit)?;
            return Err(RuntimeError {
                message: "runtime request".into(),
            });
        }

        // I/O builtins: in async mode, suspend instead of blocking
        if self.async_io {
            let io_request = if self.sleep_builtin_id == Some(builtin_id) {
                let ms = match self.stack[callee_pos + 1] {
                    Value::Int(ms) if ms >= 0 => ms as u64,
                    _ => {
                        return Err(RuntimeError {
                            message: "sleep: expected non-negative Int (milliseconds)".into(),
                        });
                    }
                };
                Some(crate::io_backend::IoRequest::Sleep(
                    std::time::Duration::from_millis(ms),
                ))
            } else if self.http_get_builtin_id == Some(builtin_id) {
                let url = crate::builtins::extract_string_arg(
                    &self.stack[callee_pos + 1..callee_pos + 1 + arity],
                    &self.heap,
                    "http_get",
                )
                .map_err(|msg| RuntimeError { message: msg })?;
                self.heap
                    .check_http_host_for("http_get", &url)
                    .map_err(|e| RuntimeError {
                        message: format!("http_get: {e}"),
                    })?;
                Some(crate::io_backend::IoRequest::HttpGet { url })
            } else if let Some(format) = self.match_http_builtin(builtin_id) {
                let args = &self.stack[callee_pos + 1..callee_pos + 1 + arity];
                let (method, url, headers, body) =
                    crate::builtins::extract_http_args(args, &self.heap, "http")
                        .map_err(|msg| RuntimeError { message: msg })?;
                let builtin_name = self.builtins[builtin_id as usize].name.as_ref();
                self.heap
                    .check_http_host_for(builtin_name, &url)
                    .map_err(|e| RuntimeError {
                        message: format!("{builtin_name}: {e}"),
                    })?;
                Some(crate::io_backend::IoRequest::Http {
                    method,
                    url,
                    headers,
                    body,
                    format,
                })
            } else if self.read_file_builtin_id == Some(builtin_id) {
                let path = crate::builtins::extract_string_arg(
                    &self.stack[callee_pos + 1..callee_pos + 1 + arity],
                    &self.heap,
                    "read_file",
                )
                .map_err(|msg| RuntimeError { message: msg })?;
                let checked = self
                    .heap
                    .check_fs_path_for("read_file", &path)
                    .map_err(|e| RuntimeError {
                        message: format!("read_file: {e}"),
                    })?;
                Some(crate::io_backend::IoRequest::ReadFile {
                    path: checked.to_string_lossy().to_string(),
                })
            } else {
                None
            };
            if let Some(request) = io_request {
                self.pending_runtime_request = Some(RuntimeRequest::Io(request));
                self.stack.truncate(callee_pos);
                self.push(Value::Unit)?;
                return Err(RuntimeError {
                    message: "runtime request".into(),
                });
            }
        }

        // Exec is fully intercepted: allowlist check + timeout
        if self.exec_builtin_id == Some(builtin_id) {
            let exec_arg = self.stack[callee_pos + 1];
            let prepared = self.prepare_exec(exec_arg)?;
            let result = self
                .run_exec(prepared)
                .map_err(|msg| RuntimeError { message: msg })?;
            self.stack.truncate(callee_pos);
            self.push(result)?;
            return Ok(());
        }

        let func = self.builtins[builtin_id as usize].func;
        let args = &self.stack[callee_pos + 1..callee_pos + 1 + arity];
        let result = func(args, &mut self.heap).map_err(|msg| RuntimeError { message: msg })?;
        self.stack.truncate(callee_pos);
        let is_print = self.print_builtin_id == Some(builtin_id);
        let is_println = self.println_builtin_id == Some(builtin_id);
        if is_print || is_println {
            let displayed = if is_println {
                format!("{}\n", self.display_value(&first_arg))
            } else {
                self.display_value(&first_arg)
            };
            if let Some(output) = &mut self.output {
                output.push(displayed.clone());
            }
            if let Some(sink) = &self.output_sink {
                sink.write(&displayed).map_err(|e| RuntimeError {
                    message: format!("stdout: {e}"),
                })?;
            }
            self.push(Value::Unit)?;
        } else {
            self.push(result)?;
        }
        Ok(())
    }

    /// Match a builtin ID against the four http_ variants, returning the response format.
    fn match_http_builtin(&self, builtin_id: u16) -> Option<crate::io_backend::HttpResponseFormat> {
        use crate::io_backend::HttpResponseFormat;
        if self.http_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Text)
        } else if self.http_json_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Json)
        } else if self.http_msgpack_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Msgpack)
        } else if self.http_bytes_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Bytes)
        } else {
            None
        }
    }

    /// Extract, resolve, and authorize an exec call before spawning.
    fn prepare_exec(&self, arg: Value) -> Result<ResolvedExec, RuntimeError> {
        let (command, args) = extract_exec_command_and_args(&self.heap, arg)
            .map_err(|message| RuntimeError { message })?;
        let resolved_path = resolve_exec_command_path(&command).map_err(|err| RuntimeError {
            message: format!("exec: {err}"),
        })?;
        if self
            .exec_allowed_paths
            .iter()
            .any(|path| path == &resolved_path)
        {
            Ok(ResolvedExec {
                display_command: command,
                resolved_path,
                args,
            })
        } else {
            let mut message = format!(
                "exec: command '{}' resolved to '{}' which is not in the allowed list: {:?}",
                command,
                resolved_path.display(),
                self.exec_allowed
            );
            if !self.exec_allowed_resolution_errors.is_empty() {
                message.push_str(&format!(
                    " (unresolved configured commands: {:?})",
                    self.exec_allowed_resolution_errors
                ));
            }
            Err(RuntimeError { message })
        }
    }

    /// Execute a previously authorized command with timeout.
    fn run_exec(&mut self, exec: ResolvedExec) -> Result<Value, String> {
        use std::io::Read as _;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let mut child = Command::new(&exec.resolved_path)
            .args(&exec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("exec: {e}"))?;

        let mut child_stdout = child.stdout.take().unwrap();
        let mut child_stderr = child.stderr.take().unwrap();

        let stdout_handle = std::thread::spawn(move || {
            let mut buf = String::new();
            child_stdout.read_to_string(&mut buf).ok();
            buf
        });
        let stderr_handle = std::thread::spawn(move || {
            let mut buf = String::new();
            child_stderr.read_to_string(&mut buf).ok();
            buf
        });

        let deadline = Instant::now() + Duration::from_secs(self.exec_timeout);
        let status = loop {
            match child.try_wait().map_err(|e| format!("exec: {e}"))? {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    child.kill().ok();
                    child.wait().ok();
                    return Err(format!(
                        "exec: '{}' timed out after {}s",
                        exec.display_command, self.exec_timeout
                    ));
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        };

        let stdout_str = stdout_handle.join().unwrap_or_default();
        let stderr_str = stderr_handle.join().unwrap_or_default();

        let exit_code = Value::Int(status.code().unwrap_or(-1) as i64);
        let stdout = Value::Heap(self.heap.alloc(HeapObject::String(stdout_str)));
        let stderr = Value::Heap(self.heap.alloc(HeapObject::String(stderr_str)));

        Ok(Value::Heap(self.heap.alloc(HeapObject::Tuple(smallvec![
            exit_code, stdout, stderr
        ]))))
    }

    // ── Dispatch loop ────────────────────────────────────────────────

    fn dispatch(&mut self) -> Result<(), RuntimeError> {
        loop {
            // Fuel check: decrement and error if exhausted
            if let Some(ref mut fuel) = self.fuel {
                if *fuel == 0 {
                    return Err(RuntimeError {
                        message: "fuel exhausted (execution limit reached)".into(),
                    });
                }
                *fuel -= 1;
            }

            let fi = self.frames.len() - 1;
            let proto_idx = self.frames[fi].proto_idx;
            let chunk = self.chunk_for(proto_idx);
            let ip = self.frames[fi].ip;

            if ip >= chunk.code.len() {
                return Err(RuntimeError {
                    message: "unexpected end of bytecode".into(),
                });
            }

            let op_byte = chunk.code[ip];
            self.frames[fi].ip = ip + 1;
            let op = Op::try_from(op_byte).map_err(|b| RuntimeError {
                message: format!("invalid opcode: {b}"),
            })?;

            match op {
                Op::Halt => return Ok(()),

                Op::Const => {
                    let idx = self.read_u16()? as usize;
                    let chunk = self.chunk_for(proto_idx);
                    let val = match &chunk.constants[idx] {
                        Constant::Int(n) => Value::Int(*n),
                        Constant::Float(f) => Value::Float(*f),
                        Constant::String(s) => {
                            let key = (proto_idx, idx);
                            if let Some(&cached) = self.string_cache.get(&key) {
                                Value::Heap(cached)
                            } else {
                                let v = self.alloc_string(s.clone());
                                if let Value::Heap(r) = v {
                                    self.string_cache.insert(key, r);
                                }
                                v
                            }
                        }
                        Constant::Char(c) => Value::Char(*c),
                    };
                    self.push(val)?;
                }
                Op::Unit => self.push(Value::Unit)?,
                Op::True => self.push(Value::Bool(true))?,
                Op::False => self.push(Value::Bool(false))?,

                Op::GetLocal => {
                    let slot = self.read_u16()? as usize;
                    let base = self.frames[fi].base;
                    let val = self.stack[base + slot];
                    self.push(val)?;
                }
                Op::SetLocal => {
                    let slot = self.read_u16()? as usize;
                    let base = self.frames[fi].base;
                    let val = self.pop()?;
                    self.stack[base + slot] = val;
                }
                Op::GetUpvalue => {
                    let idx = self.read_u16()? as usize;
                    let val = self.frames[fi].captures[idx];
                    self.push(val)?;
                }
                Op::GetGlobal => {
                    let idx = self.read_u16()? as usize;
                    let name = self.read_const_string(proto_idx, idx);
                    let slot = *self.global_names.get(name).ok_or_else(|| RuntimeError {
                        message: format!("undefined global: {name}"),
                    })?;
                    let val = self.globals[slot];
                    self.push(val)?;
                }
                Op::SetGlobal => {
                    let idx = self.read_u16()? as usize;
                    let name = self.read_const_string(proto_idx, idx).to_string();
                    let val = self.pop()?;
                    let slot = self.global_slot(name);
                    self.globals[slot] = val;
                }

                Op::Pop => {
                    self.pop()?;
                }

                // ── Arithmetic ──────────────────────────────────
                Op::AddInt => {
                    self.int_checked_binop(|a, b| a.checked_add(b), "integer overflow (addition)")?
                }
                Op::SubInt => self
                    .int_checked_binop(|a, b| a.checked_sub(b), "integer overflow (subtraction)")?,
                Op::MulInt => self.int_checked_binop(
                    |a, b| a.checked_mul(b),
                    "integer overflow (multiplication)",
                )?,
                Op::DivInt => {
                    self.int_checked_binop(|a, b| a.checked_div(b), "division by zero")?
                }
                Op::ModInt => self.int_checked_binop(|a, b| a.checked_rem(b), "mod by zero")?,
                Op::Neg => {
                    let val = self.pop()?;
                    match val {
                        Value::Int(n) => {
                            let neg = n.checked_neg().ok_or_else(|| RuntimeError {
                                message: "integer overflow (negation)".into(),
                            })?;
                            self.push(Value::Int(neg))?
                        }
                        Value::Float(f) => self.push(Value::Float(-f))?,
                        _ => {
                            return Err(RuntimeError {
                                message: "Neg: expected Int or Float".into(),
                            });
                        }
                    }
                }

                Op::AddFloat => self.float_binop(|a, b| a + b)?,
                Op::SubFloat => self.float_binop(|a, b| a - b)?,
                Op::MulFloat => self.float_binop(|a, b| a * b)?,
                Op::DivFloat => self.float_binop(|a, b| a / b)?,
                Op::NegFloat => {
                    let f = self.pop_float()?;
                    self.push(Value::Float(-f))?;
                }

                // ── Comparison ──────────────────────────────────
                Op::Eq => self.scalar_eq(true)?,
                Op::Ne => self.scalar_eq(false)?,
                Op::LtInt => self.int_cmp(|a, b| a < b)?,
                Op::GtInt => self.int_cmp(|a, b| a > b)?,
                Op::LeInt => self.int_cmp(|a, b| a <= b)?,
                Op::GeInt => self.int_cmp(|a, b| a >= b)?,

                Op::LtFloat => self.float_cmp(|a, b| a < b)?,
                Op::GtFloat => self.float_cmp(|a, b| a > b)?,
                Op::LeFloat => self.float_cmp(|a, b| a <= b)?,
                Op::GeFloat => self.float_cmp(|a, b| a >= b)?,

                Op::ConcatString => {
                    let b_ref = self.pop_string_ref()?;
                    let a_ref = self.pop_string_ref()?;
                    let a_s = match self.heap_get(a_ref)? {
                        HeapObject::String(s) => s.as_str(),
                        _ => "",
                    };
                    let b_s = match self.heap_get(b_ref)? {
                        HeapObject::String(s) => s.as_str(),
                        _ => "",
                    };
                    let mut result = String::with_capacity(a_s.len() + b_s.len());
                    result.push_str(a_s);
                    result.push_str(b_s);
                    let val = self.alloc_string(result);
                    self.push(val)?;
                }
                Op::Not => {
                    let b = self.pop_bool()?;
                    self.push(Value::Bool(!b))?;
                }

                // ── Tuples and data ─────────────────────────────
                Op::MakeTuple => {
                    let arity = self.read_u8()? as usize;
                    let start = self.stack.len() - arity;
                    let elems: Fields = self.stack.drain(start..).collect();
                    let val = self.alloc(HeapObject::Tuple(elems));
                    self.push(val)?;
                }
                Op::GetField => {
                    let idx = self.read_u8()? as usize;
                    let val = self.pop()?;
                    match val {
                        Value::Heap(r) => match self.heap_get(r)? {
                            HeapObject::Tuple(t) => self.push(t[idx])?,
                            HeapObject::Data { fields, .. } => self.push(fields[idx])?,
                            _ => {
                                return Err(RuntimeError {
                                    message: "GetField: expected tuple or data".into(),
                                });
                            }
                        },
                        _ => {
                            return Err(RuntimeError {
                                message: "GetField: expected tuple or data".into(),
                            });
                        }
                    }
                }
                Op::MakeData => {
                    let tag = self.read_u16()?;
                    let arity = self.read_u8()? as usize;
                    let start = self.stack.len() - arity;
                    let fields: Fields = self.stack.drain(start..).collect();
                    let val = self.alloc(HeapObject::Data { tag, fields });
                    self.push(val)?;
                }
                Op::GetTag => {
                    let val = self.pop()?;
                    match val {
                        Value::Heap(r) => match self.heap_get(r)? {
                            HeapObject::Data { tag, .. } => self.push(Value::Int(*tag as i64))?,
                            _ => {
                                return Err(RuntimeError {
                                    message: "GetTag: expected data value".into(),
                                });
                            }
                        },
                        _ => {
                            return Err(RuntimeError {
                                message: "GetTag: expected data value".into(),
                            });
                        }
                    }
                }

                // ── Control flow ────────────────────────────────
                Op::Jump => {
                    let offset = self.read_i16()?;
                    let ip = self.frames[fi].ip;
                    let target = self.checked_relative_ip(proto_idx, ip, offset, "Jump")?;
                    self.frames[fi].ip = target;
                }
                Op::JumpIfFalse => {
                    let offset = self.read_i16()?;
                    let cond = self.pop_bool()?;
                    if !cond {
                        let ip = self.frames[fi].ip;
                        let target =
                            self.checked_relative_ip(proto_idx, ip, offset, "JumpIfFalse")?;
                        self.frames[fi].ip = target;
                    }
                }

                // ── Functions ───────────────────────────────────
                Op::MakeClosure => {
                    let func_proto_idx = self.read_u16()? as usize;
                    let n_captures = self.read_u8()? as usize;
                    let mut captures = Vec::with_capacity(n_captures);
                    for _ in 0..n_captures {
                        let is_local = self.read_u8()? != 0;
                        let index = self.read_u16()? as usize;
                        let val = self.capture_value(fi, is_local, index)?;
                        captures.push(val);
                    }
                    let captures: Arc<[Value]> = Arc::from(captures);
                    let val = self.alloc(HeapObject::Closure {
                        proto_idx: func_proto_idx,
                        captures,
                    });
                    self.push(val)?;
                }

                Op::Call => {
                    let arity = self.read_u8()? as usize;
                    let callee_pos = self.stack.len() - 1 - arity;
                    let callee = self.stack[callee_pos];
                    match callee {
                        Value::Heap(r) => {
                            let (closure_proto, closure_captures) = match self.heap_get(r)? {
                                HeapObject::Closure {
                                    proto_idx,
                                    captures,
                                } => (*proto_idx, captures.clone()),
                                _ => {
                                    return Err(RuntimeError {
                                        message: format!("cannot call non-function: {callee:?}"),
                                    });
                                }
                            };
                            let proto = &self.protos[closure_proto];
                            if proto.arity as usize != arity {
                                return Err(RuntimeError {
                                    message: format!(
                                        "function expects {} arg(s), got {arity}",
                                        proto.arity
                                    ),
                                });
                            }
                            if self.frames.len() >= DEFAULT_MAX_CALL_FRAMES {
                                return Err(RuntimeError {
                                    message: "stack overflow".into(),
                                });
                            }
                            for i in 0..arity {
                                self.stack[callee_pos + i] = self.stack[callee_pos + 1 + i];
                            }
                            self.stack.truncate(callee_pos + arity);
                            self.frames.push(CallFrame {
                                proto_idx: closure_proto,
                                ip: 0,
                                base: callee_pos,
                                captures: closure_captures,
                            });
                        }
                        Value::Builtin(id) => {
                            self.call_builtin(id, callee_pos, arity)?;
                        }
                        _ => {
                            return Err(RuntimeError {
                                message: format!("cannot call non-function: {callee:?}"),
                            });
                        }
                    }
                }

                Op::TailCall => {
                    let arity = self.read_u8()? as usize;
                    let callee_pos = self.stack.len() - 1 - arity;
                    let callee = self.stack[callee_pos];
                    match callee {
                        Value::Heap(r) => {
                            let (closure_proto, closure_captures) = match self.heap_get(r)? {
                                HeapObject::Closure {
                                    proto_idx,
                                    captures,
                                } => (*proto_idx, captures.clone()),
                                _ => {
                                    return Err(RuntimeError {
                                        message: "tail call: expected function".into(),
                                    });
                                }
                            };
                            let proto = &self.protos[closure_proto];
                            if proto.arity as usize != arity {
                                return Err(RuntimeError {
                                    message: format!(
                                        "tail call: function expects {} arg(s), got {arity}",
                                        proto.arity
                                    ),
                                });
                            }
                            let fi = self.frames.len() - 1;
                            let base = self.frames[fi].base;
                            let args_start = callee_pos + 1;
                            for i in 0..arity {
                                self.stack[base + i] = self.stack[args_start + i];
                            }
                            self.stack.truncate(base + arity);
                            self.frames[fi].ip = 0;
                            self.frames[fi].proto_idx = closure_proto;
                            self.frames[fi].captures = closure_captures;
                        }
                        Value::Builtin(id) => {
                            self.call_builtin(id, callee_pos, arity)?;
                        }
                        _ => {
                            return Err(RuntimeError {
                                message: "tail call: expected function".into(),
                            });
                        }
                    }
                }

                Op::CallDirect => {
                    let proto_idx = self.read_u16()? as usize;
                    let proto = &self.protos[proto_idx];
                    let arity = proto.arity as usize;
                    let arg_start = self.stack.len() - arity;
                    if self.frames.len() >= DEFAULT_MAX_CALL_FRAMES {
                        return Err(RuntimeError {
                            message: "stack overflow".into(),
                        });
                    }
                    self.frames.push(CallFrame {
                        proto_idx,
                        ip: 0,
                        base: arg_start,
                        captures: Arc::from([]),
                    });
                }

                Op::TailCallDirect => {
                    let proto_idx = self.read_u16()? as usize;
                    let proto = &self.protos[proto_idx];
                    let arity = proto.arity as usize;
                    let args_start = self.stack.len() - arity;
                    let fi = self.frames.len() - 1;
                    let base = self.frames[fi].base;
                    for i in 0..arity {
                        self.stack[base + i] = self.stack[args_start + i];
                    }
                    self.stack.truncate(base + arity);
                    self.frames[fi].ip = 0;
                    self.frames[fi].proto_idx = proto_idx;
                    self.frames[fi].captures = Arc::from([]);
                }

                Op::Return => {
                    let result = self.pop()?;
                    let frame = self.frames.pop().unwrap();
                    self.stack.truncate(frame.base);
                    self.push(result)?;
                    if self.frames.is_empty() {
                        return Ok(());
                    }
                }

                Op::Panic => {
                    let idx = self.read_u16()? as usize;
                    let msg = self.read_const_string(proto_idx, idx).to_string();
                    return Err(RuntimeError { message: msg });
                }

                // ── Effect handlers ───────────────────────────
                Op::InstallHandler => {
                    if self.handlers.len() >= DEFAULT_MAX_HANDLER_FRAMES {
                        return Err(RuntimeError {
                            message: "too many installed effect handlers".into(),
                        });
                    }
                    let n_clauses = self.read_u16()? as usize;
                    let mut clauses = Vec::with_capacity(n_clauses);
                    for _ in 0..n_clauses {
                        let effect_tag = self.read_u16()?;
                        let offset = self.read_i16()?;
                        let abs_ip = self.checked_relative_ip(
                            self.frames[fi].proto_idx,
                            self.frames[fi].ip,
                            offset,
                            "InstallHandler clause",
                        )?;
                        clauses.push((effect_tag, abs_ip));
                    }
                    self.handlers.push(HandlerFrame {
                        call_frame_idx: fi,
                        stack_base: self.stack.len(),
                        clauses,
                        proto_idx: self.frames[fi].proto_idx,
                        captures: self.frames[fi].captures.clone(),
                    });
                }

                Op::RemoveHandler => {
                    self.handlers.pop();
                }

                Op::Perform => {
                    let effect_tag = self.read_u16()?;
                    let payload = self.pop()?;

                    // Step 1: look for user handler
                    let user_handler =
                        self.handlers.iter().enumerate().rev().find_map(|(hi, h)| {
                            h.clauses
                                .iter()
                                .find(|(t, _)| *t == effect_tag)
                                .map(|(_, ip)| (hi, *ip))
                        });

                    // Step 2: no user handler → error
                    let (h_idx, clause_ip) = user_handler.ok_or_else(|| RuntimeError {
                        message: format!("unhandled effect (tag {effect_tag})"),
                    })?;

                    {
                        // ── Unified path: always save deep continuation,
                        // also park shallow state when possible. ──────────
                        let handler_count_before = self.handlers.len();
                        let handler = self.handlers.remove(h_idx);
                        self.handlers.truncate(h_idx);

                        let handler_base = self.frames[handler.call_frame_idx].base;
                        let save_from = handler.stack_base;
                        let mut saved_stack = self.stack.split_off(save_from);

                        let handler_locals_count =
                            save_from
                                .checked_sub(handler_base)
                                .ok_or_else(|| RuntimeError {
                                    message: "perform: invalid handler stack range".into(),
                                })?;
                        if handler_locals_count > 0 {
                            let mut combined =
                                Vec::with_capacity(handler_locals_count + saved_stack.len());
                            combined.extend_from_slice(&self.stack[handler_base..save_from]);
                            combined.extend_from_slice(&saved_stack);
                            saved_stack = combined;
                        }

                        let hf = &self.frames[handler.call_frame_idx];
                        let handler_frame = SavedFrame {
                            proto_idx: hf.proto_idx,
                            ip: hf.ip,
                            base_offset: 0,
                            captures: hf.captures.clone(),
                        };

                        let mut saved_frames = vec![handler_frame];
                        for frame in &self.frames[handler.call_frame_idx + 1..] {
                            let frame_offset =
                                frame
                                    .base
                                    .checked_sub(save_from)
                                    .ok_or_else(|| RuntimeError {
                                        message: "perform: invalid saved frame base".into(),
                                    })?;
                            saved_frames.push(SavedFrame {
                                proto_idx: frame.proto_idx,
                                ip: frame.ip,
                                base_offset: handler_locals_count
                                    .checked_add(frame_offset)
                                    .ok_or_else(|| RuntimeError {
                                        message: "perform: saved frame base overflow".into(),
                                    })?,
                                captures: frame.captures.clone(),
                            });
                        }

                        let locals_offset =
                            save_from
                                .checked_sub(handler_base)
                                .ok_or_else(|| RuntimeError {
                                    message: "perform: invalid handler locals offset".into(),
                                })?;
                        let cont = self.alloc(HeapObject::Continuation {
                            saved_frames,
                            saved_stack,
                            saved_handler: Some(SavedHandler {
                                clauses: handler.clauses.clone(),
                                proto_idx: handler.proto_idx,
                                captures: handler.captures.clone(),
                                locals_offset,
                                handler_count_before,
                            }),
                        });

                        self.frames.truncate(handler.call_frame_idx + 1);

                        let hfi = self.frames.len() - 1;
                        self.frames[hfi].proto_idx = handler.proto_idx;
                        self.frames[hfi].captures = handler.captures;
                        self.frames[hfi].ip = clause_ip;

                        self.push(payload)?;
                        self.push(cont)?;
                    } // end perform
                }

                Op::Resume => {
                    let arg = self.pop()?;
                    let cont_val = self.pop()?;

                    // ── Deep path ─────────────────────────────────
                    let cont_ref = match cont_val {
                        Value::Heap(r) => r,
                        _ => {
                            return Err(RuntimeError {
                                message: "resume: expected continuation".into(),
                            });
                        }
                    };
                    let (saved_frames, saved_stack, saved_handler) =
                        match self.heap_get(cont_ref)? {
                            HeapObject::Continuation {
                                saved_frames,
                                saved_stack,
                                saved_handler,
                            } => (
                                saved_frames.clone(),
                                saved_stack.clone(),
                                saved_handler.clone(),
                            ),
                            _ => {
                                return Err(RuntimeError {
                                    message: "resume: expected continuation".into(),
                                });
                            }
                        };

                    // Restore saved stack and frames above the clause frame.
                    let stack_base = self.stack.len();
                    self.stack.extend_from_slice(&saved_stack);

                    let first_restored_idx = self.frames.len();
                    for sf in &saved_frames {
                        let frame_base =
                            stack_base
                                .checked_add(sf.base_offset)
                                .ok_or_else(|| RuntimeError {
                                    message: "resume: saved frame base overflow".into(),
                                })?;
                        self.frames.push(CallFrame {
                            proto_idx: sf.proto_idx,
                            ip: sf.ip,
                            base: frame_base,
                            captures: sf.captures.clone(),
                        });
                    }

                    // Auto-reinstall handler if the clause didn't reinstall
                    // it manually (via recursive wrapper). This eliminates the
                    // need for `with_handler (fn () => resume k v)` and prevents
                    // recursive frame accumulation.
                    if let Some(sh) = saved_handler
                        && self.handlers.len() < sh.handler_count_before
                    {
                        let handler_frame_base = self.frames[first_restored_idx].base;
                        let stack_base = handler_frame_base
                            .checked_add(sh.locals_offset)
                            .ok_or_else(|| RuntimeError {
                                message: "resume: handler stack base overflow".into(),
                            })?;
                        self.handlers.push(HandlerFrame {
                            call_frame_idx: first_restored_idx,
                            stack_base,
                            clauses: sh.clauses,
                            proto_idx: sh.proto_idx,
                            captures: sh.captures,
                        });
                    }

                    // Push the argument as the "return value" of perform
                    self.push(arg)?;
                }
            }
        }
    }

    // ── Stack helpers ────────────────────────────────────────────────

    fn push(&mut self, val: Value) -> Result<(), RuntimeError> {
        if self.stack.len() >= DEFAULT_MAX_STACK_SLOTS {
            return Err(RuntimeError {
                message: "stack overflow".into(),
            });
        }
        self.stack.push(val);
        Ok(())
    }

    fn pop(&mut self) -> Result<Value, RuntimeError> {
        self.stack.pop().ok_or_else(|| RuntimeError {
            message: "stack underflow".into(),
        })
    }

    fn pop_int(&mut self) -> Result<i64, RuntimeError> {
        match self.pop()? {
            Value::Int(n) => Ok(n),
            v => Err(RuntimeError {
                message: format!("expected Int, got {v:?}"),
            }),
        }
    }

    fn pop_float(&mut self) -> Result<f64, RuntimeError> {
        match self.pop()? {
            Value::Float(f) => Ok(f),
            v => Err(RuntimeError {
                message: format!("expected Float, got {v:?}"),
            }),
        }
    }

    fn pop_bool(&mut self) -> Result<bool, RuntimeError> {
        match self.pop()? {
            Value::Bool(b) => Ok(b),
            v => Err(RuntimeError {
                message: format!("expected Bool, got {v:?}"),
            }),
        }
    }

    fn pop_string_ref(&mut self) -> Result<GcRef, RuntimeError> {
        match self.pop()? {
            Value::Heap(r) => Ok(r),
            v => Err(RuntimeError {
                message: format!("expected String, got {v:?}"),
            }),
        }
    }

    fn scalar_eq(&mut self, eq: bool) -> Result<(), RuntimeError> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = values_equal(a, b, &self.heap);
        self.push(Value::Bool(if eq { result } else { !result }))
    }

    fn int_checked_binop(
        &mut self,
        f: impl Fn(i64, i64) -> Option<i64>,
        err_msg: &str,
    ) -> Result<(), RuntimeError> {
        let b = self.pop_int()?;
        let a = self.pop_int()?;
        match f(a, b) {
            Some(result) => self.push(Value::Int(result)),
            None => Err(RuntimeError {
                message: err_msg.to_string(),
            }),
        }
    }

    fn float_binop(&mut self, f: impl Fn(f64, f64) -> f64) -> Result<(), RuntimeError> {
        let b = self.pop_float()?;
        let a = self.pop_float()?;
        self.push(Value::Float(f(a, b)))
    }

    fn int_cmp(&mut self, f: impl Fn(i64, i64) -> bool) -> Result<(), RuntimeError> {
        let b = self.pop_int()?;
        let a = self.pop_int()?;
        self.push(Value::Bool(f(a, b)))
    }

    fn float_cmp(&mut self, f: impl Fn(f64, f64) -> bool) -> Result<(), RuntimeError> {
        let b = self.pop_float()?;
        let a = self.pop_float()?;
        self.push(Value::Bool(f(a, b)))
    }

    fn current_code(&self) -> &[u8] {
        &self.chunk_for(self.frames.last().unwrap().proto_idx).code
    }

    fn read_u8(&mut self) -> Result<u8, RuntimeError> {
        let fi = self.frames.len() - 1;
        let ip = self.frames[fi].ip;
        let code = self.current_code();
        if ip >= code.len() {
            return Err(RuntimeError {
                message: "truncated bytecode: expected u8 operand".into(),
            });
        }
        let val = code[ip];
        self.frames[fi].ip += 1;
        Ok(val)
    }

    fn read_u16(&mut self) -> Result<u16, RuntimeError> {
        let fi = self.frames.len() - 1;
        let ip = self.frames[fi].ip;
        let code = self.current_code();
        if ip + 1 >= code.len() {
            return Err(RuntimeError {
                message: "truncated bytecode: expected u16 operand".into(),
            });
        }
        let val = u16::from_le_bytes([code[ip], code[ip + 1]]);
        self.frames[fi].ip += 2;
        Ok(val)
    }

    fn read_i16(&mut self) -> Result<i16, RuntimeError> {
        self.read_u16().map(|v| v as i16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{FilesystemPolicy, VMBuilder};
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct CaptureOutputSink {
        text: Mutex<String>,
    }

    impl OutputSink for CaptureOutputSink {
        fn write(&self, text: &str) -> std::io::Result<()> {
            self.text
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_str(text);
            Ok(())
        }
    }

    fn run(input: &str) -> VM {
        let tokens = Lexer::new(input, 0).tokenize().expect("lex error");
        let program = Parser::new(tokens).parse_program().expect("parse error");
        let (compiled, _warnings) = Compiler::compile(program).expect("compile error");
        let mut vm = VM::new(compiled);
        vm.run().expect("runtime error");
        vm
    }

    fn compile_vm(input: &str) -> VM {
        let tokens = Lexer::new(input, 0).tokenize().expect("lex error");
        let program = Parser::new(tokens).parse_program().expect("parse error");
        let (compiled, _warnings) = Compiler::compile(program).expect("compile error");
        VM::new(compiled)
    }

    fn compile_program(input: &str) -> hiko_compile::chunk::CompiledProgram {
        let tokens = Lexer::new(input, 0).tokenize().expect("lex error");
        let program = Parser::new(tokens).parse_program().expect("parse error");
        let (compiled, _warnings) = Compiler::compile(program).expect("compile error");
        compiled
    }

    fn build_int_list(heap: &mut Heap, len: usize) -> Value {
        let mut list = Value::Heap(heap.alloc(HeapObject::Data {
            tag: TAG_NIL,
            fields: smallvec![],
        }));

        for i in (0..len).rev() {
            list = Value::Heap(heap.alloc(HeapObject::Data {
                tag: TAG_CONS,
                fields: smallvec![Value::Int(i as i64), list],
            }));
        }

        list
    }

    #[test]
    fn test_try_new_rejects_invalid_compiled_program() {
        let program = hiko_compile::chunk::CompiledProgram {
            main: Arc::new(hiko_compile::chunk::Chunk {
                code: vec![Op::Const as u8, 0, 0, Op::Halt as u8],
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            functions: Arc::from([]),
            effects: Arc::from([]),
        };

        let err = match VM::try_new(program) {
            Ok(_) => panic!("invalid compiled program should be rejected"),
            Err(err) => err,
        };
        assert!(err.message().contains("constant index 0"));
    }

    #[test]
    fn test_invalid_compiled_program_fails_before_dispatch() {
        let program = hiko_compile::chunk::CompiledProgram {
            main: Arc::new(hiko_compile::chunk::Chunk {
                code: vec![Op::Const as u8, 0, 0, Op::Halt as u8],
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            functions: Arc::from([]),
            effects: Arc::from([]),
        };

        let mut vm = VM::new(program);
        let err = vm.run().expect_err("invalid compiled program should fail");
        assert!(err.message.contains("program verification failed"));
        assert!(err.message.contains("constant index 0"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "hiko-vm-{}-{}-{}",
            name,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn global_int(vm: &VM, name: &str) -> i64 {
        match vm
            .get_global(name)
            .unwrap_or_else(|| panic!("no global: {name}"))
        {
            Value::Int(n) => *n,
            v => panic!("expected Int for {name}, got {v:?}"),
        }
    }

    fn global_str(vm: &VM, name: &str) -> String {
        match vm
            .get_global(name)
            .unwrap_or_else(|| panic!("no global: {name}"))
        {
            Value::Heap(r) => match vm.heap.get(*r).unwrap() {
                HeapObject::String(s) => s.clone(),
                v => panic!("expected String for {name}, got {v:?}"),
            },
            v => panic!("expected String for {name}, got {v:?}"),
        }
    }

    fn global_bool(vm: &VM, name: &str) -> bool {
        match vm
            .get_global(name)
            .unwrap_or_else(|| panic!("no global: {name}"))
        {
            Value::Bool(b) => *b,
            v => panic!("expected Bool for {name}, got {v:?}"),
        }
    }

    #[test]
    fn test_int_arithmetic() {
        let vm = run("val x = 1 + 2 * 3");
        assert_eq!(global_int(&vm, "x"), 7);
    }

    #[test]
    fn test_val_binding() {
        let vm = run("val x = 42");
        assert_eq!(global_int(&vm, "x"), 42);
    }

    #[test]
    fn test_if_true() {
        let vm = run("val x = if true then 1 else 2");
        assert_eq!(global_int(&vm, "x"), 1);
    }

    #[test]
    fn test_if_false() {
        let vm = run("val x = if false then 1 else 2");
        assert_eq!(global_int(&vm, "x"), 2);
    }

    #[test]
    fn test_let_expr() {
        let vm = run("val x = let val a = 10 val b = 20 in a + b end");
        assert_eq!(global_int(&vm, "x"), 30);
    }

    #[test]
    fn test_simple_function() {
        let vm = run("fun f x = x + 1 val y = f 41");
        assert_eq!(global_int(&vm, "y"), 42);
    }

    #[test]
    fn test_fibonacci() {
        let vm = run("fun fib n = if n < 2 then n else fib (n - 1) + fib (n - 2)
             val result = fib 30");
        assert_eq!(global_int(&vm, "result"), 832040);
    }

    #[test]
    fn test_closure() {
        let vm = run("fun make_adder n = fn x => n + x
             val add5 = make_adder 5
             val result = add5 10");
        assert_eq!(global_int(&vm, "result"), 15);
    }

    #[test]
    fn test_higher_order() {
        let vm = run("fun twice f x = f (f x)
             val result = twice (fn x => x + 1) 0");
        assert_eq!(global_int(&vm, "result"), 2);
    }

    #[test]
    fn test_pipeline_operator() {
        let vm = run("fun inc x = x + 1
             fun double x = x * 2
             val result = 3 |> inc |> double");
        assert_eq!(global_int(&vm, "result"), 8);
    }

    #[test]
    fn test_tco_loop() {
        let vm = run("fun loop n = if n = 0 then 42 else loop (n - 1)
             val result = loop 100000");
        assert_eq!(global_int(&vm, "result"), 42);
    }

    #[test]
    fn test_string_equality() {
        let vm = run(r#"val a = "hello" = "hello" val b = "hello" = "world""#);
        assert!(global_bool(&vm, "a"));
        assert!(!global_bool(&vm, "b"));
    }

    #[test]
    fn test_values_equal_handles_deep_lists_iteratively() {
        let mut heap = Heap::new();
        let left = build_int_list(&mut heap, 50_000);
        let right = build_int_list(&mut heap, 50_000);

        assert!(values_equal(left, right, &heap));
    }

    fn builtin_answer(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
        Ok(Value::Int(42))
    }

    #[test]
    fn test_register_builtin_owned_keeps_owned_name_and_clones_to_child() {
        let program = compile_program("val x = 0");
        let mut vm = VM::new(program);
        let name = "dynamic_builtin_answer".to_string();

        vm.register_builtin_owned(name.clone(), builtin_answer);

        let builtin_id = match vm.get_global(&name) {
            Some(Value::Builtin(id)) => *id as usize,
            other => panic!("expected builtin global, got {other:?}"),
        };
        assert_eq!(vm.builtins[builtin_id].name.as_ref(), name);

        let child = vm.create_child();
        let child_builtin_id = match child.get_global(&name) {
            Some(Value::Builtin(id)) => *id as usize,
            other => panic!("expected builtin global in child, got {other:?}"),
        };
        assert_eq!(child.builtins[child_builtin_id].name.as_ref(), name);
    }

    #[test]
    fn test_option() {
        let vm = run("datatype 'a option = None | Some of 'a
             val x = case Some 42 of None => 0 | Some n => n");
        assert_eq!(global_int(&vm, "x"), 42);
    }

    #[test]
    fn test_list_map() {
        let vm = run("fun map f xs = case xs of
                [] => []
              | x :: xs => f x :: map f xs
             fun length xs = case xs of [] => 0 | _ :: xs => 1 + length xs
             val result = length (map (fn x => x * 2) [1, 2, 3])");
        assert_eq!(global_int(&vm, "result"), 3);
    }

    #[test]
    fn test_gc_runs() {
        // Use a tail-recursive builder to avoid stack overflow, then count with accumulator
        let vm = run(
            "fun make_list_acc n acc = if n = 0 then acc else make_list_acc (n - 1) (n :: acc)
             fun length_acc xs acc = case xs of [] => acc | _ :: rest => length_acc rest (acc + 1)
             val result = length_acc (make_list_acc 5000 []) 0",
        );
        assert_eq!(global_int(&vm, "result"), 5000);
        assert!(vm.heap_live_count() < 15000); // GC should have collected intermediate values
    }

    #[test]
    fn test_boundary_gc_runs_before_async_suspend() {
        let program = compile_program(
            "fun churn n = if n = 0 then () else let val _ = (n, n) in churn (n - 1) end
             val _ = churn 600
             val _ = sleep 1",
        );
        let mut vm = VMBuilder::new(program).with_core().build();
        vm.async_io = true;

        match vm.run_slice(1_000_000) {
            RunResult::Io(crate::io_backend::IoRequest::Sleep(_)) => {}
            other => panic!("expected async sleep boundary, got {other:?}"),
        }

        let live = vm.heap_live_count();
        assert!(
            live < 100,
            "boundary GC should reclaim request-local garbage before suspend, live={live}"
        );
    }

    // ── Effect handler tests ─────────────────────────────────────────

    #[test]
    fn test_effect_handle_no_perform() {
        // Body returns normally, goes through return clause
        let vm = run("effect Ask of unit
             val result = handle 42 with return x => x + 1");
        assert_eq!(global_int(&vm, "result"), 43);
    }

    #[test]
    fn test_effect_perform_simple() {
        // Perform an effect, handler returns a value without resuming
        let vm = run("effect Ask of unit
             val result = handle perform Ask ()
               with return x => x
                  | Ask _ k => 99");
        assert_eq!(global_int(&vm, "result"), 99);
    }

    #[test]
    fn test_effect_perform_with_resume() {
        // Perform + resume: the continuation returns the resumed value
        let vm = run("effect Ask of unit
             val result = handle 1 + perform Ask ()
               with return x => x
                  | Ask _ k => resume k 41");
        assert_eq!(global_int(&vm, "result"), 42);
    }

    #[test]
    fn test_effect_perform_payload() {
        // Effect carries a payload
        let vm = run("effect Double of int
             val result = handle perform Double 21
               with return x => x
                  | Double n k => resume k (n * 2)");
        assert_eq!(global_int(&vm, "result"), 42);
    }

    #[test]
    fn test_effect_resume_direct_in_function() {
        // resume k v directly in a handler clause inside a function —
        // this was crashing due to stack corruption before the fix.
        let vm = run("effect Fetch of string\n\
             fun with_mock f =\n\
               handle f ()\n\
               with return x => x\n\
                  | Fetch url k => resume k (\"got:\" ^ url)\n\
             fun program () = perform Fetch \"test\"\n\
             val result = with_mock program");
        assert_eq!(global_str(&vm, "result"), "got:test");
    }

    #[test]
    fn test_effect_multiple_performs_reinstalled() {
        // Multiple performs through a recursively reinstalled handler
        let vm = run("effect Ask of int\n\
             fun with_handler f =\n\
               handle f ()\n\
               with return x => x\n\
                  | Ask n k => with_handler (fn () => resume k (n * 10))\n\
             fun program () =\n\
               let val a = perform Ask 1\n\
                   val b = perform Ask 2\n\
                   val c = perform Ask 3\n\
               in a + b + c end\n\
             val result = with_handler program");
        assert_eq!(global_int(&vm, "result"), 60); // 10 + 20 + 30
    }

    #[test]
    fn test_effect_generator() {
        let vm = run("effect Yield of int
             fun run_gen f = handle f ()
               with return _ => 0
                  | Yield n k => n + run_gen (fn _ => resume k ())
             fun gen () =
               let val _ = perform Yield 1
                   val _ = perform Yield 2
                   val _ = perform Yield 3
               in () end
             val result = run_gen gen");
        assert_eq!(global_int(&vm, "result"), 6);
    }

    #[test]
    fn test_effect_no_resume() {
        // Handler does not resume, so it aborts the computation
        let vm = run("effect Abort of int
             fun f () = let val _ = perform Abort 42 in 0 end
             val result = handle f ()
               with return x => x
                  | Abort n _ => n");
        assert_eq!(global_int(&vm, "result"), 42);
    }

    #[test]
    fn test_effect_nested_handlers() {
        // Nested handle blocks with different effects
        let vm = run("effect A of unit
             effect B of unit
             val result = handle
               handle 1 + perform A () + perform B ()
               with return x => x
                  | A _ k => resume k 10
             with return x => x
                | B _ k => resume k 100");
        assert_eq!(global_int(&vm, "result"), 111);
    }

    #[test]
    fn test_effect_state() {
        // State effect: get/put pattern
        let vm = run("effect Get of unit
             effect Put of int
             fun run_state init f =
               handle f ()
               with return x => x
                  | Get _ k => run_state init (fn _ => resume k init)
                  | Put n k => run_state n (fn _ => resume k ())
             val result = run_state 0 (fn _ =>
               let val _ = perform Put 42
               in perform Get () end)");
        assert_eq!(global_int(&vm, "result"), 42);
    }

    #[test]
    fn test_effect_resume_direct_large_loop() {
        // Repeated direct resumes should stay correct for larger workloads,
        // even before a dedicated tail-resume path lands.
        let vm = run("effect Ask of int\n\
             fun loop n acc =\n\
               if n = 0 then acc\n\
               else let val x = perform Ask n\n\
                    in loop (n - 1) (acc + x) end\n\
             fun run_effect f =\n\
               handle f ()\n\
               with return x => x\n\
                  | Ask n k => resume k n\n\
             val result = run_effect (fn () => loop 20000 0)");
        assert_eq!(global_int(&vm, "result"), 200010000);
    }

    #[test]
    fn test_effect_resume_non_tail_pending_application() {
        // A perform inside a larger expression must restore the pending
        // application state exactly; this used to corrupt the callee slot.
        let vm = run("effect Ask of int\n\
             fun loop n acc =\n\
               if n = 0 then acc\n\
               else step n acc\n\
             and step n acc =\n\
               loop (n - 1) (acc + perform Ask n)\n\
             fun run_effect f =\n\
               handle f ()\n\
               with return x => x\n\
                  | Ask n k => resume k n\n\
             val result = run_effect (fn () => loop 500 0)");
        assert_eq!(global_int(&vm, "result"), 125250);
    }

    #[test]
    fn test_resume_blocked_invalid_continuation_errors() {
        let mut vm = compile_vm("val _ = ()");
        let bogus = vm
            .heap
            .alloc(HeapObject::String("not a continuation".into()));
        vm.blocked_continuation = Some(bogus);

        let err = vm.resume_blocked(Value::Int(1)).unwrap_err();
        assert_eq!(err.message, "resume_blocked: expected continuation");
        assert!(vm.blocked_continuation.is_none());
    }

    #[test]
    fn test_resume_blocked_rejects_saved_frame_base_overflow() {
        let mut vm = compile_vm("val _ = ()");
        vm.stack.push(Value::Unit);
        let cont = vm.heap.alloc(HeapObject::Continuation {
            saved_frames: vec![
                SavedFrame {
                    proto_idx: usize::MAX,
                    ip: 0,
                    base_offset: 0,
                    captures: Arc::from([]),
                },
                SavedFrame {
                    proto_idx: usize::MAX,
                    ip: 0,
                    base_offset: usize::MAX,
                    captures: Arc::from([]),
                },
            ],
            saved_stack: vec![Value::Unit],
            saved_handler: None,
        });
        vm.blocked_continuation = Some(cont);

        let err = vm.resume_blocked(Value::Int(1)).unwrap_err();
        assert_eq!(err.message, "resume_blocked: saved frame base overflow");
    }

    #[test]
    fn test_capture_value_rejects_out_of_bounds_local_index() {
        let mut vm = compile_vm("val _ = ()");
        vm.stack.push(Value::Unit);
        vm.frames.push(CallFrame {
            proto_idx: usize::MAX,
            ip: 0,
            base: 0,
            captures: Arc::from([]),
        });

        let err = vm.capture_value(0, true, 1).unwrap_err();
        assert_eq!(
            err.message,
            "MakeClosure: local capture index 1 out of bounds"
        );
    }

    #[test]
    fn test_install_handler_rejects_excessive_nesting() {
        let program = hiko_compile::chunk::CompiledProgram {
            main: Arc::new(hiko_compile::chunk::Chunk {
                code: vec![Op::InstallHandler as u8, 0, 0, Op::Halt as u8],
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            functions: Arc::from([]),
            effects: Arc::from([]),
        };
        let mut vm = VM::new(program);
        for _ in 0..DEFAULT_MAX_HANDLER_FRAMES {
            vm.handlers.push(HandlerFrame {
                call_frame_idx: 0,
                stack_base: 0,
                clauses: Vec::new(),
                proto_idx: usize::MAX,
                captures: Arc::from([]),
            });
        }

        let err = vm.run().unwrap_err();
        assert_eq!(err.message, "too many installed effect handlers");
    }

    #[test]
    fn test_heap_limit_returns_runtime_error() {
        let mut vm = compile_vm("val pair = (1, 2)");
        vm.set_max_heap(0);

        let err = vm.run().unwrap_err();
        assert!(err.is_heap_limit());
        assert_eq!(err.message, "heap limit exceeded: 0 objects (max 0)");
    }

    #[test]
    fn test_create_child_shares_compiled_program() {
        let vm = compile_vm("fun id x = x\nval _ = id 1");
        let child = vm.create_child();

        assert!(Arc::ptr_eq(&vm.main_chunk, &child.main_chunk));
        assert!(Arc::ptr_eq(&vm.protos, &child.protos));
        assert!(Arc::ptr_eq(&vm.effect_metadata, &child.effect_metadata));
    }

    #[test]
    fn test_output_sink_streams_print_and_println() {
        let sink = Arc::new(CaptureOutputSink::default());
        let mut vm = compile_vm("val _ = print \"a\" val _ = println \"b\"");
        vm.enable_output_capture();
        vm.set_output_sink(sink.clone());

        vm.run().unwrap();

        let streamed = sink.text.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(streamed, "ab\n");
        assert_eq!(vm.get_output().concat(), "ab\n");
    }

    #[test]
    fn test_read_stdin_uses_injected_buffer_once() {
        let mut vm = compile_vm("val first = read_stdin () val second = read_stdin ()");
        vm.set_stdin_override("{\"path\":\"src/main.rs\"}".to_string());

        vm.run().unwrap();

        assert_eq!(global_str(&vm, "first"), "{\"path\":\"src/main.rs\"}");
        assert_eq!(global_str(&vm, "second"), "");
    }

    // ── Capability / builder tests ───────────────────────────────────

    #[test]
    fn test_fuel_exhaustion() {
        let tokens = Lexer::new("fun loop n = loop (n + 1) val _ = loop 0", 0)
            .tokenize()
            .unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile(program).unwrap();

        let mut vm = crate::builder::VMBuilder::new(compiled)
            .with_core()
            .max_fuel(1000)
            .build();

        let result = vm.run();
        assert!(result.is_err());
        assert!(result.unwrap_err().is_fuel_exhausted());
    }

    #[test]
    fn test_sandboxed_no_filesystem() {
        let tokens = Lexer::new("val x = 1 + 2", 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let (compiled, _) = Compiler::compile(program).unwrap();

        let mut vm = VMBuilder::new(compiled).with_core().build();
        vm.run().unwrap();
        match vm.get_global("x") {
            Some(Value::Int(3)) => {}
            other => panic!("expected Int(3), got {other:?}"),
        }
        assert!(vm.get_global("epoch_ms").is_some());
        #[cfg(feature = "builtin-time")]
        assert!(vm.get_global("date_utc_tz").is_some());
        // Filesystem and HTTP builtins should not exist
        assert!(vm.get_global("read_file").is_none());
        assert!(vm.get_global("write_file").is_none());
        assert!(vm.get_global("http_get").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_exec_allows_resolved_absolute_path() {
        use crate::builder::ExecPolicy;
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("exec-allowed");
        let script = root.join("allowed.sh");
        fs::write(&script, "#!/bin/sh\necho ok\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        let program = compile_program(&format!(
            "val (code, out, err) = exec ({:?}, [])",
            script.to_string_lossy()
        ));
        let mut vm = VMBuilder::new(program)
            .with_core()
            .with_exec(ExecPolicy {
                allowed: vec![script.to_string_lossy().to_string()],
                timeout: 1,
            })
            .build();

        vm.run().unwrap();
        assert_eq!(global_int(&vm, "code"), 0);
        assert_eq!(global_str(&vm, "out"), "ok\n");
        assert_eq!(global_str(&vm, "err"), "");

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn test_exec_rejects_resolved_path_not_in_allowlist() {
        use crate::builder::ExecPolicy;
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("exec-denied");
        let allowed = root.join("allowed.sh");
        let denied = root.join("denied.sh");
        fs::write(&allowed, "#!/bin/sh\necho allowed\n").unwrap();
        fs::write(&denied, "#!/bin/sh\necho denied\n").unwrap();
        for path in [&allowed, &denied] {
            let mut perms = fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }

        let program = compile_program(&format!(
            "val _ = exec ({:?}, [])",
            denied.to_string_lossy()
        ));
        let mut vm = VMBuilder::new(program)
            .with_core()
            .with_exec(ExecPolicy {
                allowed: vec![allowed.to_string_lossy().to_string()],
                timeout: 1,
            })
            .build();

        let err = vm.run().unwrap_err();
        assert!(err.message.contains("resolved to"));
        assert!(err.message.contains("not in the allowed list"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_fs_root_rejects_list_dir_traversal() {
        let root = temp_dir("fs-root-list");
        let program = compile_program("val _ = list_dir \"../\"");
        let mut vm = VMBuilder::new(program)
            .with_core()
            .with_filesystem(FilesystemPolicy {
                root: root.to_string_lossy().to_string(),
                allow_read: true,
                allow_write: false,
                allow_delete: false,
            })
            .build();

        let err = vm.run().unwrap_err();
        assert!(err.message.contains("outside allowed root"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_fs_root_allows_create_dir_for_missing_nested_child() {
        let root = temp_dir("fs-root-create");
        let program = compile_program("val _ = create_dir \"nested/a/b\"");
        let mut vm = VMBuilder::new(program)
            .with_core()
            .with_filesystem(FilesystemPolicy {
                root: root.to_string_lossy().to_string(),
                allow_read: false,
                allow_write: true,
                allow_delete: false,
            })
            .build();

        vm.run().unwrap();
        assert!(root.join("nested").join("a").join("b").is_dir());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_fs_root_rejects_glob_traversal() {
        let root = temp_dir("fs-root-glob");
        let program = compile_program("val _ = glob \"../*\"");
        let mut vm = VMBuilder::new(program)
            .with_core()
            .with_filesystem(FilesystemPolicy {
                root: root.to_string_lossy().to_string(),
                allow_read: true,
                allow_write: false,
                allow_delete: false,
            })
            .build();

        let err = vm.run().unwrap_err();
        assert!(err.message.contains("outside allowed root"));

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn test_fs_root_rejects_walk_dir_symlink_escape() {
        use std::os::unix::fs::symlink;

        let base = temp_dir("fs-root-walk");
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        symlink(&outside, root.join("link")).unwrap();

        let program = compile_program("val _ = walk_dir \".\"");
        let mut vm = VMBuilder::new(program)
            .with_core()
            .with_filesystem(FilesystemPolicy {
                root: root.to_string_lossy().to_string(),
                allow_read: true,
                allow_write: false,
                allow_delete: false,
            })
            .build();

        let err = vm.run().unwrap_err();
        assert!(err.message.contains("outside allowed root"));

        let _ = fs::remove_dir_all(base);
    }
}
