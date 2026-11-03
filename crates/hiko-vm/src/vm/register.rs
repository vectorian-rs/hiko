//! Register VM, stage 1 (#155; design in
//! `docs/rnd/2026-11-register-vm-design.md`).
//!
//! Verified stack bytecode is translated, function by function, into
//! register code. A frame's register `r` is `stack[base + r]`, the slot the
//! stack VM would use for operand-stack position `r`, so frames, GC roots
//! and suspension keep their layout. A function the translator cannot
//! handle (an opcode outside stage 1, or a join where the stack depths
//! differ) keeps running on the stack VM; the two call each other through
//! the shared stack layout.
//!
//! The translator keeps a symbolic operand stack: `GetLocal` and constants
//! emit nothing and only record where the value is. Values are written to
//! their stack-slot registers ("materialized") before jumps, at jump
//! targets, before calls, and before a local that something still refers to
//! is overwritten.

use std::collections::BTreeMap;

use hiko_compile::chunk::{Chunk, CompiledProgram, Constant};
use hiko_compile::op::Op;

use super::dispatch::{Flow, error, error_fmt};
use super::*;

pub(super) type Reg = u16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cmp {
    Lt,
    Gt,
    Le,
    Ge,
}

impl Cmp {
    #[inline(always)]
    fn holds(self, a: i64, b: i64) -> bool {
        match self {
            Cmp::Lt => a < b,
            Cmp::Gt => a > b,
            Cmp::Le => a <= b,
            Cmp::Ge => a >= b,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IntOp {
    Add,
    Sub,
    Mul,
}

/// One register instruction. Jump targets are indices into the function's
/// code.
#[derive(Clone, Copy, Debug)]
pub(super) enum RegOp {
    /// Does nothing; carries the work of stack instructions that emitted no
    /// code before a jump target.
    Nop,
    LoadConst {
        dst: Reg,
        idx: u16,
    },
    Move {
        dst: Reg,
        src: Reg,
    },
    Int {
        op: IntOp,
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    CmpInt {
        cmp: Cmp,
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    JumpIfNotCmpInt {
        cmp: Cmp,
        a: Reg,
        b: Reg,
        target: u32,
    },
    JumpIfFalse {
        cond: Reg,
        target: u32,
    },
    Jump {
        target: u32,
    },
    /// Call `proto` with the arguments in `first_arg ..`; the result lands in
    /// `first_arg`.
    CallDirect {
        proto: u16,
        first_arg: Reg,
        n_args: u8,
    },
    CallSelf {
        first_arg: Reg,
        n_args: u8,
    },
    TailCallDirect {
        proto: u16,
        first_arg: Reg,
        n_args: u8,
    },
    TailCallSelf {
        first_arg: Reg,
        n_args: u8,
    },
    Return {
        src: Reg,
    },
    /// Fail with the message `messages[msg]`.
    Panic {
        msg: u16,
    },
    Halt,
}

/// A translated function.
#[derive(Debug)]
pub(super) struct RegFunction {
    pub(super) code: Vec<RegOp>,
    /// Stack instructions each `RegOp` stands for, charged as work.
    pub(super) weight: Vec<u32>,
    /// Stack bytecode offset each `RegOp` came from, for error spans.
    pub(super) src_ip: Vec<u32>,
    /// Scalar constants, already converted.
    pub(super) consts: Vec<Value>,
    /// `Panic` messages.
    pub(super) messages: Vec<String>,
    pub(super) n_regs: u16,
}

/// Register code for a whole program: `main` and each function, when it
/// translates.
#[derive(Debug, Default)]
pub(super) struct RegisterCode {
    pub(super) main: Option<RegFunction>,
    pub(super) protos: Vec<Option<RegFunction>>,
}

impl RegisterCode {
    pub(super) fn translate(program: &CompiledProgram) -> Self {
        let arities: Vec<u8> = program.functions.iter().map(|p| p.arity).collect();
        RegisterCode {
            main: translate(&program.main, 0, &arities),
            protos: program
                .functions
                .iter()
                .map(|p| translate(&p.chunk, p.arity, &arities))
                .collect(),
        }
    }

    #[inline(always)]
    pub(super) fn get(&self, proto_idx: usize) -> Option<&RegFunction> {
        if proto_idx == usize::MAX {
            self.main.as_ref()
        } else {
            self.protos.get(proto_idx)?.as_ref()
        }
    }
}

// ── Translation ──────────────────────────────────────────────────────

struct Inst {
    start: usize,
    op: Op,
    /// u16 operand (local slot, constant, proto) where the op has one.
    a: u16,
    /// Jump target (stack bytecode offset) for jumps.
    target: Option<usize>,
}

fn decode(code: &[u8]) -> Option<Vec<Inst>> {
    let mut out = Vec::new();
    let mut ip = 0;
    while ip < code.len() {
        let start = ip;
        let op = Op::try_from(code[ip]).ok()?;
        ip += 1;
        let u16_at = |ip: usize| -> Option<u16> {
            Some(u16::from_le_bytes([*code.get(ip)?, *code.get(ip + 1)?]))
        };
        let (a, target) = match op {
            Op::Const
            | Op::GetLocal
            | Op::SetLocal
            | Op::CallDirect
            | Op::TailCallDirect
            | Op::Panic => {
                let a = u16_at(ip)?;
                ip += 2;
                (a, None)
            }
            Op::Jump
            | Op::JumpIfFalse
            | Op::JumpIfNotLtInt
            | Op::JumpIfNotGtInt
            | Op::JumpIfNotLeInt
            | Op::JumpIfNotGeInt => {
                let offset = u16_at(ip)? as i16;
                ip += 2;
                (0, Some(ip.checked_add_signed(offset as isize)?))
            }
            Op::Unit
            | Op::True
            | Op::False
            | Op::GetLocal0
            | Op::GetLocal1
            | Op::GetLocal2
            | Op::GetLocal3
            | Op::SetLocal0
            | Op::SetLocal1
            | Op::SetLocal2
            | Op::SetLocal3
            | Op::Pop
            | Op::AddInt
            | Op::SubInt
            | Op::MulInt
            | Op::LtInt
            | Op::GtInt
            | Op::LeInt
            | Op::GeInt
            | Op::CallSelf
            | Op::TailCallSelf
            | Op::Return
            | Op::Halt => (0, None),
            // Outside stage 1: the function stays on the stack VM.
            _ => return None,
        };
        out.push(Inst {
            start,
            op,
            a,
            target,
        });
    }
    Some(out)
}

/// Stack effect and whether control falls through, for the supported ops.
fn stack_rule(inst: &Inst, self_arity: u8, arities: &[u8]) -> Option<(isize, bool)> {
    let arity_of = |p: u16| arities.get(p as usize).map(|&a| a as isize);
    Some(match inst.op {
        Op::Const
        | Op::Unit
        | Op::True
        | Op::False
        | Op::GetLocal
        | Op::GetLocal0
        | Op::GetLocal1
        | Op::GetLocal2
        | Op::GetLocal3 => (1, true),
        Op::SetLocal
        | Op::SetLocal0
        | Op::SetLocal1
        | Op::SetLocal2
        | Op::SetLocal3
        | Op::Pop
        | Op::AddInt
        | Op::SubInt
        | Op::MulInt
        | Op::LtInt
        | Op::GtInt
        | Op::LeInt
        | Op::GeInt
        | Op::JumpIfFalse => (-1, true),
        Op::JumpIfNotLtInt | Op::JumpIfNotGtInt | Op::JumpIfNotLeInt | Op::JumpIfNotGeInt => {
            (-2, true)
        }
        Op::Jump => (0, false),
        Op::CallDirect => (1 - arity_of(inst.a)?, true),
        Op::CallSelf => (1 - self_arity as isize, true),
        Op::TailCallDirect | Op::TailCallSelf | Op::Return | Op::Panic | Op::Halt => (0, false),
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// The value is in this register.
    Reg(Reg),
    /// The value is this scalar constant (index into `consts`).
    Const(u16),
}

struct Builder {
    code: Vec<RegOp>,
    weight: Vec<u32>,
    src_ip: Vec<u32>,
    pending: u32,
    current_ip: u32,
}

impl Builder {
    fn emit(&mut self, op: RegOp) {
        self.code.push(op);
        self.weight.push(self.pending);
        self.src_ip.push(self.current_ip);
        self.pending = 0;
    }

    /// Write every symbolic entry to its own stack-slot register.
    fn materialize(&mut self, stack: &mut [Entry]) {
        for (k, entry) in stack.iter_mut().enumerate() {
            let dst = k as Reg;
            match *entry {
                Entry::Reg(r) if r == dst => {}
                Entry::Reg(src) => self.emit(RegOp::Move { dst, src }),
                Entry::Const(idx) => self.emit(RegOp::LoadConst { dst, idx }),
            }
            *entry = Entry::Reg(dst);
        }
    }

    /// The register holding `entry` at stack position `pos`, loading a
    /// constant into the position's own register first.
    fn operand(&mut self, entry: Entry, pos: usize) -> Reg {
        match entry {
            Entry::Reg(r) => r,
            Entry::Const(idx) => {
                let dst = pos as Reg;
                self.emit(RegOp::LoadConst { dst, idx });
                dst
            }
        }
    }
}

/// Translate one function whose first `arity` stack slots are its
/// arguments.
fn translate(chunk: &Chunk, arity: u8, arities: &[u8]) -> Option<RegFunction> {
    let insts = decode(&chunk.code)?;
    if insts.is_empty() {
        return None;
    }
    let index: BTreeMap<usize, usize> = insts
        .iter()
        .enumerate()
        .map(|(i, x)| (x.start, i))
        .collect();

    // Stack depth at each instruction; every path must agree.
    let mut depth: Vec<Option<usize>> = vec![None; insts.len()];
    let mut work = vec![(0usize, arity as usize)];
    let mut max_depth = arity as usize;
    while let Some((i, d)) = work.pop() {
        match depth[i] {
            Some(existing) if existing == d => continue,
            Some(_) => return None,
            None => depth[i] = Some(d),
        }
        let inst = &insts[i];
        let (delta, falls) = stack_rule(inst, arity, arities)?;
        let after = d.checked_add_signed(delta)?;
        max_depth = max_depth.max(d).max(after);
        if let Some(t) = inst.target {
            work.push((*index.get(&t)?, after));
        }
        if falls {
            if i + 1 >= insts.len() {
                return None;
            }
            work.push((i + 1, after));
        }
    }
    let n_regs = Reg::try_from(max_depth.max(1)).ok()?;

    let targets: std::collections::BTreeSet<usize> =
        insts.iter().filter_map(|x| x.target).collect();

    let mut consts = Vec::new();
    let mut messages = Vec::new();
    let mut b = Builder {
        code: Vec::new(),
        weight: Vec::new(),
        src_ip: Vec::new(),
        pending: 0,
        current_ip: 0,
    };
    // Stack offset -> register pc, and jumps to patch.
    let mut pc_of: BTreeMap<usize, u32> = BTreeMap::new();
    let mut patches: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<Entry> = Vec::new();
    let mut reachable_prev = false;

    for (i, inst) in insts.iter().enumerate() {
        let Some(d) = depth[i] else {
            // Unreachable; nothing jumps here.
            reachable_prev = false;
            continue;
        };
        b.current_ip = inst.start as u32;
        if targets.contains(&inst.start) {
            if reachable_prev {
                b.materialize(&mut stack);
                if b.pending > 0 {
                    b.emit(RegOp::Nop);
                }
            }
            stack = (0..d).map(|k| Entry::Reg(k as Reg)).collect();
            pc_of.insert(inst.start, b.code.len() as u32);
        } else if !reachable_prev {
            stack = (0..d).map(|k| Entry::Reg(k as Reg)).collect();
        }
        debug_assert_eq!(stack.len(), d);
        b.pending += 1;
        reachable_prev = true;

        let local = |op: Op, a: u16| -> u16 {
            match op {
                Op::GetLocal0 | Op::SetLocal0 => 0,
                Op::GetLocal1 | Op::SetLocal1 => 1,
                Op::GetLocal2 | Op::SetLocal2 => 2,
                Op::GetLocal3 | Op::SetLocal3 => 3,
                _ => a,
            }
        };

        match inst.op {
            Op::Const => {
                let value = match chunk.constants.get(inst.a as usize)? {
                    Constant::Int(n) => Value::Int(*n),
                    Constant::Float(f) => Value::Float(*f),
                    Constant::Char(c) => Value::Char(*c),
                    Constant::Word(w) => Value::Word(*w),
                    // Strings need the heap; stage 1 leaves them to the stack VM.
                    Constant::String(_) => return None,
                };
                stack.push(Entry::Const(push_const(&mut consts, value)?));
            }
            Op::Unit => stack.push(Entry::Const(push_const(&mut consts, Value::Unit)?)),
            Op::True => stack.push(Entry::Const(push_const(&mut consts, Value::Bool(true))?)),
            Op::False => stack.push(Entry::Const(push_const(&mut consts, Value::Bool(false))?)),
            Op::GetLocal | Op::GetLocal0 | Op::GetLocal1 | Op::GetLocal2 | Op::GetLocal3 => {
                let slot = local(inst.op, inst.a) as usize;
                // The value currently in that slot (a register or constant).
                stack.push(*stack.get(slot)?);
            }
            Op::SetLocal | Op::SetLocal0 | Op::SetLocal1 | Op::SetLocal2 | Op::SetLocal3 => {
                let slot = local(inst.op, inst.a) as usize;
                let value = stack.pop()?;
                if slot >= stack.len() {
                    return None;
                }
                let target = slot as Reg;
                // Entries that still read the slot's old value keep it.
                for (k, entry) in stack.iter_mut().enumerate() {
                    if k != slot && *entry == Entry::Reg(target) {
                        b.emit(RegOp::Move {
                            dst: k as Reg,
                            src: target,
                        });
                        *entry = Entry::Reg(k as Reg);
                    }
                }
                match value {
                    Entry::Reg(r) if r == target => {}
                    Entry::Reg(src) => b.emit(RegOp::Move { dst: target, src }),
                    Entry::Const(idx) => b.emit(RegOp::LoadConst { dst: target, idx }),
                }
                stack[slot] = Entry::Reg(target);
            }
            Op::Pop => {
                stack.pop()?;
            }
            Op::AddInt
            | Op::SubInt
            | Op::MulInt
            | Op::LtInt
            | Op::GtInt
            | Op::LeInt
            | Op::GeInt => {
                let rhs = stack.pop()?;
                let lhs = stack.pop()?;
                let pos = stack.len();
                let a = b.operand(lhs, pos);
                let bb = b.operand(rhs, pos + 1);
                let dst = pos as Reg;
                let op = match inst.op {
                    Op::AddInt => RegOp::Int {
                        op: IntOp::Add,
                        dst,
                        a,
                        b: bb,
                    },
                    Op::SubInt => RegOp::Int {
                        op: IntOp::Sub,
                        dst,
                        a,
                        b: bb,
                    },
                    Op::MulInt => RegOp::Int {
                        op: IntOp::Mul,
                        dst,
                        a,
                        b: bb,
                    },
                    Op::LtInt => RegOp::CmpInt {
                        cmp: Cmp::Lt,
                        dst,
                        a,
                        b: bb,
                    },
                    Op::GtInt => RegOp::CmpInt {
                        cmp: Cmp::Gt,
                        dst,
                        a,
                        b: bb,
                    },
                    Op::LeInt => RegOp::CmpInt {
                        cmp: Cmp::Le,
                        dst,
                        a,
                        b: bb,
                    },
                    _ => RegOp::CmpInt {
                        cmp: Cmp::Ge,
                        dst,
                        a,
                        b: bb,
                    },
                };
                b.emit(op);
                stack.push(Entry::Reg(dst));
            }
            Op::JumpIfNotLtInt | Op::JumpIfNotGtInt | Op::JumpIfNotLeInt | Op::JumpIfNotGeInt => {
                let rhs = stack.pop()?;
                let lhs = stack.pop()?;
                let pos = stack.len();
                b.materialize(&mut stack);
                let a = b.operand(lhs, pos);
                let bb = b.operand(rhs, pos + 1);
                let cmp = match inst.op {
                    Op::JumpIfNotLtInt => Cmp::Lt,
                    Op::JumpIfNotGtInt => Cmp::Gt,
                    Op::JumpIfNotLeInt => Cmp::Le,
                    _ => Cmp::Ge,
                };
                patches.push((b.code.len(), inst.target?));
                b.emit(RegOp::JumpIfNotCmpInt {
                    cmp,
                    a,
                    b: bb,
                    target: 0,
                });
            }
            Op::JumpIfFalse => {
                let cond = stack.pop()?;
                let pos = stack.len();
                b.materialize(&mut stack);
                let cond = b.operand(cond, pos);
                patches.push((b.code.len(), inst.target?));
                b.emit(RegOp::JumpIfFalse { cond, target: 0 });
            }
            Op::Jump => {
                b.materialize(&mut stack);
                patches.push((b.code.len(), inst.target?));
                b.emit(RegOp::Jump { target: 0 });
                reachable_prev = false;
            }
            Op::CallDirect | Op::CallSelf => {
                let n_args = if inst.op == Op::CallSelf {
                    arity as usize
                } else {
                    *arities.get(inst.a as usize)? as usize
                };
                b.materialize(&mut stack);
                let first_arg = stack.len().checked_sub(n_args)?;
                let first = first_arg as Reg;
                let n_args = u8::try_from(n_args).ok()?;
                b.emit(if inst.op == Op::CallSelf {
                    RegOp::CallSelf {
                        first_arg: first,
                        n_args,
                    }
                } else {
                    RegOp::CallDirect {
                        proto: inst.a,
                        first_arg: first,
                        n_args,
                    }
                });
                stack.truncate(first_arg);
                stack.push(Entry::Reg(first));
            }
            Op::TailCallDirect | Op::TailCallSelf => {
                let n_args = if inst.op == Op::TailCallSelf {
                    arity as usize
                } else {
                    *arities.get(inst.a as usize)? as usize
                };
                b.materialize(&mut stack);
                let first_arg = stack.len().checked_sub(n_args)? as Reg;
                let n_args = u8::try_from(n_args).ok()?;
                b.emit(if inst.op == Op::TailCallSelf {
                    RegOp::TailCallSelf { first_arg, n_args }
                } else {
                    RegOp::TailCallDirect {
                        proto: inst.a,
                        first_arg,
                        n_args,
                    }
                });
                reachable_prev = false;
            }
            Op::Return => {
                let value = stack.pop()?;
                let src = b.operand(value, stack.len());
                b.emit(RegOp::Return { src });
                reachable_prev = false;
            }
            Op::Panic => {
                let Constant::String(message) = chunk.constants.get(inst.a as usize)? else {
                    return None;
                };
                let msg = u16::try_from(messages.len()).ok()?;
                messages.push(message.clone());
                b.emit(RegOp::Panic { msg });
                reachable_prev = false;
            }
            Op::Halt => {
                b.emit(RegOp::Halt);
                reachable_prev = false;
            }
            _ => return None,
        }
    }

    for (at, target) in patches {
        let pc = *pc_of.get(&target)?;
        match &mut b.code[at] {
            RegOp::JumpIfNotCmpInt { target, .. }
            | RegOp::JumpIfFalse { target, .. }
            | RegOp::Jump { target } => *target = pc,
            _ => return None,
        }
    }

    let function = RegFunction {
        code: b.code,
        weight: b.weight,
        src_ip: b.src_ip,
        consts,
        messages,
        n_regs,
    };
    check(&function).then_some(function)
}

fn push_const(consts: &mut Vec<Value>, value: Value) -> Option<u16> {
    let idx = u16::try_from(consts.len()).ok()?;
    consts.push(value);
    Some(idx)
}

/// Every register below `n_regs`, every constant and jump target valid.
fn check(f: &RegFunction) -> bool {
    let reg = |r: Reg| r < f.n_regs;
    let pc = |t: u32| (t as usize) < f.code.len();
    let konst = |i: u16| (i as usize) < f.consts.len();
    f.code.iter().all(|op| match *op {
        RegOp::Nop | RegOp::Halt => true,
        RegOp::LoadConst { dst, idx } => reg(dst) && konst(idx),
        RegOp::Move { dst, src } => reg(dst) && reg(src),
        RegOp::Int { dst, a, b, .. } | RegOp::CmpInt { dst, a, b, .. } => {
            reg(dst) && reg(a) && reg(b)
        }
        RegOp::JumpIfNotCmpInt { a, b, target, .. } => reg(a) && reg(b) && pc(target),
        RegOp::JumpIfFalse { cond, target } => reg(cond) && pc(target),
        RegOp::Jump { target } => pc(target),
        // The result lands in `first_arg`.
        RegOp::CallDirect {
            first_arg, n_args, ..
        }
        | RegOp::CallSelf { first_arg, n_args } => {
            reg(first_arg) && (first_arg as usize + n_args as usize) <= f.n_regs as usize
        }
        RegOp::TailCallDirect {
            first_arg, n_args, ..
        }
        | RegOp::TailCallSelf { first_arg, n_args } => {
            (first_arg as usize + n_args as usize) <= f.n_regs as usize
        }
        RegOp::Return { src } => reg(src),
        RegOp::Panic { msg } => (msg as usize) < f.messages.len(),
    })
}

// ── Execution ────────────────────────────────────────────────────────

impl VM {
    /// Whether the top frame runs register code.
    pub(super) fn top_frame_is_register(&self) -> bool {
        self.frames
            .last()
            .is_some_and(|f| self.register_code.get(f.proto_idx).is_some())
    }

    /// Run register frames until the program finishes or the top frame runs
    /// on the stack VM.
    ///
    /// A register frame's `ip` is a register pc. On an error it is the pc of
    /// the failing instruction; `error_span` maps it back through `src_ip`.
    pub(super) fn dispatch_register(&mut self) -> Result<Flow, RuntimeError> {
        let code = Arc::clone(&self.register_code);
        loop {
            let fi = self.current_frame_index()?;
            let (proto_idx, base, mut pc) = {
                let f = &self.frames[fi];
                (f.proto_idx, f.base, f.ip)
            };
            let Some(func) = code.get(proto_idx) else {
                return Ok(Flow::Switch);
            };
            // A register frame owns registers base .. base + n_regs.
            let top = base + func.n_regs as usize;
            if top > DEFAULT_MAX_STACK_SLOTS {
                return Err(error("stack overflow"));
            }
            self.stack.resize(top, Value::Unit);
            let regs = base..top;

            // Fail at the current instruction, leaving the frame on it.
            macro_rules! fail {
                ($e:expr) => {{
                    self.frames[fi].ip = pc;
                    return Err($e);
                }};
            }
            macro_rules! r {
                ($reg:expr) => {
                    match self.stack[regs.clone()].get($reg as usize) {
                        Some(v) => *v,
                        None => fail!(error("register out of bounds")),
                    }
                };
            }
            macro_rules! set {
                ($reg:expr, $val:expr) => {{
                    let v = $val;
                    match self.stack[regs.clone()].get_mut($reg as usize) {
                        Some(slot) => *slot = v,
                        None => fail!(error("register out of bounds")),
                    }
                }};
            }
            macro_rules! int {
                ($reg:expr) => {
                    match r!($reg) {
                        Value::Int(n) => n,
                        v => fail!(error_fmt(format_args!("expected Int, got {v:?}"))),
                    }
                };
            }

            loop {
                let (Some(&op), Some(&weight)) = (func.code.get(pc), func.weight.get(pc)) else {
                    fail!(error("unexpected end of register code"));
                };
                if let Some(fuel) = self.fuel.as_mut() {
                    // An instruction worth more than the fuel left is paid
                    // in part now and the rest after the yield, so every
                    // slice makes progress and the total charged is exactly
                    // the stack VM's.
                    let due = u64::from(weight - self.register_paid);
                    if *fuel < due {
                        self.register_paid += *fuel as u32;
                        *fuel = 0;
                        fail!(error("fuel exhausted (execution limit reached)"));
                    }
                    *fuel -= due;
                    self.register_paid = 0;
                }
                match op {
                    RegOp::Nop => {}
                    RegOp::LoadConst { dst, idx } => {
                        let Some(&v) = func.consts.get(idx as usize) else {
                            fail!(error("constant out of bounds"));
                        };
                        set!(dst, v);
                    }
                    RegOp::Move { dst, src } => set!(dst, r!(src)),
                    RegOp::Int { op, dst, a, b } => {
                        let (x, y) = (int!(a), int!(b));
                        let v = match op {
                            IntOp::Add => x.checked_add(y),
                            IntOp::Sub => x.checked_sub(y),
                            IntOp::Mul => x.checked_mul(y),
                        };
                        match v {
                            Some(v) => set!(dst, Value::Int(v)),
                            None => fail!(error(match op {
                                IntOp::Add => "integer overflow (addition)",
                                IntOp::Sub => "integer overflow (subtraction)",
                                IntOp::Mul => "integer overflow (multiplication)",
                            })),
                        }
                    }
                    RegOp::CmpInt { cmp, dst, a, b } => {
                        let (x, y) = (int!(a), int!(b));
                        set!(dst, Value::Bool(cmp.holds(x, y)));
                    }
                    RegOp::JumpIfNotCmpInt { cmp, a, b, target } => {
                        let (x, y) = (int!(a), int!(b));
                        if !cmp.holds(x, y) {
                            pc = target as usize;
                            continue;
                        }
                    }
                    RegOp::JumpIfFalse { cond, target } => match r!(cond) {
                        Value::Bool(true) => {}
                        Value::Bool(false) => {
                            pc = target as usize;
                            continue;
                        }
                        v => fail!(error_fmt(format_args!("expected Bool, got {v:?}"))),
                    },
                    RegOp::Jump { target } => {
                        pc = target as usize;
                        continue;
                    }
                    RegOp::CallDirect {
                        proto,
                        first_arg,
                        n_args,
                    } => {
                        if self.frames.len() >= DEFAULT_MAX_CALL_FRAMES {
                            fail!(error("stack overflow"));
                        }
                        self.enter(
                            fi,
                            pc + 1,
                            proto as usize,
                            base + first_arg as usize,
                            n_args,
                            Captures::Empty,
                        );
                        break;
                    }
                    RegOp::CallSelf { first_arg, n_args } => {
                        if self.frames.len() >= DEFAULT_MAX_CALL_FRAMES {
                            fail!(error("stack overflow"));
                        }
                        let captures = self.frames[fi].captures.clone();
                        self.enter(
                            fi,
                            pc + 1,
                            proto_idx,
                            base + first_arg as usize,
                            n_args,
                            captures,
                        );
                        break;
                    }
                    RegOp::TailCallDirect {
                        proto,
                        first_arg,
                        n_args,
                    } => {
                        self.stack.copy_within(
                            base + first_arg as usize..base + first_arg as usize + n_args as usize,
                            base,
                        );
                        let f = &mut self.frames[fi];
                        f.ip = 0;
                        f.proto_idx = proto as usize;
                        f.captures = Captures::Empty;
                        // The callee may run on the stack VM, which expects
                        // exactly its arguments above the base.
                        self.stack.truncate(base + n_args as usize);
                        break;
                    }
                    RegOp::TailCallSelf { first_arg, n_args } => {
                        self.stack.copy_within(
                            base + first_arg as usize..base + first_arg as usize + n_args as usize,
                            base,
                        );
                        pc = 0;
                        continue;
                    }
                    RegOp::Return { src } => {
                        let value = r!(src);
                        if self.return_from_frame(value)? {
                            return Ok(Flow::Done);
                        }
                        break;
                    }
                    RegOp::Panic { msg } => {
                        let message = func.messages.get(msg as usize).cloned().unwrap_or_default();
                        fail!(RuntimeError { message });
                    }
                    RegOp::Halt => return Ok(Flow::Done),
                }
                pc += 1;
            }
        }
    }

    /// Push a call frame for `proto_idx` whose arguments are the `n_args`
    /// stack slots from `arg_start`; the caller resumes at `resume_pc`.
    fn enter(
        &mut self,
        caller: usize,
        resume_pc: usize,
        proto_idx: usize,
        arg_start: usize,
        n_args: u8,
        captures: Captures,
    ) {
        self.frames[caller].ip = resume_pc;
        // A stack-VM callee expects exactly its arguments above its base.
        self.stack.truncate(arg_start + n_args as usize);
        self.frames.push(CallFrame {
            proto_idx,
            ip: 0,
            base: arg_start,
            captures,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;

    fn compile(src: &str) -> CompiledProgram {
        let tokens = Lexer::new(src, 0).tokenize().expect("lex error");
        let program = Parser::new(tokens).parse_program().expect("parse error");
        Compiler::compile(program).expect("compile error").0
    }

    /// Whether the body of function `name` translated. A function over a
    /// tuple also has a one-argument wrapper; the body is the one with the
    /// highest arity.
    fn translates(src: &str, name: &str) -> bool {
        let program = compile(src);
        let code = RegisterCode::translate(&program);
        let (idx, _) = program
            .functions
            .iter()
            .enumerate()
            .filter(|(_, p)| p.name.as_deref() == Some(name))
            .max_by_key(|(_, p)| p.arity)
            .expect("no such function");
        code.protos[idx].is_some()
    }

    /// Run `src` on the stack VM alone and with register code, returning
    /// both outputs (or errors).
    fn run_both(src: &str, fuel: Option<u64>) -> [Result<Vec<String>, String>; 2] {
        [false, true].map(|register| {
            let mut vm = VM::new(compile(src));
            if !register {
                vm.register_code = Arc::default();
            }
            vm.enable_output_capture();
            vm.fuel = fuel;
            match vm.run() {
                Ok(()) => Ok(vm.get_output().to_vec()),
                Err(e) => Err(e.message),
            }
        })
    }

    #[track_caller]
    fn assert_same(src: &str) -> Vec<String> {
        let [stack, register] = run_both(src, None);
        assert_eq!(stack, register);
        stack.expect("program failed")
    }

    const FIB: &str = "
        fun fib n = if n < 2 then n else fib (n - 1) + fib (n - 2)
        val _ = println (int_to_string (fib 20))
    ";

    const TAK: &str = "
        fun tak (x, y, z) =
          if y < x
          then tak (tak (x - 1, y, z), tak (y - 1, z, x), tak (z - 1, x, y))
          else z
        val _ = println (int_to_string (tak (18, 12, 6)))
    ";

    #[test]
    fn fib_and_tak_translate_and_agree() {
        for (src, name, expected) in [(FIB, "fib", "6765"), (TAK, "tak", "7")] {
            assert!(translates(src, name), "{name} did not translate");
            assert_eq!(assert_same(src).concat().trim(), expected);
        }
    }

    #[test]
    fn errors_agree_and_point_at_the_source() {
        let src = "
            fun grow n = if n > 4611686018427387903 then n * 4 else grow (n * 2)
            val _ = grow 1
        ";
        assert!(translates(src, "grow"));
        let [stack, register] = run_both(src, None);
        assert_eq!(stack, register);
        assert!(
            stack
                .unwrap_err()
                .contains("integer overflow (multiplication)")
        );

        let spans = [false, true].map(|register| {
            let mut vm = VM::new(compile(src));
            if !register {
                vm.register_code = Arc::default();
            }
            assert!(vm.run().is_err());
            vm.error_span()
        });
        assert!(spans[0].is_some());
        assert_eq!(spans[0], spans[1]);
    }

    #[test]
    fn fuel_charges_the_same_work() {
        // Each register instruction is charged the stack instructions it
        // replaces, so the least fuel that finishes is the same on both VMs.
        for src in [FIB, TAK] {
            let finishes = |fuel: u64| run_both(src, Some(fuel)).map(|r| r.is_ok());
            let (mut lo, mut hi) = (0u64, 1u64 << 24);
            assert_eq!(finishes(hi), [true, true]);
            while lo + 1 < hi {
                let mid = (lo + hi) / 2;
                let [stack, register] = finishes(mid);
                assert_eq!(stack, register, "fuel {mid}");
                if stack { hi = mid } else { lo = mid }
            }
            assert_eq!(finishes(hi - 1), [false, false]);
        }
    }

    #[test]
    fn one_fuel_slices_make_progress() {
        // A scheduler slice can grant less fuel than one register
        // instruction is worth; the program still finishes, in exactly as
        // many slices as on the stack VM.
        let slices = [false, true].map(|register| {
            let mut vm = VM::new(compile(TAK));
            if !register {
                vm.register_code = Arc::default();
            }
            vm.enable_output_capture();
            let mut slices = 0u64;
            loop {
                slices += 1;
                match vm.run_slice(1) {
                    RunResult::Yielded => {}
                    RunResult::Done => break,
                    other => panic!("unexpected {other:?}"),
                }
            }
            (slices, vm.get_output().to_vec())
        });
        assert_eq!(slices[0], slices[1]);
    }

    #[test]
    fn locals_rebound_while_still_on_the_stack() {
        // `let` rebinding and reads of a local that is overwritten later.
        let src = "
            fun f (a : int, b) =
              let val c = a + b
                  val d = c * 2
                  val c = d - a
              in if c < d then c + d else a end
            fun loop (i, acc) = if i > 0 then loop (i - 1, acc + f (i, acc)) else acc
            val _ = println (int_to_string (loop (10, 0)))
        ";
        assert_same(src);
    }
}
