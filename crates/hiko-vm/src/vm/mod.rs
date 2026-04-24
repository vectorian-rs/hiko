//! Bytecode VM entry point and shared VM state.
//!
//! The implementation is split into focused submodules:
//!
//! - `runtime_bridge`: runtime-visible slice transitions and process creation
//! - `dispatch`: opcode interpreter and stack helpers
//! - `builtins`: builtin registration and runtime-aware builtin dispatch
//! - `gc`: allocation helpers and root-set management
//! - `host`: output sinks, stdin, and `exec`

#[cfg(test)]
use smallvec::smallvec;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use hiko_compile::chunk::{Chunk, CompiledProgram, Constant, EffectMeta, FunctionProto};
use hiko_compile::op::Op;

use crate::heap::Heap;
use crate::process::ProcessFailure;
use crate::value::{BuiltinEntry, Fields, GcRef, HeapObject, SavedFrame, SavedHandler, Value};
use crate::verify::{VerificationError, verify_program};

mod builtins;
mod dispatch;
mod gc;
mod host;
mod runtime_bridge;

pub use host::{OutputSink, StdoutOutputSink};
pub use runtime_bridge::{RunResult, RuntimeRequest};

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
    /// When true, I/O builtins suspend via `RuntimeRequest::Io` instead of blocking.
    async_io: bool,
    /// Pending runtime request from a process/runtime builtin.
    pending_runtime_request: Option<RuntimeRequest>,
    /// Effect metadata from compiled program (name → tag).
    pub effect_metadata: Arc<[EffectMeta]>,
    /// Saved continuation when blocked on runtime I/O. GC root.
    blocked_continuation: Option<GcRef>,
    /// Cooperative cancellation flag. Checked at suspension points.
    cancelled: bool,
    startup_error: Option<String>,
    output_sink: Option<Arc<dyn OutputSink>>,
}

pub(crate) fn values_equal(a: Value, b: Value, heap: &Heap) -> bool {
    let mut worklist = vec![(a, b)];

    while let Some((left, right)) = worklist.pop() {
        match (left, right) {
            (Value::Int(x), Value::Int(y)) if x == y => {}
            (Value::Word(x), Value::Word(y)) if x == y => {}
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
            frames: Vec::with_capacity(16),
            globals: Vec::with_capacity(program.main.constants.len()),
            global_names: HashMap::new(),
            effect_metadata: program.effects,
            protos: program.functions,
            main_chunk: program.main,
            output: None,
            builtins: Vec::new(),
            handlers: Vec::with_capacity(4),
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

    /// Push a value onto the stack (used by runtime to inject results).
    pub fn push_value(&mut self, value: Value) {
        self.stack.push(value);
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

    /// Set the maximum heap size (in number of objects).
    pub fn set_max_heap(&mut self, max: usize) {
        self.heap.set_max_objects(max);
    }

    /// Set the work limit (currently measured as opcode executions).
    /// This sets both the per-run fuel and the persistent budget.
    pub fn set_max_work(&mut self, work: u64) {
        self.set_fuel(work);
    }

    /// Set the fuel limit (max opcode executions).
    /// This remains as a compatibility alias for `set_max_work`.
    pub fn set_fuel(&mut self, fuel: u64) {
        self.fuel = Some(fuel);
        self.max_fuel_remaining = Some(fuel);
    }

    /// Set the maximum tracked heap memory in bytes.
    pub fn set_max_memory_bytes(&mut self, max: usize) {
        self.heap.set_max_bytes(max);
    }

    /// Set the maximum cumulative I/O bytes charged to this VM.
    pub fn set_max_io_bytes(&mut self, max: u64) {
        self.heap.set_max_io_bytes(max);
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

    pub fn disable_output_capture(&mut self) {
        self.output = None;
    }

    pub fn heap_live_count(&self) -> usize {
        self.heap.live_count()
    }

    pub fn heap_live_bytes(&self) -> usize {
        self.heap.live_bytes()
    }

    pub fn heap_peak_bytes(&self) -> usize {
        self.heap.peak_bytes()
    }

    pub fn io_bytes_used(&self) -> u64 {
        self.heap.io_bytes_used()
    }

    pub fn error_span(&self) -> Option<hiko_syntax::span::Span> {
        let frame = self.frames.last()?;
        let chunk = self.chunk_for_checked(frame.proto_idx).ok()?;
        chunk.span_at(frame.ip.saturating_sub(1))
    }

    fn unknown_proto_error(proto_idx: usize) -> RuntimeError {
        RuntimeError {
            message: format!("unknown function proto: {proto_idx}"),
        }
    }

    fn proto(&self, proto_idx: usize) -> Result<&FunctionProto, RuntimeError> {
        self.protos
            .get(proto_idx)
            .ok_or_else(|| Self::unknown_proto_error(proto_idx))
    }

    fn chunk_for_checked(&self, proto_idx: usize) -> Result<&Chunk, RuntimeError> {
        if proto_idx == usize::MAX {
            Ok(&self.main_chunk)
        } else {
            Ok(&self.proto(proto_idx)?.chunk)
        }
    }

    fn read_const_string(&self, proto_idx: usize, idx: usize) -> Result<&str, RuntimeError> {
        let chunk = self.chunk_for_checked(proto_idx)?;
        match chunk.constants.get(idx) {
            Some(Constant::String(s)) => Ok(s),
            Some(_) => Err(RuntimeError {
                message: format!("expected string constant at index {idx}"),
            }),
            None => Err(RuntimeError {
                message: format!("constant index out of bounds: {idx}"),
            }),
        }
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
        let mut list = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_NIL,
                fields: smallvec![],
            })
            .unwrap(),
        );

        for i in (0..len).rev() {
            list = Value::Heap(
                heap.alloc(HeapObject::Data {
                    tag: TAG_CONS,
                    fields: smallvec![Value::Int(i as i64), list],
                })
                .unwrap(),
            );
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

    #[test]
    fn test_dispatch_rejects_unknown_function_proto() {
        let mut vm = compile_vm("val _ = ()");
        vm.frames.push(CallFrame {
            proto_idx: 9999,
            ip: 0,
            base: 0,
            captures: Arc::from([]),
        });

        let err = vm
            .dispatch()
            .expect_err("invalid proto index should not panic");
        assert_eq!(err.message, "unknown function proto: 9999");
    }

    #[test]
    fn test_dispatch_rejects_out_of_bounds_local_without_panicking() {
        let program = hiko_compile::chunk::CompiledProgram {
            main: Arc::new(hiko_compile::chunk::Chunk {
                code: vec![Op::GetLocal as u8, 1, 0, Op::Halt as u8],
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            functions: Arc::from([]),
            effects: Arc::from([]),
        };

        let mut vm = VM::new(program);
        let err = vm.run().expect_err("invalid local slot should fail");
        assert_eq!(err.message, "GetLocal: stack index 1 out of bounds");
    }

    #[test]
    fn test_dispatch_rejects_out_of_bounds_field_without_panicking() {
        let program = hiko_compile::chunk::CompiledProgram {
            main: Arc::new(hiko_compile::chunk::Chunk {
                code: vec![
                    Op::MakeTuple as u8,
                    0,
                    Op::GetField as u8,
                    0,
                    Op::Halt as u8,
                ],
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            functions: Arc::from([]),
            effects: Arc::from([]),
        };

        let mut vm = VM::new(program);
        let err = vm.run().expect_err("invalid field index should fail");
        assert_eq!(err.message, "GetField: field index 0 out of bounds");
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
        vm.set_async_io(true);

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

    #[test]
    fn test_request_cancellation_causes_next_slice_to_cancel() {
        let program = compile_program(
            "fun loop n = if n = 0 then () else loop (n - 1)
             val _ = loop 10000",
        );
        let mut vm = VM::new(program);

        vm.request_cancellation();

        assert!(matches!(vm.run_slice(100), RunResult::Cancelled));
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
            .alloc(HeapObject::String("not a continuation".into()))
            .unwrap();
        vm.blocked_continuation = Some(bogus);

        let err = vm.resume_blocked(Value::Int(1)).unwrap_err();
        assert_eq!(err.message, "resume_blocked: expected continuation");
        assert!(vm.blocked_continuation.is_none());
    }

    #[test]
    fn test_resume_blocked_rejects_saved_frame_base_overflow() {
        let mut vm = compile_vm("val _ = ()");
        vm.stack.push(Value::Unit);
        let cont = vm
            .heap
            .alloc(HeapObject::Continuation {
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
            })
            .unwrap();
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
    fn test_heap_limit_collects_before_failing() {
        let mut vm = compile_vm(
            "fun churn n =
               if n = 0 then 42
               else let val _ = (n, n)
                    in churn (n - 1) end
             val result = churn 200",
        );
        vm.set_max_heap(32);

        vm.run()
            .expect("unreachable tuples should be collected before heap limit failure");
        assert_eq!(global_int(&vm, "result"), 42);
        assert!(vm.heap_live_count() <= 32);
    }

    #[test]
    fn test_memory_limit_returns_runtime_error() {
        let mut vm = compile_vm("val pair = (1, 2)");
        vm.set_max_memory_bytes(0);

        let err = vm.run().unwrap_err();
        assert!(err.message.starts_with("memory limit exceeded:"));
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
    fn test_io_limit_returns_runtime_error_on_stdout() {
        let mut vm = compile_vm("val _ = println \"abcd\"");
        vm.enable_output_capture();
        vm.set_max_io_bytes(4);

        let err = vm.run().unwrap_err();
        assert!(err.message.starts_with("stdout: io limit exceeded:"));
        assert!(vm.get_output().is_empty());
    }

    #[test]
    fn test_tracked_usage_reports_memory_and_io_bytes() {
        let mut vm = compile_vm("val pair = (1, 2)\nval _ = println \"ok\"");
        vm.enable_output_capture();

        vm.run().unwrap();

        assert!(vm.heap_live_bytes() > 0);
        assert!(vm.heap_peak_bytes() >= vm.heap_live_bytes());
        assert_eq!(vm.io_bytes_used(), 3);
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
