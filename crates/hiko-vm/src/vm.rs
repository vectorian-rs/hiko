use std::collections::HashMap;
use std::rc::Rc;

use hiko_compile::chunk::{Chunk, CompiledProgram, Constant};
use hiko_compile::op::Op;

use crate::value::{ClosureValue, DataValue, Value};

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
        fn bi_print(args: &[Value]) -> Result<Value, String> {
            Ok(Value::String(Rc::new(format!("{}", args[0]))))
        }
        fn bi_println(args: &[Value]) -> Result<Value, String> {
            Ok(Value::String(Rc::new(format!("{}\n", args[0]))))
        }
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
        fn bi_string_length(args: &[Value]) -> Result<Value, String> {
            match &args[0] {
                Value::String(s) => Ok(Value::Int(s.len() as i64)),
                _ => Err("string_length: expected String".into()),
            }
        }

        let builtins: &[(&str, crate::value::BuiltinFn)] = &[
            ("print", bi_print),
            ("println", bi_println),
            ("int_to_string", bi_int_to_string),
            ("float_to_string", bi_float_to_string),
            ("string_length", bi_string_length),
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
            let op = Op::from_byte(op_byte).ok_or_else(|| RuntimeError {
                message: format!("invalid opcode: {op_byte}"),
            })?;

            match op {
                Op::Halt => return Ok(()),

                Op::Const => {
                    let idx = self.read_u16() as usize;
                    let val = self.read_const(proto_idx, idx);
                    self.push(val)?;
                }
                Op::Unit => self.push(Value::Unit)?,
                Op::True => self.push(Value::Bool(true))?,
                Op::False => self.push(Value::Bool(false))?,

                Op::GetLocal => {
                    let slot = self.read_u16() as usize;
                    let base = self.frames[fi].base;
                    let val = self.stack[base + slot].clone();
                    self.push(val)?;
                }
                Op::SetLocal => {
                    let slot = self.read_u16() as usize;
                    let base = self.frames[fi].base;
                    let val = self.pop();
                    self.stack[base + slot] = val;
                }
                Op::GetUpvalue => {
                    let idx = self.read_u16() as usize;
                    let val = self.frames[fi].captures[idx].clone();
                    self.push(val)?;
                }
                Op::GetGlobal => {
                    let idx = self.read_u16() as usize;
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
                    let idx = self.read_u16() as usize;
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
                                message: "NegInt: expected number".into(),
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
                    let arity = self.read_u8() as usize;
                    let start = self.stack.len() - arity;
                    let elems: Vec<Value> = self.stack.drain(start..).collect();
                    self.push(Value::Tuple(Rc::new(elems)))?;
                }
                Op::GetField => {
                    let idx = self.read_u8() as usize;
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
                    let tag = self.read_u16();
                    let arity = self.read_u8() as usize;
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
                    let offset = self.read_i16();
                    let frame = &mut self.frames[fi];
                    frame.ip = (frame.ip as i64 + offset as i64) as usize;
                }
                Op::JumpIfFalse => {
                    let offset = self.read_i16();
                    let cond = self.pop_bool()?;
                    if !cond {
                        let frame = &mut self.frames[fi];
                        frame.ip = (frame.ip as i64 + offset as i64) as usize;
                    }
                }

                // ── Functions ───────────────────────────────────
                Op::MakeClosure => {
                    let proto_idx = self.read_u16() as usize;
                    let n_captures = self.read_u8() as usize;
                    let mut captures = Vec::with_capacity(n_captures);
                    for _ in 0..n_captures {
                        let is_local = self.read_u8() != 0;
                        let index = self.read_u16() as usize;
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
                    let arity = self.read_u8() as usize;
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
                        Value::Builtin { func, .. } => {
                            let args_start = callee_pos + 1;
                            let args: Vec<Value> =
                                self.stack[args_start..args_start + arity].to_vec();
                            let result =
                                func(&args).map_err(|msg| RuntimeError { message: msg })?;
                            // Handle println/print side effects
                            if let Value::String(ref s) = result
                                && !s.is_empty()
                            {
                                self.output.push((**s).clone());
                            }
                            // Pop callee + args, push result
                            self.stack.truncate(callee_pos);
                            self.push(result)?;
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

    fn read_u8(&mut self) -> u8 {
        let frame = self.frames.last_mut().unwrap();
        let chunk = if frame.proto_idx == usize::MAX {
            &self.main_chunk
        } else {
            &self.protos[frame.proto_idx].chunk
        };
        let val = chunk.code[frame.ip];
        frame.ip += 1;
        val
    }

    fn read_u16(&mut self) -> u16 {
        let frame = self.frames.last_mut().unwrap();
        let chunk = if frame.proto_idx == usize::MAX {
            &self.main_chunk
        } else {
            &self.protos[frame.proto_idx].chunk
        };
        let val = u16::from_le_bytes([chunk.code[frame.ip], chunk.code[frame.ip + 1]]);
        frame.ip += 2;
        val
    }

    fn read_i16(&mut self) -> i16 {
        let frame = self.frames.last_mut().unwrap();
        let chunk = if frame.proto_idx == usize::MAX {
            &self.main_chunk
        } else {
            &self.protos[frame.proto_idx].chunk
        };
        let val = i16::from_le_bytes([chunk.code[frame.ip], chunk.code[frame.ip + 1]]);
        frame.ip += 2;
        val
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
        let compiled = Compiler::compile(&program).expect("compile error");
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
}
