use std::collections::HashMap;
use std::rc::Rc;

use hiko_compile::chunk::{Chunk, CompiledProgram, Constant};
use hiko_compile::op::Op;

use crate::heap::Heap;
use crate::value::{BuiltinEntry, BuiltinFn, GcRef, HeapObject, SavedFrame, Value};

const MAX_STACK: usize = 64 * 1024;
const MAX_FRAMES: usize = 1024;

const BUILTIN_PRINT: u16 = 0;
const BUILTIN_PRINTLN: u16 = 1;

const TAG_NIL: u16 = 0;
const TAG_CONS: u16 = 1;

#[derive(Debug)]
pub struct RuntimeError {
    pub message: String,
}

struct CallFrame {
    proto_idx: usize,
    ip: usize,
    base: usize,
    captures: Rc<[Value]>,
}

struct HandlerFrame {
    call_frame_idx: usize,
    stack_base: usize,          // stack height when handler was installed
    clauses: Vec<(u16, usize)>, // (effect_tag, absolute_ip)
    proto_idx: usize,
    captures: Rc<[Value]>,
}

pub struct VM {
    heap: Heap,
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    globals: Vec<Value>,
    global_names: HashMap<String, usize>,
    protos: Vec<hiko_compile::chunk::FunctionProto>,
    main_chunk: Chunk,
    output: Vec<String>,
    builtins: Vec<BuiltinEntry>,
    handlers: Vec<HandlerFrame>,
    string_cache: HashMap<(usize, usize), GcRef>,
}

fn values_equal(a: Value, b: Value, heap: &Heap) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Char(x), Value::Char(y)) => x == y,
        (Value::Unit, Value::Unit) => true,
        (Value::Heap(ra), Value::Heap(rb)) => {
            if ra == rb {
                return true;
            }
            let (Ok(obj_a), Ok(obj_b)) = (heap.get(ra), heap.get(rb)) else {
                return false;
            };
            match (obj_a, obj_b) {
                (HeapObject::String(sa), HeapObject::String(sb)) => sa == sb,
                (HeapObject::Tuple(ta), HeapObject::Tuple(tb)) => {
                    ta.len() == tb.len()
                        && ta
                            .iter()
                            .zip(tb.iter())
                            .all(|(x, y)| values_equal(*x, *y, heap))
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
                    ta == tb
                        && fa.len() == fb.len()
                        && fa
                            .iter()
                            .zip(fb.iter())
                            .all(|(x, y)| values_equal(*x, *y, heap))
                }
                _ => false,
            }
        }
        _ => false,
    }
}

impl VM {
    pub fn new(program: CompiledProgram) -> Self {
        let mut vm = VM {
            heap: Heap::new(),
            stack: Vec::with_capacity(256),
            frames: Vec::new(),
            globals: Vec::new(),
            global_names: HashMap::new(),
            protos: program.functions,
            main_chunk: program.main,
            output: Vec::new(),
            builtins: Vec::new(),
            handlers: Vec::new(),
            string_cache: HashMap::new(),
        };
        vm.register_builtins();
        vm
    }

    // ── Heap helpers ─────────────────────────────────────────────────

    fn heap_get(&self, r: GcRef) -> Result<&HeapObject, RuntimeError> {
        self.heap
            .get(r)
            .map_err(|e| RuntimeError { message: e.into() })
    }

    fn alloc(&mut self, obj: HeapObject) -> Value {
        if self.heap.should_collect() {
            self.gc_collect();
        }
        Value::Heap(self.heap.alloc(obj))
    }

    fn alloc_string(&mut self, s: String) -> Value {
        self.alloc(HeapObject::String(s))
    }

    fn gc_collect(&mut self) {
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
            .chain(self.string_cache.values().copied());
        self.heap.collect(roots);
    }

    /// Format a value for display (print/println).
    fn display_value(&self, v: &Value) -> String {
        match v {
            Value::Builtin(id) => format!("<builtin:{}>", self.builtins[*id as usize].name),
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
                        format!(
                            "{} :: {}",
                            self.display_value(&fields[0]),
                            self.display_value(&fields[1])
                        )
                    } else {
                        format!("Data({tag})")
                    }
                }
                Ok(HeapObject::Closure { .. }) => "<fn>".to_string(),
                Ok(HeapObject::Continuation { .. }) => "<continuation>".to_string(),
                Err(_) => "<dangling ref>".to_string(),
            },
            other => other.to_string(),
        }
    }

    // ── Builtins ─────────────────────────────────────────────────────

    fn register_builtins(&mut self) {
        fn alloc_list(heap: &mut Heap, elems: Vec<Value>) -> Value {
            let mut list = Value::Heap(heap.alloc(HeapObject::Data {
                tag: TAG_NIL,
                fields: vec![],
            }));
            for elem in elems.into_iter().rev() {
                list = Value::Heap(heap.alloc(HeapObject::Data {
                    tag: TAG_CONS,
                    fields: vec![elem, list],
                }));
            }
            list
        }
        fn bi_print(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            // Return a marker; the VM handles display
            Ok(args[0]) // VM will display this value
        }
        fn bi_println(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            Ok(args[0])
        }
        fn bi_read_line(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| format!("read_line: {e}"))?;
            if line.ends_with('\n') {
                line.pop();
            }
            Ok(Value::Heap(heap.alloc(HeapObject::String(line))))
        }
        fn bi_int_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Int(n) => Ok(Value::Heap(heap.alloc(HeapObject::String(n.to_string())))),
                _ => Err("int_to_string: expected Int".into()),
            }
        }
        fn bi_float_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::Heap(heap.alloc(HeapObject::String(f.to_string())))),
                _ => Err("float_to_string: expected Float".into()),
            }
        }
        fn bi_string_to_int(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s
                        .trim()
                        .parse::<i64>()
                        .map(Value::Int)
                        .map_err(|e| format!("string_to_int: {e}")),
                    _ => Err("string_to_int: expected String".into()),
                },
                _ => Err("string_to_int: expected String".into()),
            }
        }
        fn bi_char_to_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Char(c) => Ok(Value::Int(*c as i64)),
                _ => Err("char_to_int: expected Char".into()),
            }
        }
        fn bi_int_to_char(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Int(n) => char::from_u32(*n as u32)
                    .map(Value::Char)
                    .ok_or_else(|| format!("int_to_char: invalid codepoint {n}")),
                _ => Err("int_to_char: expected Int".into()),
            }
        }
        fn bi_int_to_float(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Int(n) => Ok(Value::Float(*n as f64)),
                _ => Err("int_to_float: expected Int".into()),
            }
        }
        fn bi_string_length(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => Ok(Value::Int(s.chars().count() as i64)),
                    _ => Err("string_length: expected String".into()),
                },
                _ => Err("string_length: expected String".into()),
            }
        }
        fn bi_substring(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let (v0, v1, v2) = match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::Tuple(t) => (t[0], t[1], t[2]),
                    _ => return Err("substring: expected (String, Int, Int)".into()),
                },
                _ => return Err("substring: expected (String, Int, Int)".into()),
            };
            let s = match v0 {
                Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s,
                    _ => return Err("substring: expected String".into()),
                },
                _ => return Err("substring: expected String".into()),
            };
            match (v1, v2) {
                (Value::Int(start), Value::Int(len)) => {
                    let start = usize::try_from(start)
                        .map_err(|_| "substring: negative start index".to_string())?;
                    let len = usize::try_from(len)
                        .map_err(|_| "substring: negative length".to_string())?;
                    let result: String = s.chars().skip(start).take(len).collect();
                    if result.chars().count() < len {
                        Err("substring: out of bounds".to_string())
                    } else {
                        Ok(Value::Heap(heap.alloc(HeapObject::String(result))))
                    }
                }
                _ => Err("substring: expected (String, Int, Int)".into()),
            }
        }
        fn bi_string_contains(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let (v0, v1) = match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::Tuple(t) => (t[0], t[1]),
                    _ => return Err("string_contains: expected (String, String)".into()),
                },
                _ => return Err("string_contains: expected (String, String)".into()),
            };
            let a = match v0 {
                Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s.as_str(),
                    _ => return Err("string_contains: expected String".into()),
                },
                _ => return Err("string_contains: expected String".into()),
            };
            let b = match v1 {
                Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s.as_str(),
                    _ => return Err("string_contains: expected String".into()),
                },
                _ => return Err("string_contains: expected String".into()),
            };
            Ok(Value::Bool(a.contains(b)))
        }
        fn bi_trim(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => Ok(Value::Heap(
                        heap.alloc(HeapObject::String(s.trim().to_string())),
                    )),
                    _ => Err("trim: expected String".into()),
                },
                _ => Err("trim: expected String".into()),
            }
        }
        fn bi_split(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let (v0, v1) = match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::Tuple(t) => (t[0], t[1]),
                    _ => return Err("split: expected (String, String)".into()),
                },
                _ => return Err("split: expected (String, String)".into()),
            };
            let s = match v0 {
                Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s.clone(),
                    _ => return Err("split: expected String".into()),
                },
                _ => return Err("split: expected String".into()),
            };
            let sep = match v1 {
                Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s.clone(),
                    _ => return Err("split: expected String".into()),
                },
                _ => return Err("split: expected String".into()),
            };
            let parts: Vec<Value> = s
                .split(&sep)
                .map(|p| Value::Heap(heap.alloc(HeapObject::String(p.to_string()))))
                .collect();
            let list = alloc_list(heap, parts);
            Ok(list)
        }
        fn bi_sqrt(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::Float(f.sqrt())),
                _ => Err("sqrt: expected Float".into()),
            }
        }
        fn bi_abs_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Int(n) => Ok(Value::Int(n.abs())),
                _ => Err("abs_int: expected Int".into()),
            }
        }
        fn bi_abs_float(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::Float(f.abs())),
                _ => Err("abs_float: expected Float".into()),
            }
        }
        fn bi_floor(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::Int(f.floor() as i64)),
                _ => Err("floor: expected Float".into()),
            }
        }
        fn bi_ceil(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::Int(f.ceil() as i64)),
                _ => Err("ceil: expected Float".into()),
            }
        }
        fn bi_read_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let path = match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s.clone(),
                    _ => return Err("read_file: expected String".into()),
                },
                _ => return Err("read_file: expected String".into()),
            };
            let contents = std::fs::read_to_string(&path).map_err(|e| format!("read_file: {e}"))?;
            Ok(Value::Heap(heap.alloc(HeapObject::String(contents))))
        }
        fn bi_write_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let (v0, v1) = match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::Tuple(t) => (t[0], t[1]),
                    _ => return Err("write_file: expected (String, String)".into()),
                },
                _ => return Err("write_file: expected (String, String)".into()),
            };
            let path = match v0 {
                Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s.clone(),
                    _ => return Err("write_file: expected String".into()),
                },
                _ => return Err("write_file: expected String".into()),
            };
            let contents = match v1 {
                Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s.clone(),
                    _ => return Err("write_file: expected String".into()),
                },
                _ => return Err("write_file: expected String".into()),
            };
            std::fs::write(&path, &contents).map_err(|e| format!("write_file: {e}"))?;
            Ok(Value::Unit)
        }
        fn bi_file_exists(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => {
                        Ok(Value::Bool(std::path::Path::new(s.as_str()).exists()))
                    }
                    _ => Err("file_exists: expected String".into()),
                },
                _ => Err("file_exists: expected String".into()),
            }
        }
        fn bi_list_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let path = match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s.clone(),
                    _ => return Err("list_dir: expected String".into()),
                },
                _ => return Err("list_dir: expected String".into()),
            };
            let entries: Vec<Value> = std::fs::read_dir(&path)
                .map_err(|e| format!("list_dir: {e}"))?
                .filter_map(|entry| {
                    entry.ok().map(|e| {
                        Value::Heap(heap.alloc(HeapObject::String(
                            e.file_name().to_string_lossy().to_string(),
                        )))
                    })
                })
                .collect();
            let list = alloc_list(heap, entries);
            Ok(list)
        }
        fn bi_remove_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => {
                        std::fs::remove_file(s.as_str())
                            .map_err(|e| format!("remove_file: {e}"))?;
                        Ok(Value::Unit)
                    }
                    _ => Err("remove_file: expected String".into()),
                },
                _ => Err("remove_file: expected String".into()),
            }
        }
        fn bi_create_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => {
                        std::fs::create_dir_all(s.as_str())
                            .map_err(|e| format!("create_dir: {e}"))?;
                        Ok(Value::Unit)
                    }
                    _ => Err("create_dir: expected String".into()),
                },
                _ => Err("create_dir: expected String".into()),
            }
        }
        fn bi_http_get(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let url = match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => s.clone(),
                    _ => return Err("http_get: expected String".into()),
                },
                _ => return Err("http_get: expected String".into()),
            };
            let response = ureq::get(&url)
                .call()
                .map_err(|e| format!("http_get: {e}"))?;

            let status = Value::Int(response.status().as_u16() as i64);

            // Collect headers as a list of (name, value) tuples
            let mut header_values: Vec<Value> = Vec::new();
            for name in response.headers().keys() {
                if let Some(val) = response.headers().get(name) {
                    let k = Value::Heap(heap.alloc(HeapObject::String(name.to_string())));
                    let v = Value::Heap(
                        heap.alloc(HeapObject::String(val.to_str().unwrap_or("").to_string())),
                    );
                    let pair = Value::Heap(heap.alloc(HeapObject::Tuple(vec![k, v])));
                    header_values.push(pair);
                }
            }
            let headers = alloc_list(heap, header_values);

            let body_str = response
                .into_body()
                .read_to_string()
                .map_err(|e| format!("http_get: {e}"))?;
            let body = Value::Heap(heap.alloc(HeapObject::String(body_str)));

            // Return (status, headers, body)
            Ok(Value::Heap(
                heap.alloc(HeapObject::Tuple(vec![status, headers, body])),
            ))
        }
        fn bi_exit(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Int(code) => std::process::exit(*code as i32),
                _ => Err("exit: expected Int".into()),
            }
        }
        fn bi_panic(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::String(s) => Err(s.clone()),
                    _ => Err("panic: expected String".into()),
                },
                _ => Err("panic: expected String".into()),
            }
        }
        fn bi_assert(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let (v0, v1) = match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::Tuple(t) => (t[0], t[1]),
                    _ => return Err("assert: expected (Bool, String)".into()),
                },
                _ => return Err("assert: expected (Bool, String)".into()),
            };
            match (v0, v1) {
                (Value::Bool(true), _) => Ok(Value::Unit),
                (Value::Bool(false), Value::Heap(r)) => {
                    match heap.get(r).map_err(|e| e.to_string())? {
                        HeapObject::String(msg) => Err(format!("assertion failed: {msg}")),
                        _ => Err("assertion failed".into()),
                    }
                }
                _ => Err("assert: expected (Bool, String)".into()),
            }
        }
        fn bi_assert_eq(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
            let (v0, v1, v2) = match &args[0] {
                Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
                    HeapObject::Tuple(t) if t.len() >= 3 => (t[0], t[1], t[2]),
                    _ => return Err("assert_eq: expected (a, a, String)".into()),
                },
                _ => return Err("assert_eq: expected (a, a, String)".into()),
            };
            let eq = values_equal(v0, v1, heap);
            if eq {
                Ok(Value::Unit)
            } else {
                let msg = match v2 {
                    Value::Heap(r) => match heap.get(r) {
                        Ok(HeapObject::String(s)) => s.clone(),
                        _ => String::new(),
                    },
                    _ => String::new(),
                };
                Err(format!(
                    "assertion failed: {msg}: expected {:?}, got {:?}",
                    v1, v0
                ))
            }
        }

        let entries: Vec<(&str, BuiltinFn)> = vec![
            ("print", bi_print),
            ("println", bi_println),
            ("read_line", bi_read_line),
            ("int_to_string", bi_int_to_string),
            ("float_to_string", bi_float_to_string),
            ("string_to_int", bi_string_to_int),
            ("char_to_int", bi_char_to_int),
            ("int_to_char", bi_int_to_char),
            ("int_to_float", bi_int_to_float),
            ("string_length", bi_string_length),
            ("substring", bi_substring),
            ("string_contains", bi_string_contains),
            ("trim", bi_trim),
            ("split", bi_split),
            ("sqrt", bi_sqrt),
            ("abs_int", bi_abs_int),
            ("abs_float", bi_abs_float),
            ("floor", bi_floor),
            ("ceil", bi_ceil),
            ("read_file", bi_read_file),
            ("write_file", bi_write_file),
            ("file_exists", bi_file_exists),
            ("list_dir", bi_list_dir),
            ("remove_file", bi_remove_file),
            ("create_dir", bi_create_dir),
            ("http_get", bi_http_get),
            ("exit", bi_exit),
            ("panic", bi_panic),
            ("assert", bi_assert),
            ("assert_eq", bi_assert_eq),
        ];
        for (i, (name, func)) in entries.into_iter().enumerate() {
            self.builtins.push(BuiltinEntry { name, func });
            let slot = self.global_slot(name.to_string());
            self.globals[slot] = Value::Builtin(i as u16);
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
        self.frames.push(CallFrame {
            proto_idx: usize::MAX,
            ip: 0,
            base: 0,
            captures: Rc::from([]),
        });
        self.dispatch()
    }

    pub fn get_global(&self, name: &str) -> Option<&Value> {
        self.global_names.get(name).map(|&slot| &self.globals[slot])
    }

    pub fn get_output(&self) -> &[String] {
        &self.output
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
        let func = self.builtins[builtin_id as usize].func;
        let args = &self.stack[callee_pos + 1..callee_pos + 1 + arity];
        let result = func(args, &mut self.heap).map_err(|msg| RuntimeError { message: msg })?;
        self.stack.truncate(callee_pos);
        if builtin_id == BUILTIN_PRINT || builtin_id == BUILTIN_PRINTLN {
            let displayed = if builtin_id == BUILTIN_PRINTLN {
                format!("{}\n", self.display_value(&first_arg))
            } else {
                self.display_value(&first_arg)
            };
            self.output.push(displayed);
            self.push(Value::Unit)?;
        } else {
            self.push(result)?;
        }
        Ok(())
    }

    // ── Dispatch loop ────────────────────────────────────────────────

    fn dispatch(&mut self) -> Result<(), RuntimeError> {
        loop {
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

                Op::EqFloat => self.float_cmp(|a, b| a == b)?,
                Op::NeFloat => self.float_cmp(|a, b| a != b)?,
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
                    let elems: Vec<Value> = self.stack.drain(start..).collect();
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
                    let fields: Vec<Value> = self.stack.drain(start..).collect();
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
                    let frame = &mut self.frames[fi];
                    frame.ip = (frame.ip as i64 + offset as i64) as usize;
                }
                Op::JumpIfFalse => {
                    let offset = self.read_i16()?;
                    let cond = self.pop_bool()?;
                    if !cond {
                        let frame = &mut self.frames[fi];
                        frame.ip = (frame.ip as i64 + offset as i64) as usize;
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
                        let val = if is_local {
                            let base = self.frames[fi].base;
                            self.stack[base + index]
                        } else {
                            self.frames[fi].captures[index]
                        };
                        captures.push(val);
                    }
                    let captures: Rc<[Value]> = Rc::from(captures);
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
                            if self.frames.len() >= MAX_FRAMES {
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
                    let n_clauses = self.read_u16()? as usize;
                    let mut clauses = Vec::with_capacity(n_clauses);
                    for _ in 0..n_clauses {
                        let effect_tag = self.read_u16()?;
                        let offset = self.read_i16()? as i64;
                        let abs_ip = (self.frames[fi].ip as i64 + offset) as usize;
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

                    let (h_idx, clause_ip) = self
                        .handlers
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(hi, h)| {
                            h.clauses
                                .iter()
                                .find(|(t, _)| *t == effect_tag)
                                .map(|(_, ip)| (hi, *ip))
                        })
                        .ok_or_else(|| RuntimeError {
                            message: format!("unhandled effect (tag {effect_tag})"),
                        })?;

                    let handler = self.handlers.remove(h_idx);
                    self.handlers.truncate(h_idx);

                    // Save stack from handler's stack_base (the point where
                    // InstallHandler ran). The handler frame's pre-existing
                    // locals below this point stay on the stack.
                    let save_from = handler.stack_base;
                    let saved_stack = self.stack.split_off(save_from);

                    // Save the handler frame's current IP/state so Resume
                    // can re-create it. Its base stays on the real stack
                    // (below save_from), but we record it with base_offset=0
                    // and restore it specially in Resume.
                    let hf = &self.frames[handler.call_frame_idx];
                    let handler_frame = SavedFrame {
                        proto_idx: hf.proto_idx,
                        ip: hf.ip,
                        base_offset: 0, // sentinel, handled specially in Resume
                        captures: hf.captures.clone(),
                    };

                    let mut saved_frames = vec![handler_frame];
                    for frame in &self.frames[handler.call_frame_idx + 1..] {
                        saved_frames.push(SavedFrame {
                            proto_idx: frame.proto_idx,
                            ip: frame.ip,
                            base_offset: {
                                debug_assert!(frame.base >= save_from);
                                frame.base - save_from
                            },
                            captures: frame.captures.clone(),
                        });
                    }

                    let cont = self.alloc(HeapObject::Continuation {
                        saved_frames,
                        saved_stack,
                    });

                    self.frames.truncate(handler.call_frame_idx + 1);

                    // Restore handler frame context and jump to clause
                    let hfi = self.frames.len() - 1;
                    self.frames[hfi].proto_idx = handler.proto_idx;
                    self.frames[hfi].captures = handler.captures;
                    self.frames[hfi].ip = clause_ip;

                    // Push payload and continuation for the clause to bind
                    self.push(payload)?;
                    self.push(cont)?;
                }

                Op::Resume => {
                    let arg = self.pop()?;
                    let cont_val = self.pop()?;
                    let cont_ref = match cont_val {
                        Value::Heap(r) => r,
                        _ => {
                            return Err(RuntimeError {
                                message: "resume: expected continuation".into(),
                            });
                        }
                    };
                    let (saved_frames, saved_stack) = match self.heap_get(cont_ref)? {
                        HeapObject::Continuation {
                            saved_frames,
                            saved_stack,
                            ..
                        } => (saved_frames.clone(), saved_stack.clone()),
                        _ => {
                            return Err(RuntimeError {
                                message: "resume: expected continuation".into(),
                            });
                        }
                    };

                    // Restore saved stack and frames. The handler clause
                    // frame stays; the resumed computation returns into it.
                    let handler_base = self.frames.last().unwrap().base;
                    let stack_base = self.stack.len();
                    self.stack.extend_from_slice(&saved_stack);

                    for (i, sf) in saved_frames.iter().enumerate() {
                        let frame_base = if i == 0 {
                            // First saved frame is the handler frame, so its
                            // base is the same as the clause frame's base
                            handler_base
                        } else {
                            stack_base + sf.base_offset
                        };
                        self.frames.push(CallFrame {
                            proto_idx: sf.proto_idx,
                            ip: sf.ip,
                            base: frame_base,
                            captures: sf.captures.clone(),
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
        if self.stack.len() >= MAX_STACK {
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
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;

    fn run(input: &str) -> VM {
        let tokens = Lexer::new(input, 0).tokenize().expect("lex error");
        let program = Parser::new(tokens).parse_program().expect("parse error");
        let (compiled, _warnings) = Compiler::compile(program).expect("compile error");
        let mut vm = VM::new(compiled);
        vm.run().expect("runtime error");
        vm
    }

    fn global_int(vm: &VM, name: &str) -> i64 {
        match vm.get_global(name).expect(&format!("no global: {name}")) {
            Value::Int(n) => *n,
            v => panic!("expected Int for {name}, got {v:?}"),
        }
    }

    fn global_bool(vm: &VM, name: &str) -> bool {
        match vm.get_global(name).expect(&format!("no global: {name}")) {
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

    // ── Effect handler tests ─────────────────────────────────────────

    #[test]
    fn test_effect_handle_no_perform() {
        // Body returns normally, goes through return clause
        let vm = run("effect Ask of Unit
             val result = handle 42 with return x => x + 1");
        assert_eq!(global_int(&vm, "result"), 43);
    }

    #[test]
    fn test_effect_perform_simple() {
        // Perform an effect, handler returns a value without resuming
        let vm = run("effect Ask of Unit
             val result = handle perform Ask ()
               with return x => x
                  | Ask _ k => 99");
        assert_eq!(global_int(&vm, "result"), 99);
    }

    #[test]
    fn test_effect_perform_with_resume() {
        // Perform + resume: the continuation returns the resumed value
        let vm = run("effect Ask of Unit
             val result = handle 1 + perform Ask ()
               with return x => x
                  | Ask _ k => resume k 41");
        assert_eq!(global_int(&vm, "result"), 42);
    }

    #[test]
    fn test_effect_perform_payload() {
        // Effect carries a payload
        let vm = run("effect Double of Int
             val result = handle perform Double 21
               with return x => x
                  | Double n k => resume k (n * 2)");
        assert_eq!(global_int(&vm, "result"), 42);
    }

    #[test]
    fn test_effect_generator() {
        let vm = run("effect Yield of Int
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
        let vm = run("effect Abort of Int
             fun f () = let val _ = perform Abort 42 in 0 end
             val result = handle f ()
               with return x => x
                  | Abort n _ => n");
        assert_eq!(global_int(&vm, "result"), 42);
    }

    #[test]
    fn test_effect_nested_handlers() {
        // Nested handle blocks with different effects
        let vm = run("effect A of Unit
             effect B of Unit
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
        let vm = run("effect Get of Unit
             effect Put of Int
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
}
