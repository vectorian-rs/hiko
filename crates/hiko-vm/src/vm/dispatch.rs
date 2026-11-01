//! Bytecode dispatch loop and stack/operand helpers.

use super::*;

/// Largest `ConcatString` buffer a VM keeps between concatenations.
const CONCAT_BUF_KEEP_BYTES: usize = 4096;

/// A member of a `MakeRecClosures` group that captures other members.
struct RecClosurePatch {
    /// Position of the closure in the group.
    member: usize,
    /// Its captures, with `Unit` where a group member goes.
    captures: Vec<Value>,
    /// `(capture index, group position)` for each group capture.
    group_refs: Vec<(usize, usize)>,
}

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

            let fi = self.current_frame_index()?;
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
                    let constant = self
                        .chunk_for_checked(proto_idx)?
                        .constants
                        .get(idx)
                        .ok_or_else(|| RuntimeError {
                            message: format!("Const: constant index {idx} out of bounds"),
                        })?;
                    let val = match constant {
                        Constant::Int(n) => Value::Int(*n),
                        Constant::Float(f) => Value::Float(*f),
                        Constant::Char(c) => Value::Char(*c),
                        Constant::Word(w) => Value::Word(*w),
                        Constant::String(s) => {
                            let key = (proto_idx, idx);
                            if let Some(&cached) = self.string_cache.get(&key) {
                                Value::Heap(cached)
                            } else {
                                // Only clone the constant on the first load.
                                let s = s.clone();
                                let v = self.alloc_string(s)?;
                                if let Value::Heap(r) = v {
                                    self.string_cache.insert(key, r);
                                }
                                v
                            }
                        }
                    };
                    self.push(val)?;
                }
                Op::Unit => self.push(Value::Unit)?,
                Op::True => self.push(Value::Bool(true))?,
                Op::False => self.push(Value::Bool(false))?,

                Op::GetLocal => {
                    let slot = self.read_u16()? as usize;
                    let idx = self.local_stack_index(fi, slot, "GetLocal")?;
                    let val = self.stack[idx];
                    self.push(val)?;
                }
                Op::GetLocal0 | Op::GetLocal1 | Op::GetLocal2 | Op::GetLocal3 => {
                    let slot = match op {
                        Op::GetLocal0 => 0,
                        Op::GetLocal1 => 1,
                        Op::GetLocal2 => 2,
                        Op::GetLocal3 => 3,
                        _ => unreachable!(),
                    };
                    let idx = self.local_stack_index(fi, slot, "GetLocal")?;
                    let val = self.stack[idx];
                    self.push(val)?;
                }
                Op::SetLocal => {
                    let slot = self.read_u16()? as usize;
                    let idx = self.local_stack_index(fi, slot, "SetLocal")?;
                    let val = self.pop()?;
                    self.stack[idx] = val;
                }
                Op::SetLocal0 | Op::SetLocal1 | Op::SetLocal2 | Op::SetLocal3 => {
                    let slot = match op {
                        Op::SetLocal0 => 0,
                        Op::SetLocal1 => 1,
                        Op::SetLocal2 => 2,
                        Op::SetLocal3 => 3,
                        _ => unreachable!(),
                    };
                    let idx = self.local_stack_index(fi, slot, "SetLocal")?;
                    let val = self.pop()?;
                    self.stack[idx] = val;
                }
                Op::GetUpvalue => {
                    let idx = self.read_u16()? as usize;
                    let val =
                        self.frames[fi]
                            .captures
                            .get(idx)
                            .copied()
                            .ok_or_else(|| RuntimeError {
                                message: format!("GetUpvalue: capture index {idx} out of bounds"),
                            })?;
                    self.push(val)?;
                }
                Op::GetGlobal => {
                    let slot = self.read_u16()? as usize;
                    let val = match self.globals.get(slot) {
                        Some(&Some(val)) => val,
                        _ => self.load_global(slot)?,
                    };
                    self.push(val)?;
                }
                Op::SetGlobal => {
                    let slot = self.read_u16()? as usize;
                    let val = self.pop()?;
                    *self.global_mut(slot) = Some(val);
                    self.note_global_defined(slot);
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
                        Value::Word(_) => {
                            return Err(RuntimeError {
                                message: "cannot negate word value".into(),
                            });
                        }
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

                Op::AddWord => self.word_binop(u64::wrapping_add)?,
                Op::SubWord => self.word_binop(u64::wrapping_sub)?,
                Op::MulWord => self.word_binop(u64::wrapping_mul)?,
                Op::DivWord => {
                    self.word_checked_binop(u64::checked_div, "word division by zero")?
                }
                Op::ModWord => self.word_checked_binop(
                    |a, b| if b == 0 { None } else { Some(a % b) },
                    "word mod by zero",
                )?,
                Op::LtWord => self.word_cmp(|a, b| a < b)?,
                Op::GtWord => self.word_cmp(|a, b| a > b)?,
                Op::LeWord => self.word_cmp(|a, b| a <= b)?,
                Op::GeWord => self.word_cmp(|a, b| a >= b)?,

                Op::ConcatString => {
                    let b_ref = self.pop_string_ref()?;
                    let a_ref = self.pop_string_ref()?;
                    // An `Arc<str>` cannot be built in place, so building a
                    // `String` and converting it would allocate twice.
                    let mut buf = std::mem::take(&mut self.concat_buf);
                    buf.clear();
                    for r in [a_ref, b_ref] {
                        if let HeapObject::String(s) = self.heap_get(r)? {
                            buf.push_str(s);
                        }
                    }
                    let val = self.alloc(HeapObject::String(Arc::from(buf.as_str())));
                    // Keep a small buffer for the next concatenation, but do
                    // not hold on to a large one outside the heap's accounting.
                    if buf.capacity() <= CONCAT_BUF_KEEP_BYTES {
                        self.concat_buf = buf;
                    }
                    self.push(val?)?;
                }
                Op::Not => {
                    let b = self.pop_bool()?;
                    self.push(Value::Bool(!b))?;
                }

                Op::MakeTuple => {
                    let arity = self.read_u8()? as usize;
                    let start = self.stack_start_for_arity(arity, "MakeTuple")?;
                    let elems: Fields = self.stack.drain(start..).collect();
                    let val = self.alloc(HeapObject::Tuple(elems))?;
                    self.push(val)?;
                }
                Op::GetField => {
                    let idx = self.read_u8()? as usize;
                    self.push_field(idx)?;
                }
                Op::GetField0 => self.push_field(0)?,
                Op::GetField1 => self.push_field(1)?,
                Op::MakeData => {
                    let tag = self.read_u16()?;
                    let arity = self.read_u8()? as usize;
                    let start = self.stack_start_for_arity(arity, "MakeData")?;
                    let fields: Fields = self.stack.drain(start..).collect();
                    let val = self.alloc(HeapObject::Data { tag, fields })?;
                    self.push(val)?;
                }
                Op::MakeCons => {
                    let start = self.stack_start_for_arity(2, "MakeCons")?;
                    let fields: Fields = self.stack.drain(start..).collect();
                    let val = self.alloc(HeapObject::Data { tag: 1, fields })?;
                    self.push(val)?;
                }
                Op::Jump => {
                    let offset = self.read_i16()?;
                    let ip = self.frames[fi].ip;
                    let target = self.checked_relative_ip(proto_idx, ip, offset, "Jump")?;
                    self.frames[fi].ip = target;
                }
                Op::JumpIfNotTag => {
                    let tag = self.read_u16()?;
                    let offset = self.read_i16()?;
                    let actual = match self.pop()? {
                        Value::Heap(r) => match self.heap_get(r)? {
                            HeapObject::Data { tag, .. } => Some(*tag),
                            _ => None,
                        },
                        _ => None,
                    };
                    let Some(actual) = actual else {
                        return Err(RuntimeError {
                            message: "JumpIfNotTag: expected data value".into(),
                        });
                    };
                    if actual != tag {
                        let ip = self.frames[fi].ip;
                        let target =
                            self.checked_relative_ip(proto_idx, ip, offset, "JumpIfNotTag")?;
                        self.frames[fi].ip = target;
                    }
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

                Op::MakeRecClosures => self.make_rec_closures(fi)?,

                Op::Call => {
                    let arity = self.read_u8()? as usize;
                    let callee_pos = self.callee_pos_for_arity(arity, "Call")?;
                    match self.stack[callee_pos] {
                        Value::Builtin(id) => self.call_builtin(id, callee_pos, arity)?,
                        _ => self.enter_closure(callee_pos, arity)?,
                    }
                }

                Op::TailCall => {
                    let arity = self.read_u8()? as usize;
                    let callee_pos = self.callee_pos_for_arity(arity, "TailCall")?;
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
                            let fi = self.current_frame_index()?;
                            let base = self.frames[fi].base;
                            let args_start = callee_pos + 1;
                            for i in 0..arity {
                                let dst = self.checked_stack_index(base, i, "TailCall target")?;
                                let src =
                                    self.checked_stack_index(args_start, i, "TailCall args")?;
                                self.stack[dst] = self.stack[src];
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
                    let arg_start = self.stack_start_for_arity(arity, "CallDirect")?;
                    if self.frames.len() >= DEFAULT_MAX_CALL_FRAMES {
                        return Err(RuntimeError {
                            message: "stack overflow".into(),
                        });
                    }
                    self.frames.push(CallFrame {
                        proto_idx,
                        ip: 0,
                        base: arg_start,
                        captures: self.no_captures.clone(),
                    });
                }

                Op::TailCallDirect => {
                    let proto_idx = self.read_u16()? as usize;
                    let proto = self.proto(proto_idx)?;
                    let arity = proto.arity as usize;
                    let args_start = self.stack_start_for_arity(arity, "TailCallDirect")?;
                    let fi = self.current_frame_index()?;
                    let base = self.frames[fi].base;
                    for i in 0..arity {
                        let dst = self.checked_stack_index(base, i, "TailCallDirect target")?;
                        let src = self.checked_stack_index(args_start, i, "TailCallDirect args")?;
                        self.stack[dst] = self.stack[src];
                    }
                    self.stack.truncate(base + arity);
                    self.frames[fi].ip = 0;
                    self.frames[fi].proto_idx = proto_idx;
                    self.frames[fi].captures = self.no_captures.clone();
                }

                Op::Return => {
                    let result = self.pop()?;
                    let frame = self.frames.pop().ok_or_else(|| RuntimeError {
                        message: "Return: call frame underflow".into(),
                    })?;
                    self.stack.truncate(frame.base);
                    self.push(result)?;
                    if self
                        .hidden_handlers
                        .last()
                        .is_some_and(|h| h.clause_frame_idx == self.frames.len())
                        && let Some(hidden) = self.hidden_handlers.pop()
                    {
                        self.handlers.extend(hidden.handlers);
                    }
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
                        let flags = self.read_u8()?;
                        let action = match flags {
                            HANDLER_CLAUSE_PROVIDER => {
                                let slot = self.read_u16()? as usize;
                                let closure_slot =
                                    self.local_stack_index(fi, slot, "InstallHandler provider")?;
                                ClauseAction::Provide { closure_slot }
                            }
                            0 => {
                                let offset = self.read_i16()?;
                                let ip = self.checked_relative_ip(
                                    self.frames[fi].proto_idx,
                                    self.frames[fi].ip,
                                    offset,
                                    "InstallHandler clause",
                                )?;
                                ClauseAction::Abort { ip }
                            }
                            _ => {
                                return Err(RuntimeError {
                                    message: format!(
                                        "InstallHandler clause: invalid flags {flags:#04x}"
                                    ),
                                });
                            }
                        };
                        clauses.push(HandlerClause { effect_tag, action });
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
                                .find(|c| c.effect_tag == effect_tag)
                                .map(|c| (hi, c.action))
                        });

                    let (h_idx, action) = user_handler.ok_or_else(|| RuntimeError {
                        message: format!("unhandled effect (tag {effect_tag})"),
                    })?;

                    // The verifier does not prove handler install/remove
                    // balance, so a handler may outlive the frame or stack
                    // range it was installed in. Check before using it.
                    let handler_frame_idx = self.handlers[h_idx].call_frame_idx;
                    let save_from = self.handlers[h_idx].stack_base;
                    let handler_frame_active = self
                        .frames
                        .get(handler_frame_idx)
                        .is_some_and(|frame| frame.base <= save_from);
                    if !handler_frame_active || save_from > self.stack.len() {
                        return Err(RuntimeError {
                            message: "perform: handler frame is no longer active".into(),
                        });
                    }

                    match action {
                        ClauseAction::Provide { closure_slot } => {
                            if closure_slot >= save_from {
                                return Err(RuntimeError {
                                    message: "perform: invalid provider closure slot".into(),
                                });
                            }
                            // The clause runs at the perform site, outside
                            // the handler: hide the handler and every handler
                            // inside it until the clause frame returns.
                            let hidden = self.handlers.split_off(h_idx);
                            self.hidden_handlers.push(HiddenHandlers {
                                clause_frame_idx: self.frames.len(),
                                handlers: hidden,
                            });
                            let callee_pos = self.stack.len();
                            self.push(self.stack[closure_slot])?;
                            self.push(payload)?;
                            // The clause's continuation slot.
                            self.push(Value::Unit)?;
                            self.enter_closure(callee_pos, 2)?;
                        }
                        ClauseAction::Abort { ip } => {
                            // Unwind to the handler frame. Nothing here
                            // allocates, so the popped payload cannot be
                            // collected before it is pushed back.
                            self.handlers.truncate(h_idx + 1);
                            let Some(handler) = self.handlers.pop() else {
                                unreachable!("h_idx indexes an installed handler");
                            };
                            self.stack.truncate(save_from);
                            self.frames.truncate(handler_frame_idx + 1);
                            while self
                                .hidden_handlers
                                .last()
                                .is_some_and(|h| h.clause_frame_idx > handler_frame_idx)
                            {
                                self.hidden_handlers.pop();
                            }
                            let frame = &mut self.frames[handler_frame_idx];
                            frame.proto_idx = handler.proto_idx;
                            frame.captures = handler.captures;
                            frame.ip = ip;
                            self.push(payload)?;
                            // The clause's continuation slot.
                            self.push(Value::Unit)?;
                        }
                    }
                }
            }
        }
    }

    /// Push a call frame for the closure at `callee_pos`, whose `arity`
    /// arguments follow it on the stack.
    fn enter_closure(&mut self, callee_pos: usize, arity: usize) -> Result<(), RuntimeError> {
        let callee = self.stack[callee_pos];
        let (closure_proto, closure_captures) = match callee {
            Value::Heap(r) => match self.heap_get(r)? {
                HeapObject::Closure {
                    proto_idx,
                    captures,
                } => (*proto_idx, captures.clone()),
                _ => {
                    return Err(RuntimeError {
                        message: format!("cannot call non-function: {callee:?}"),
                    });
                }
            },
            _ => {
                return Err(RuntimeError {
                    message: format!("cannot call non-function: {callee:?}"),
                });
            }
        };
        let proto = self.proto(closure_proto)?;
        if proto.arity as usize != arity {
            return Err(RuntimeError {
                message: format!("function expects {} arg(s), got {arity}", proto.arity),
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
        Ok(())
    }

    /// `MakeRecClosures`: create a group of closures that capture each other.
    ///
    /// Each closure is pushed as soon as it is allocated, so the members
    /// created so far stay GC roots while the next one is allocated. Group
    /// captures hold `Unit` until every member exists; then each closure
    /// that refers to a member gets its final capture array. Nothing can
    /// call a member before that, because no other instruction runs in
    /// between.
    fn make_rec_closures(&mut self, fi: usize) -> Result<(), RuntimeError> {
        let n_closures = self.read_u8()? as usize;
        let base = self.stack.len();
        let mut pending: Vec<RecClosurePatch> = Vec::new();
        for member in 0..n_closures {
            let proto_idx = self.read_u16()? as usize;
            let n_captures = self.read_u8()? as usize;
            let mut captures = Vec::with_capacity(n_captures);
            let mut group_refs = Vec::new();
            for capture_idx in 0..n_captures {
                let source = self.read_u8()?;
                let index = self.read_u16()? as usize;
                let val = match source {
                    REC_CAPTURE_UPVALUE => self.capture_value(fi, false, index)?,
                    REC_CAPTURE_LOCAL => self.capture_value(fi, true, index)?,
                    REC_CAPTURE_GROUP if index < n_closures => {
                        group_refs.push((capture_idx, index));
                        Value::Unit
                    }
                    _ => {
                        return Err(RuntimeError {
                            message: format!(
                                "MakeRecClosures: invalid capture source {source} index {index}"
                            ),
                        });
                    }
                };
                captures.push(val);
            }
            let closure = self.alloc(HeapObject::Closure {
                proto_idx,
                captures: Arc::from(captures.as_slice()),
            })?;
            self.push(closure)?;
            if !group_refs.is_empty() {
                pending.push(RecClosurePatch {
                    member,
                    captures,
                    group_refs,
                });
            }
        }
        for patch in pending {
            let mut captures = patch.captures;
            for (capture_idx, target) in patch.group_refs {
                captures[capture_idx] = self.stack[base + target];
            }
            let Value::Heap(r) = self.stack[base + patch.member] else {
                return Err(RuntimeError {
                    message: "MakeRecClosures: closure is not a heap value".into(),
                });
            };
            self.heap
                .set_closure_captures(r, Arc::from(captures))
                .map_err(|e| RuntimeError { message: e.into() })?;
        }
        Ok(())
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

    fn push_field(&mut self, idx: usize) -> Result<(), RuntimeError> {
        let val = self.pop()?;
        match val {
            Value::Heap(r) => match self.heap_get(r)? {
                HeapObject::Tuple(t) => {
                    let field = t.get(idx).copied().ok_or_else(|| RuntimeError {
                        message: format!("GetField: field index {idx} out of bounds"),
                    })?;
                    self.push(field)
                }
                HeapObject::Data { fields, .. } => {
                    let field = fields.get(idx).copied().ok_or_else(|| RuntimeError {
                        message: format!("GetField: field index {idx} out of bounds"),
                    })?;
                    self.push(field)
                }
                _ => Err(RuntimeError {
                    message: "GetField: expected tuple or data".into(),
                }),
            },
            _ => Err(RuntimeError {
                message: "GetField: expected tuple or data".into(),
            }),
        }
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

    fn pop_word(&mut self) -> Result<u64, RuntimeError> {
        match self.pop()? {
            Value::Word(w) => Ok(w),
            v => Err(RuntimeError {
                message: format!("expected Word, got {v:?}"),
            }),
        }
    }

    fn word_binop(&mut self, f: impl Fn(u64, u64) -> u64) -> Result<(), RuntimeError> {
        let b = self.pop_word()?;
        let a = self.pop_word()?;
        self.push(Value::Word(f(a, b)))
    }

    fn word_checked_binop(
        &mut self,
        f: impl Fn(u64, u64) -> Option<u64>,
        err_msg: &str,
    ) -> Result<(), RuntimeError> {
        let b = self.pop_word()?;
        let a = self.pop_word()?;
        match f(a, b) {
            Some(result) => self.push(Value::Word(result)),
            None => Err(RuntimeError {
                message: err_msg.to_string(),
            }),
        }
    }

    fn word_cmp(&mut self, f: impl Fn(u64, u64) -> bool) -> Result<(), RuntimeError> {
        let b = self.pop_word()?;
        let a = self.pop_word()?;
        self.push(Value::Bool(f(a, b)))
    }

    fn current_frame_index(&self) -> Result<usize, RuntimeError> {
        self.frames
            .len()
            .checked_sub(1)
            .ok_or_else(|| RuntimeError {
                message: "call frame underflow".into(),
            })
    }

    fn checked_stack_index(
        &self,
        base: usize,
        offset: usize,
        what: &str,
    ) -> Result<usize, RuntimeError> {
        let idx = base.checked_add(offset).ok_or_else(|| RuntimeError {
            message: format!("{what}: stack index overflow"),
        })?;
        if idx >= self.stack.len() {
            return Err(RuntimeError {
                message: format!("{what}: stack index {idx} out of bounds"),
            });
        }
        Ok(idx)
    }

    fn local_stack_index(
        &self,
        frame_idx: usize,
        slot: usize,
        what: &str,
    ) -> Result<usize, RuntimeError> {
        let frame = self.frames.get(frame_idx).ok_or_else(|| RuntimeError {
            message: format!("{what}: frame index {frame_idx} out of bounds"),
        })?;
        self.checked_stack_index(frame.base, slot, what)
    }

    fn stack_start_for_arity(&self, arity: usize, what: &str) -> Result<usize, RuntimeError> {
        self.stack
            .len()
            .checked_sub(arity)
            .ok_or_else(|| RuntimeError {
                message: format!("{what}: stack underflow for arity {arity}"),
            })
    }

    fn callee_pos_for_arity(&self, arity: usize, what: &str) -> Result<usize, RuntimeError> {
        let required = arity.checked_add(1).ok_or_else(|| RuntimeError {
            message: format!("{what}: arity overflow"),
        })?;
        self.stack
            .len()
            .checked_sub(required)
            .ok_or_else(|| RuntimeError {
                message: format!("{what}: stack underflow for arity {arity}"),
            })
    }

    fn current_code(&self) -> Result<&[u8], RuntimeError> {
        let proto_idx = self
            .frames
            .last()
            .ok_or_else(|| RuntimeError {
                message: "call frame underflow".into(),
            })?
            .proto_idx;
        Ok(&self.chunk_for_checked(proto_idx)?.code)
    }

    fn read_u8(&mut self) -> Result<u8, RuntimeError> {
        let fi = self.current_frame_index()?;
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
        let fi = self.current_frame_index()?;
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
