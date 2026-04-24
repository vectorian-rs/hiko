//! Bytecode dispatch loop and stack/operand helpers.

use super::*;

impl VM {
    pub(super) fn dispatch(&mut self) -> Result<(), RuntimeError> {
        loop {
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
            let chunk = self.chunk_for_checked(proto_idx)?;
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
                    let chunk = self.chunk_for_checked(proto_idx)?;
                    let val = match &chunk.constants[idx] {
                        Constant::Int(n) => Value::Int(*n),
                        Constant::Float(f) => Value::Float(*f),
                        Constant::String(s) => {
                            let key = (proto_idx, idx);
                            if let Some(&cached) = self.string_cache.get(&key) {
                                Value::Heap(cached)
                            } else {
                                let v = self.alloc_string(s.clone())?;
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
                    let name = self.read_const_string(proto_idx, idx)?;
                    let slot = *self.global_names.get(name).ok_or_else(|| RuntimeError {
                        message: format!("undefined global: {name}"),
                    })?;
                    let val = self.globals[slot];
                    self.push(val)?;
                }
                Op::SetGlobal => {
                    let idx = self.read_u16()? as usize;
                    let name = self.read_const_string(proto_idx, idx)?.to_string();
                    let val = self.pop()?;
                    let slot = self.global_slot(name);
                    self.globals[slot] = val;
                }

                Op::Pop => {
                    self.pop()?;
                }

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
                    let val = self.alloc_string(result)?;
                    self.push(val)?;
                }
                Op::Not => {
                    let b = self.pop_bool()?;
                    self.push(Value::Bool(!b))?;
                }

                Op::MakeTuple => {
                    let arity = self.read_u8()? as usize;
                    let start = self.stack.len() - arity;
                    let elems: Fields = self.stack.drain(start..).collect();
                    let val = self.alloc(HeapObject::Tuple(elems))?;
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
                    let val = self.alloc(HeapObject::Data { tag, fields })?;
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
                    })?;
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
                            let proto = self.proto(closure_proto)?;
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
                            let proto = self.proto(closure_proto)?;
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
                    let proto = self.proto(proto_idx)?;
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
                    let proto = self.proto(proto_idx)?;
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
                    let msg = self.read_const_string(proto_idx, idx)?.to_string();
                    return Err(RuntimeError { message: msg });
                }

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

                    let user_handler =
                        self.handlers.iter().enumerate().rev().find_map(|(hi, h)| {
                            h.clauses
                                .iter()
                                .find(|(t, _)| *t == effect_tag)
                                .map(|(_, ip)| (hi, *ip))
                        });

                    let (h_idx, clause_ip) = user_handler.ok_or_else(|| RuntimeError {
                        message: format!("unhandled effect (tag {effect_tag})"),
                    })?;

                    {
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
                        })?;

                        self.frames.truncate(handler.call_frame_idx + 1);

                        let hfi = self.frames.len() - 1;
                        self.frames[hfi].proto_idx = handler.proto_idx;
                        self.frames[hfi].captures = handler.captures;
                        self.frames[hfi].ip = clause_ip;

                        self.push(payload)?;
                        self.push(cont)?;
                    }
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

                    self.push(arg)?;
                }
            }
        }
    }

    pub(super) fn push(&mut self, val: Value) -> Result<(), RuntimeError> {
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

    fn current_code(&self) -> Result<&[u8], RuntimeError> {
        let proto_idx = self.frames.last().unwrap().proto_idx;
        Ok(&self.chunk_for_checked(proto_idx)?.code)
    }

    fn read_u8(&mut self) -> Result<u8, RuntimeError> {
        let fi = self.frames.len() - 1;
        let ip = self.frames[fi].ip;
        let code = self.current_code()?;
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
        let code = self.current_code()?;
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
