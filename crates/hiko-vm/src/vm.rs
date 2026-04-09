use std::collections::HashMap;
use std::rc::Rc;

use hiko_compile::chunk::{Chunk, CompiledProgram, Constant};
use hiko_compile::op::Op;

use crate::value::{ClosureValue, DataValue, Value};

/// Build a Hiko list (Cons/Nil) from a Vec of values.
fn build_list(items: Vec<Value>) -> Value {
    let mut list = Value::Data(Rc::new(DataValue {
        tag: 0,
        fields: vec![],
    })); // Nil
    for item in items.into_iter().rev() {
        list = Value::Data(Rc::new(DataValue {
            tag: 1,
            fields: vec![item, list],
        })); // Cons
    }
    list
}

const MAX_STACK: usize = 64 * 1024;
const MAX_FRAMES: usize = 1024;

#[derive(Debug)]
pub struct RuntimeError {
    pub message: String,
}

struct CallFrame {
    proto_idx: usize, // index into VM.protos (usize::MAX = main chunk)
    ip: usize,
    base: usize,
    captures: Vec<Value>,
}

pub struct VM {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    globals: HashMap<String, Value>,
    protos: Vec<hiko_compile::chunk::FunctionProto>,
    main_chunk: Chunk,
    output: Vec<String>,
}

impl VM {
    pub fn new(program: CompiledProgram) -> Self {
        let mut vm = VM {
            stack: Vec::with_capacity(256),
            frames: Vec::new(),
            globals: HashMap::new(),
            protos: program.functions,
            main_chunk: program.main,
            output: Vec::new(),
        };
        vm.register_builtins();
        vm
    }

    fn register_builtins(&mut self) {
        // ── I/O ──────────────────────────────────────────────────────
        fn bi_print(args: &[Value]) -> Result<Value, String> {
            Ok(Value::String(Rc::new(format!("{}", args[0]))))
        }
        fn bi_println(args: &[Value]) -> Result<Value, String> {
            Ok(Value::String(Rc::new(format!("{}\n", args[0]))))
        }
        fn bi_read_line(_args: &[Value]) -> Result<Value, String> {
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| format!("read_line: {e}"))?;
            if line.ends_with('\n') {
                line.pop();
            }
            Ok(Value::String(Rc::new(line)))
        }

        // ── Conversions ──────────────────────────────────────────────
        fn bi_int_to_string(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Int(n) => Ok(Value::String(Rc::new(n.to_string()))),
                _ => Err("int_to_string: expected Int".into()),
            }
        }
        fn bi_float_to_string(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::String(Rc::new(f.to_string()))),
                _ => Err("float_to_string: expected Float".into()),
            }
        }
        fn bi_string_to_int(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(s) => s
                    .trim()
                    .parse::<i64>()
                    .map(Value::Int)
                    .map_err(|e| format!("string_to_int: {e}")),
                _ => Err("string_to_int: expected String".into()),
            }
        }
        fn bi_char_to_int(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Char(c) => Ok(Value::Int(*c as i64)),
                _ => Err("char_to_int: expected Char".into()),
            }
        }
        fn bi_int_to_char(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Int(n) => char::from_u32(*n as u32)
                    .map(Value::Char)
                    .ok_or_else(|| format!("int_to_char: invalid codepoint {n}")),
                _ => Err("int_to_char: expected Int".into()),
            }
        }

        // ── String operations ────────────────────────────────────────
        fn bi_string_length(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(s) => Ok(Value::Int(s.chars().count() as i64)),
                _ => Err("string_length: expected String".into()),
            }
        }
        fn bi_substring(args: &[Value]) -> Result<Value, String> {
            let tup = match &args[0] {
                Value::Tuple(t) => t,
                _ => return Err("substring: expected (String, Int, Int)".into()),
            };
            match (&tup[0], &tup[1], &tup[2]) {
                (Value::String(s), Value::Int(start), Value::Int(len)) => {
                    let start = *start as usize;
                    let len = *len as usize;
                    let result: String = s.chars().skip(start).take(len).collect();
                    if result.chars().count() < len {
                        Err("substring: out of bounds".to_string())
                    } else {
                        Ok(Value::String(Rc::new(result)))
                    }
                }
                _ => Err("substring: expected (String, Int, Int)".into()),
            }
        }
        fn bi_string_contains(args: &[Value]) -> Result<Value, String> {
            let tup = match &args[0] {
                Value::Tuple(t) => t,
                _ => return Err("string_contains: expected (String, String)".into()),
            };
            match (&tup[0], &tup[1]) {
                (Value::String(haystack), Value::String(needle)) => {
                    Ok(Value::Bool(haystack.contains(needle.as_str())))
                }
                _ => Err("string_contains: expected (String, String)".into()),
            }
        }
        fn bi_trim(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(s) => Ok(Value::String(Rc::new(s.trim().to_string()))),
                _ => Err("trim: expected String".into()),
            }
        }
        fn bi_split(args: &[Value]) -> Result<Value, String> {
            let tup = match &args[0] {
                Value::Tuple(t) => t,
                _ => return Err("split: expected (String, String)".into()),
            };
            match (&tup[0], &tup[1]) {
                (Value::String(s), Value::String(sep)) => {
                    let parts: Vec<Value> = s
                        .split(sep.as_str())
                        .map(|p| Value::String(Rc::new(p.to_string())))
                        .collect();
                    Ok(build_list(parts))
                }
                _ => Err("split: expected (String, String)".into()),
            }
        }

        // ── Math ─────────────────────────────────────────────────────
        fn bi_sqrt(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::Float(f.sqrt())),
                _ => Err("sqrt: expected Float".into()),
            }
        }
        fn bi_abs_int(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Int(n) => Ok(Value::Int(n.abs())),
                _ => Err("abs_int: expected Int".into()),
            }
        }
        fn bi_abs_float(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::Float(f.abs())),
                _ => Err("abs_float: expected Float".into()),
            }
        }
        fn bi_floor(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::Int(f.floor() as i64)),
                _ => Err("floor: expected Float".into()),
            }
        }
        fn bi_ceil(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Float(f) => Ok(Value::Int(f.ceil() as i64)),
                _ => Err("ceil: expected Float".into()),
            }
        }
        fn bi_int_to_float(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Int(n) => Ok(Value::Float(*n as f64)),
                _ => Err("int_to_float: expected Int".into()),
            }
        }

        // ── Filesystem ───────────────────────────────────────────────
        fn bi_read_file(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(path) => {
                    let contents = std::fs::read_to_string(path.as_str())
                        .map_err(|e| format!("read_file: {e}"))?;
                    Ok(Value::String(Rc::new(contents)))
                }
                _ => Err("read_file: expected String".into()),
            }
        }
        fn bi_write_file(args: &[Value]) -> Result<Value, String> {
            let tup = match &args[0] {
                Value::Tuple(t) => t,
                _ => return Err("write_file: expected (String, String)".into()),
            };
            match (&tup[0], &tup[1]) {
                (Value::String(path), Value::String(contents)) => {
                    std::fs::write(path.as_str(), contents.as_str())
                        .map_err(|e| format!("write_file: {e}"))?;
                    Ok(Value::Unit)
                }
                _ => Err("write_file: expected (String, String)".into()),
            }
        }
        fn bi_file_exists(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(path) => {
                    Ok(Value::Bool(std::path::Path::new(path.as_str()).exists()))
                }
                _ => Err("file_exists: expected String".into()),
            }
        }
        fn bi_list_dir(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(path) => {
                    let entries: Vec<Value> = std::fs::read_dir(path.as_str())
                        .map_err(|e| format!("list_dir: {e}"))?
                        .filter_map(|entry| {
                            entry.ok().map(|e| {
                                Value::String(Rc::new(e.file_name().to_string_lossy().to_string()))
                            })
                        })
                        .collect();
                    Ok(build_list(entries))
                }
                _ => Err("list_dir: expected String".into()),
            }
        }
        fn bi_remove_file(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(path) => {
                    std::fs::remove_file(path.as_str()).map_err(|e| format!("remove_file: {e}"))?;
                    Ok(Value::Unit)
                }
                _ => Err("remove_file: expected String".into()),
            }
        }
        fn bi_create_dir(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(path) => {
                    std::fs::create_dir_all(path.as_str())
                        .map_err(|e| format!("create_dir: {e}"))?;
                    Ok(Value::Unit)
                }
                _ => Err("create_dir: expected String".into()),
            }
        }

        // ── HTTP ─────────────────────────────────────────────────────
        fn bi_http_get(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(url) => {
                    let body = ureq::get(url.as_str())
                        .call()
                        .map_err(|e| format!("http_get: {e}"))?
                        .into_body()
                        .read_to_string()
                        .map_err(|e| format!("http_get: {e}"))?;
                    Ok(Value::String(Rc::new(body)))
                }
                _ => Err("http_get: expected String".into()),
            }
        }

        // ── System ───────────────────────────────────────────────────
        fn bi_exit(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::Int(code) => std::process::exit(*code as i32),
                _ => Err("exit: expected Int".into()),
            }
        }
        fn bi_panic(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(msg) => Err(msg.to_string()),
                _ => Err("panic: expected String".into()),
            }
        }
        fn bi_assert(args: &[Value]) -> Result<Value, String> {
            let tup = match &args[0] {
                Value::Tuple(t) => t,
                _ => return Err("assert: expected (Bool, String)".into()),
            };
            match (&tup[0], &tup[1]) {
                (Value::Bool(true), _) => Ok(Value::Unit),
                (Value::Bool(false), Value::String(msg)) => Err(format!("assertion failed: {msg}")),
                _ => Err("assert: expected (Bool, String)".into()),
            }
        }
        fn bi_assert_eq(args: &[Value]) -> Result<Value, String> {
            let tup = match &args[0] {
                Value::Tuple(t) => t,
                _ => return Err("assert_eq: expected (a, a, String)".into()),
            };
            if tup.len() < 3 {
                return Err("assert_eq: expected (a, a, String)".into());
            }
            let eq = match (&tup[0], &tup[1]) {
                (Value::Int(a), Value::Int(b)) => a == b,
                (Value::Float(a), Value::Float(b)) => a == b,
                (Value::Bool(a), Value::Bool(b)) => a == b,
                (Value::Char(a), Value::Char(b)) => a == b,
                (Value::String(a), Value::String(b)) => a == b,
                (Value::Unit, Value::Unit) => true,
                _ => false,
            };
            if eq {
                Ok(Value::Unit)
            } else {
                let msg = match &tup[2] {
                    Value::String(s) => s.to_string(),
                    _ => "".to_string(),
                };
                Err(format!(
                    "assertion failed: {msg}: expected {:?}, got {:?}",
                    tup[1], tup[0]
                ))
            }
        }

        let builtins: &[(&str, crate::value::BuiltinFn)] = &[
            // I/O
            ("print", bi_print),
            ("println", bi_println),
            ("read_line", bi_read_line),
            // Conversions
            ("int_to_string", bi_int_to_string),
            ("float_to_string", bi_float_to_string),
            ("string_to_int", bi_string_to_int),
            ("char_to_int", bi_char_to_int),
            ("int_to_char", bi_int_to_char),
            ("int_to_float", bi_int_to_float),
            // String ops
            ("string_length", bi_string_length),
            ("substring", bi_substring),
            ("string_contains", bi_string_contains),
            ("trim", bi_trim),
            ("split", bi_split),
            // Math
            ("sqrt", bi_sqrt),
            ("abs_int", bi_abs_int),
            ("abs_float", bi_abs_float),
            ("floor", bi_floor),
            ("ceil", bi_ceil),
            // Filesystem
            ("read_file", bi_read_file),
            ("write_file", bi_write_file),
            ("file_exists", bi_file_exists),
            ("list_dir", bi_list_dir),
            ("remove_file", bi_remove_file),
            ("create_dir", bi_create_dir),
            // HTTP
            ("http_get", bi_http_get),
            // System
            ("exit", bi_exit),
            ("panic", bi_panic),
            ("assert", bi_assert),
            ("assert_eq", bi_assert_eq),
        ];
        for &(name, func) in builtins {
            self.globals
                .insert(name.into(), Value::Builtin { name, func });
        }
    }

    pub fn run(&mut self) -> Result<(), RuntimeError> {
        self.frames.push(CallFrame {
            proto_idx: usize::MAX,
            ip: 0,
            base: 0,
            captures: Vec::new(),
        });
        self.dispatch()
    }

    /// Get the source span for the most recent error (call after run() returns Err).
    pub fn error_span(&self) -> Option<hiko_syntax::span::Span> {
        let frame = self.frames.last()?;
        let chunk = self.chunk_for(frame.proto_idx);
        chunk.span_at(frame.ip.saturating_sub(1))
    }

    pub fn get_global(&self, name: &str) -> Option<&Value> {
        self.globals.get(name)
    }

    pub fn get_output(&self) -> &[String] {
        &self.output
    }

    fn chunk_for(&self, proto_idx: usize) -> &Chunk {
        if proto_idx == usize::MAX {
            &self.main_chunk
        } else {
            &self.protos[proto_idx].chunk
        }
    }

    fn read_const(&self, proto_idx: usize, idx: usize) -> Value {
        match &self.chunk_for(proto_idx).constants[idx] {
            Constant::Int(n) => Value::Int(*n),
            Constant::Float(f) => Value::Float(*f),
            Constant::String(s) => Value::String(Rc::new(s.clone())),
            Constant::Char(c) => Value::Char(*c),
        }
    }

    fn read_const_string(&self, proto_idx: usize, idx: usize) -> &str {
        match &self.chunk_for(proto_idx).constants[idx] {
            Constant::String(s) => s,
            _ => panic!("expected string constant"),
        }
    }

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
                    let val = self.read_const(proto_idx, idx);
                    self.push(val)?;
                }
                Op::Unit => self.push(Value::Unit)?,
                Op::True => self.push(Value::Bool(true))?,
                Op::False => self.push(Value::Bool(false))?,

                Op::GetLocal => {
                    let slot = self.read_u16()? as usize;
                    let base = self.frames[fi].base;
                    let val = self.stack[base + slot].clone();
                    self.push(val)?;
                }
                Op::SetLocal => {
                    let slot = self.read_u16()? as usize;
                    let base = self.frames[fi].base;
                    let val = self.pop();
                    self.stack[base + slot] = val;
                }
                Op::GetUpvalue => {
                    let idx = self.read_u16()? as usize;
                    let val = self.frames[fi].captures[idx].clone();
                    self.push(val)?;
                }
                Op::GetGlobal => {
                    let idx = self.read_u16()? as usize;
                    let name = self.read_const_string(proto_idx, idx);
                    let val = self
                        .globals
                        .get(name)
                        .cloned()
                        .ok_or_else(|| RuntimeError {
                            message: format!("undefined global: {name}"),
                        })?;
                    self.push(val)?;
                }
                Op::SetGlobal => {
                    let idx = self.read_u16()? as usize;
                    let name = self.read_const_string(proto_idx, idx).to_string();
                    let val = self.pop();
                    self.globals.insert(name, val);
                }

                Op::Pop => {
                    self.pop();
                }

                // ── Arithmetic ──────────────────────────────────
                Op::AddInt => self.int_binop(|a, b| a + b)?,
                Op::SubInt => self.int_binop(|a, b| a - b)?,
                Op::MulInt => self.int_binop(|a, b| a * b)?,
                Op::DivInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    if b == 0 {
                        return Err(RuntimeError {
                            message: "division by zero".into(),
                        });
                    }
                    self.push(Value::Int(a / b))?;
                }
                Op::ModInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    if b == 0 {
                        return Err(RuntimeError {
                            message: "mod by zero".into(),
                        });
                    }
                    self.push(Value::Int(a % b))?;
                }
                Op::NegInt => {
                    let val = self.pop();
                    match val {
                        Value::Int(n) => self.push(Value::Int(-n))?,
                        Value::Float(f) => self.push(Value::Float(-f))?,
                        _ => {
                            return Err(RuntimeError {
                                message: "NegInt: expected Int or Float".into(),
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
                Op::EqInt => self.scalar_eq(true)?,
                Op::NeInt => self.scalar_eq(false)?,
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

                Op::EqBool => {
                    let b = self.pop_bool()?;
                    let a = self.pop_bool()?;
                    self.push(Value::Bool(a == b))?;
                }
                Op::NeBool => {
                    let b = self.pop_bool()?;
                    let a = self.pop_bool()?;
                    self.push(Value::Bool(a != b))?;
                }
                Op::EqChar => {
                    let b = self.pop_char()?;
                    let a = self.pop_char()?;
                    self.push(Value::Bool(a == b))?;
                }
                Op::NeChar => {
                    let b = self.pop_char()?;
                    let a = self.pop_char()?;
                    self.push(Value::Bool(a != b))?;
                }
                Op::EqString => {
                    let b = self.pop_string()?;
                    let a = self.pop_string()?;
                    self.push(Value::Bool(a == b))?;
                }
                Op::NeString => {
                    let b = self.pop_string()?;
                    let a = self.pop_string()?;
                    self.push(Value::Bool(a != b))?;
                }

                Op::ConcatString => {
                    let b = self.pop_string()?;
                    let a = self.pop_string()?;
                    let mut result = (*a).clone();
                    result.push_str(&b);
                    self.push(Value::String(Rc::new(result)))?;
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
                    self.push(Value::Tuple(Rc::new(elems)))?;
                }
                Op::GetField => {
                    let idx = self.read_u8()? as usize;
                    let val = self.pop();
                    match val {
                        Value::Tuple(t) => self.push(t[idx].clone())?,
                        Value::Data(d) => self.push(d.fields[idx].clone())?,
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
                    self.push(Value::Data(Rc::new(DataValue { tag, fields })))?;
                }
                Op::GetTag => {
                    let val = self.pop();
                    if let Value::Data(d) = val {
                        self.push(Value::Int(d.tag as i64))?;
                    } else {
                        return Err(RuntimeError {
                            message: "GetTag: expected data value".into(),
                        });
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
                    let proto_idx = self.read_u16()? as usize;
                    let n_captures = self.read_u8()? as usize;
                    let mut captures = Vec::with_capacity(n_captures);
                    for _ in 0..n_captures {
                        let is_local = self.read_u8()? != 0;
                        let index = self.read_u16()? as usize;
                        let val = if is_local {
                            let base = self.frames[fi].base;
                            self.stack[base + index].clone()
                        } else {
                            self.frames[fi].captures[index].clone()
                        };
                        captures.push(val);
                    }
                    self.push(Value::Closure(Rc::new(ClosureValue {
                        proto_idx,
                        captures,
                    })))?;
                }

                Op::Call => {
                    let arity = self.read_u8()? as usize;
                    let callee_pos = self.stack.len() - 1 - arity;
                    let callee = self.stack[callee_pos].clone();
                    match callee {
                        Value::Closure(closure) => {
                            let proto = &self.protos[closure.proto_idx];
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
                            self.stack.remove(callee_pos);
                            self.frames.push(CallFrame {
                                proto_idx: closure.proto_idx,
                                ip: 0,
                                base: callee_pos,
                                captures: closure.captures.clone(),
                            });
                        }
                        Value::Builtin { func, name, .. } => {
                            self.call_builtin(func, name, callee_pos, arity)?;
                        }
                        _ => {
                            return Err(RuntimeError {
                                message: format!("cannot call non-function: {callee:?}"),
                            });
                        }
                    }
                }

                Op::Return => {
                    let result = self.pop();
                    let frame = self.frames.pop().unwrap();
                    self.stack.truncate(frame.base);
                    self.push(result)?;
                    if self.frames.is_empty() {
                        return Ok(());
                    }
                }

                Op::TailCall => {
                    let arity = self.read_u8()? as usize;
                    let callee_pos = self.stack.len() - 1 - arity;
                    let callee = self.stack[callee_pos].clone();
                    match callee {
                        Value::Closure(closure) => {
                            let proto = &self.protos[closure.proto_idx];
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
                            // Copy new args over the current frame's locals
                            let args_start = callee_pos + 1;
                            for i in 0..arity {
                                self.stack[base + i] = self.stack[args_start + i].clone();
                            }
                            self.stack.truncate(base + arity);
                            self.frames[fi].ip = 0;
                            self.frames[fi].proto_idx = closure.proto_idx;
                            self.frames[fi].captures = closure.captures.clone();
                        }
                        Value::Builtin { func, name, .. } => {
                            self.call_builtin(func, name, callee_pos, arity)?;
                        }
                        _ => {
                            return Err(RuntimeError {
                                message: "tail call: expected function".into(),
                            });
                        }
                    }
                }

                Op::Panic => {
                    let idx = self.read_u16()? as usize;
                    let msg = self.read_const_string(proto_idx, idx).to_string();
                    return Err(RuntimeError { message: msg });
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

    fn pop(&mut self) -> Value {
        self.stack.pop().expect("stack underflow")
    }

    fn pop_int(&mut self) -> Result<i64, RuntimeError> {
        match self.pop() {
            Value::Int(n) => Ok(n),
            v => Err(RuntimeError {
                message: format!("expected Int, got {v:?}"),
            }),
        }
    }

    fn pop_float(&mut self) -> Result<f64, RuntimeError> {
        match self.pop() {
            Value::Float(f) => Ok(f),
            v => Err(RuntimeError {
                message: format!("expected Float, got {v:?}"),
            }),
        }
    }

    fn scalar_eq(&mut self, eq: bool) -> Result<(), RuntimeError> {
        let b = self.pop();
        let a = self.pop();
        let result = match (&a, &b) {
            (Value::Int(x), Value::Int(y)) => x == y,
            (Value::Float(x), Value::Float(y)) => x == y,
            (Value::Bool(x), Value::Bool(y)) => x == y,
            (Value::Char(x), Value::Char(y)) => x == y,
            (Value::String(x), Value::String(y)) => x == y,
            _ => {
                return Err(RuntimeError {
                    message: format!("cannot compare {a:?} and {b:?}"),
                });
            }
        };
        self.push(Value::Bool(if eq { result } else { !result }))
    }

    fn pop_bool(&mut self) -> Result<bool, RuntimeError> {
        match self.pop() {
            Value::Bool(b) => Ok(b),
            v => Err(RuntimeError {
                message: format!("expected Bool, got {v:?}"),
            }),
        }
    }

    fn pop_char(&mut self) -> Result<char, RuntimeError> {
        match self.pop() {
            Value::Char(c) => Ok(c),
            v => Err(RuntimeError {
                message: format!("expected Char, got {v:?}"),
            }),
        }
    }

    fn pop_string(&mut self) -> Result<Rc<String>, RuntimeError> {
        match self.pop() {
            Value::String(s) => Ok(s),
            v => Err(RuntimeError {
                message: format!("expected String, got {v:?}"),
            }),
        }
    }

    fn int_binop(&mut self, f: impl Fn(i64, i64) -> i64) -> Result<(), RuntimeError> {
        let b = self.pop_int()?;
        let a = self.pop_int()?;
        self.push(Value::Int(f(a, b)))
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

    fn call_builtin(
        &mut self,
        func: crate::value::BuiltinFn,
        name: &str,
        callee_pos: usize,
        arity: usize,
    ) -> Result<(), RuntimeError> {
        let args_start = callee_pos + 1;
        let args: Vec<Value> = self.stack[args_start..args_start + arity].to_vec();
        let result = func(&args).map_err(|msg| RuntimeError { message: msg })?;
        self.stack.truncate(callee_pos);
        if matches!(name, "print" | "println") {
            if let Value::String(ref s) = result {
                self.output.push((**s).clone());
            }
            self.push(Value::Unit)?;
        } else {
            self.push(result)?;
        }
        Ok(())
    }

    fn current_code(&self) -> &[u8] {
        let frame = self.frames.last().unwrap();
        if frame.proto_idx == usize::MAX {
            &self.main_chunk.code
        } else {
            &self.protos[frame.proto_idx].chunk.code
        }
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
        let fi = self.frames.len() - 1;
        let ip = self.frames[fi].ip;
        let code = self.current_code();
        if ip + 1 >= code.len() {
            return Err(RuntimeError {
                message: "truncated bytecode: expected i16 operand".into(),
            });
        }
        let val = i16::from_le_bytes([code[ip], code[ip + 1]]);
        self.frames[fi].ip += 2;
        Ok(val)
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
        let (compiled, _warnings) = Compiler::compile(&program).expect("compile error");
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
    fn test_comparison() {
        let vm = run("val a = 1 < 2 val b = 2 < 1");
        assert!(global_bool(&vm, "a"));
        assert!(!global_bool(&vm, "b"));
    }

    #[test]
    fn test_negation() {
        let vm = run("val x = ~42");
        assert_eq!(global_int(&vm, "x"), -42);
    }

    #[test]
    fn test_bool_ops() {
        let vm = run("val a = true andalso false val b = false orelse true");
        assert!(!global_bool(&vm, "a"));
        assert!(global_bool(&vm, "b"));
    }

    #[test]
    fn test_string_concat() {
        let vm = run(r#"val s = "hello" ^ " " ^ "world""#);
        match vm.get_global("s").unwrap() {
            Value::String(s) => assert_eq!(&**s, "hello world"),
            v => panic!("expected String, got {v:?}"),
        }
    }

    #[test]
    fn test_tuple() {
        let vm = run("val t = (1, 2, 3)");
        match vm.get_global("t").unwrap() {
            Value::Tuple(t) => assert_eq!(t.len(), 3),
            v => panic!("expected Tuple, got {v:?}"),
        }
    }

    #[test]
    fn test_list() {
        let vm = run("val xs = [1, 2, 3]");
        // Should be Cons(1, Cons(2, Cons(3, Nil)))
        match vm.get_global("xs").unwrap() {
            Value::Data(d) => {
                assert_eq!(d.tag, 1); // Cons
                assert_eq!(d.fields.len(), 2);
            }
            v => panic!("expected Data, got {v:?}"),
        }
    }

    #[test]
    fn test_builtin_int_to_string() {
        let vm = run("val s = int_to_string 42");
        match vm.get_global("s").unwrap() {
            Value::String(s) => assert_eq!(&**s, "42"),
            v => panic!("expected String, got {v:?}"),
        }
    }

    #[test]
    fn test_val_rec() {
        let vm = run(
            "val rec fact = fn n => if n = 0 then 1 else n * fact (n - 1)
             val result = fact 10",
        );
        assert_eq!(global_int(&vm, "result"), 3628800);
    }

    #[test]
    fn test_wildcard_binding() {
        let vm = run("val _ = 42 val x = 1");
        assert_eq!(global_int(&vm, "x"), 1);
    }

    #[test]
    fn test_unit() {
        let vm = run("val u = ()");
        assert!(matches!(vm.get_global("u").unwrap(), Value::Unit));
    }

    // ── Phase 4: ADTs + Pattern Matching ─────────────────────────────

    #[test]
    fn test_nullary_constructor() {
        let vm = run("datatype color = Red | Blue val x = Red");
        match vm.get_global("x").unwrap() {
            Value::Data(d) => assert_eq!(d.tag, 0),
            v => panic!("expected Data, got {v:?}"),
        }
    }

    #[test]
    fn test_unary_constructor() {
        let vm = run("datatype 'a option = None | Some of 'a val x = Some 42");
        match vm.get_global("x").unwrap() {
            Value::Data(d) => {
                assert_eq!(d.tag, 1);
                assert!(matches!(&d.fields[0], Value::Int(42)));
            }
            v => panic!("expected Data, got {v:?}"),
        }
    }

    #[test]
    fn test_case_simple() {
        let vm = run("datatype 'a option = None | Some of 'a
             val x = case Some 42 of None => 0 | Some n => n");
        assert_eq!(global_int(&vm, "x"), 42);
    }

    #[test]
    fn test_case_none() {
        let vm = run("datatype 'a option = None | Some of 'a
             val x = case None of None => 99 | Some n => n");
        assert_eq!(global_int(&vm, "x"), 99);
    }

    #[test]
    fn test_case_list() {
        let vm = run("fun hd xs = case xs of x :: _ => x | [] => 0
             val x = hd [42, 1, 2]");
        assert_eq!(global_int(&vm, "x"), 42);
    }

    #[test]
    fn test_case_empty_list() {
        let vm = run("fun hd xs = case xs of x :: _ => x | [] => 0
             val x = hd []");
        assert_eq!(global_int(&vm, "x"), 0);
    }

    #[test]
    fn test_clausal_fun() {
        let vm = run("fun fact 0 = 1
               | fact n = n * fact (n - 1)
             val result = fact 10");
        assert_eq!(global_int(&vm, "result"), 3628800);
    }

    #[test]
    fn test_map() {
        let vm = run("fun map f xs = case xs of
                [] => []
              | x :: xs => f x :: map f xs
             val result = map (fn x => x * 2) [1, 2, 3]");
        // result should be [2, 4, 6] = Cons(2, Cons(4, Cons(6, Nil)))
        match vm.get_global("result").unwrap() {
            Value::Data(d) => {
                assert_eq!(d.tag, 1); // Cons
                assert!(matches!(&d.fields[0], Value::Int(2)));
            }
            v => panic!("expected Data, got {v:?}"),
        }
    }

    #[test]
    fn test_filter() {
        let vm = run("fun filter p xs = case xs of
                [] => []
              | x :: xs => if p x then x :: filter p xs else filter p xs
             fun length xs = case xs of [] => 0 | _ :: xs => 1 + length xs
             val result = length (filter (fn x => x > 2) [1, 2, 3, 4, 5])");
        assert_eq!(global_int(&vm, "result"), 3);
    }

    #[test]
    fn test_foldl() {
        // Simpler: use a different var name to avoid shadowing confusion
        let vm = run("fun foldl f init xs = case xs of
                [] => init
              | x :: rest => foldl f (f (init, x)) rest
             val result = foldl (fn (a, b) => a + b) 0 [1, 2, 3, 4, 5]");
        assert_eq!(global_int(&vm, "result"), 15);
    }

    #[test]
    fn test_option_map() {
        let vm = run("datatype 'a option = None | Some of 'a
             fun map_opt f opt = case opt of
                None => None
              | Some x => Some (f x)
             val result = map_opt (fn x => x + 1) (Some 41)
             val n = case result of None => 0 | Some x => x");
        assert_eq!(global_int(&vm, "n"), 42);
    }

    #[test]
    fn test_nested_pattern() {
        let vm = run("datatype 'a option = None | Some of 'a
             fun get_or opt d = case opt of Some x => x | None => d
             val x = get_or (Some 42) 0");
        assert_eq!(global_int(&vm, "x"), 42);
    }

    #[test]
    fn test_tuple_pattern_in_case() {
        let vm = run("val x = case (1, 2) of (a, b) => a + b");
        assert_eq!(global_int(&vm, "x"), 3);
    }

    #[test]
    fn test_tuple_destructure_val() {
        let vm = run("val (x, y) = (10, 20) val z = x + y");
        assert_eq!(global_int(&vm, "z"), 30);
    }

    #[test]
    fn test_3arg_simple() {
        let vm = run("fun f a b c = a + b + c val result = f 1 2 3");
        assert_eq!(global_int(&vm, "result"), 6);
    }

    #[test]
    fn test_3arg_case() {
        let vm = run("fun f a b xs = case xs of [] => a + b | x :: _ => a + b + x
             val result = f 10 20 [3]");
        assert_eq!(global_int(&vm, "result"), 33);
    }

    #[test]
    fn test_expr_eval() {
        let vm = run(
            "datatype expr = Num of Int | Add of expr * expr | Mul of expr * expr
             fun eval e = case e of
                Num n => n
              | Add (a, b) => eval a + eval b
              | Mul (a, b) => eval a * eval b
             val result = eval (Add (Num 1, Mul (Num 2, Num 3)))",
        );
        assert_eq!(global_int(&vm, "result"), 7);
    }

    // ── Tail-call optimization ───────────────────────────────────────

    #[test]
    fn test_tco_loop() {
        // This would stack overflow without TCO (100K iterations)
        let vm = run("fun loop n = if n = 0 then 42 else loop (n - 1)
             val result = loop 100000");
        assert_eq!(global_int(&vm, "result"), 42);
    }

    #[test]
    fn test_tco_accumulator() {
        // Tail-recursive sum
        let vm = run(
            "fun sum_acc acc n = if n = 0 then acc else sum_acc (acc + n) (n - 1)
             val result = sum_acc 0 10000",
        );
        assert_eq!(global_int(&vm, "result"), 50005000);
    }

    #[test]
    fn test_tco_case() {
        // TCO through case branches (100K iterations)
        let vm = run(
            "fun count_down n = case n of 0 => 42 | _ => count_down (n - 1)
             val result = count_down 100000",
        );
        assert_eq!(global_int(&vm, "result"), 42);
    }
}
