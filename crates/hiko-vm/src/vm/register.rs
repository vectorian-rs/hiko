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

use std::collections::BTreeSet;

use hiko_compile::chunk::{Chunk, CompiledProgram, Constant, FunctionProto};
use hiko_compile::op::Op;

use crate::verify::{DecodedInst, StackRule};

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
    /// Each instruction with the number of stack instructions it stands
    /// for, charged as work.
    pub(super) code: Vec<(RegOp, u32)>,
    /// Stack bytecode offset each instruction came from, for error spans.
    pub(super) src_ip: Vec<u32>,
    /// The deepest stack, relative to the frame, that the stack VM would
    /// have reached by each instruction; 0 where it pushes nothing.
    pub(super) peak: Vec<u16>,
    /// Scalar constants, already converted.
    pub(super) consts: Vec<Value>,
    /// `Panic` messages.
    pub(super) messages: Vec<String>,
    pub(super) n_regs: u16,
}

/// Register code for a whole program, for each function that translates.
#[derive(Debug, Default)]
pub(super) struct RegisterCode {
    /// Indexed by `proto_idx + 1` (wrapping), so `main` (`usize::MAX`) is
    /// entry 0.
    functions: Vec<Option<RegFunction>>,
}

impl RegisterCode {
    pub(super) fn translate(program: &CompiledProgram) -> Self {
        let effect_tags: BTreeSet<u16> = program.effects.iter().map(|e| e.tag).collect();
        let translate = |chunk: &Chunk, n_locals: u16, proto: Option<&FunctionProto>| {
            let decoded = crate::verify::decode_chunk(
                chunk,
                n_locals as usize,
                proto,
                &program.functions,
                &effect_tags,
                program.globals.len(),
            )
            .ok()?;
            translate(chunk, &decoded, proto.map_or(0, |p| p.arity))
        };
        let main = translate(&program.main, program.main_n_locals, None);
        let protos = program
            .functions
            .iter()
            .map(|p| translate(&p.chunk, p.n_locals, Some(p)));
        RegisterCode {
            functions: std::iter::once(main).chain(protos).collect(),
        }
    }

    #[inline(always)]
    pub(super) fn get(&self, proto_idx: usize) -> Option<&RegFunction> {
        self.functions.get(proto_idx.wrapping_add(1))?.as_ref()
    }

    #[inline(always)]
    pub(super) fn is_register(&self, proto_idx: usize) -> bool {
        self.get(proto_idx).is_some()
    }

    /// The stack bytecode offset of register instruction `pc`.
    pub(super) fn src_ip(&self, proto_idx: usize, pc: usize) -> Option<usize> {
        Some(*self.get(proto_idx)?.src_ip.get(pc)? as usize)
    }
}

// ── Translation ──────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// The value is in this register.
    Reg(Reg),
    /// The value is this scalar constant (index into `consts`).
    Const(u16),
}

#[derive(Default)]
struct Builder {
    code: Vec<(RegOp, u32)>,
    src_ip: Vec<u32>,
    peak: Vec<u16>,
    /// Stack instructions translated since the last instruction that does
    /// their work, and the deepest stack they reached.
    pending: u32,
    pending_peak: u16,
    current_ip: u32,
}

impl Builder {
    /// Emit the instruction that does the pending stack instructions' work;
    /// it is charged for them.
    fn emit(&mut self, op: RegOp) {
        self.code.push((op, self.pending));
        self.src_ip.push(self.current_ip);
        self.peak.push(self.pending_peak);
        self.pending = 0;
        self.pending_peak = 0;
    }

    /// Emit a copy into a register, which the stack VM has no instruction
    /// for. It costs nothing, so a fuel stop falls on the instruction that
    /// does the work, as on the stack VM.
    fn spill(&mut self, op: RegOp) {
        self.code.push((op, 0));
        self.src_ip.push(self.current_ip);
        self.peak.push(0);
    }

    /// Write every symbolic entry to its own stack-slot register.
    fn materialize(&mut self, stack: &mut [Entry]) {
        for (k, entry) in stack.iter_mut().enumerate() {
            let dst = k as Reg;
            match *entry {
                Entry::Reg(r) if r == dst => {}
                Entry::Reg(src) => self.spill(RegOp::Move { dst, src }),
                Entry::Const(idx) => self.spill(RegOp::LoadConst { dst, idx }),
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
                self.spill(RegOp::LoadConst { dst, idx });
                dst
            }
        }
    }
}

fn int_op(op: Op) -> Option<IntOp> {
    Some(match op {
        Op::AddInt => IntOp::Add,
        Op::SubInt => IntOp::Sub,
        Op::MulInt => IntOp::Mul,
        _ => return None,
    })
}

fn cmp_op(op: Op) -> Option<Cmp> {
    Some(match op {
        Op::LtInt | Op::JumpIfNotLtInt => Cmp::Lt,
        Op::GtInt | Op::JumpIfNotGtInt => Cmp::Gt,
        Op::LeInt | Op::JumpIfNotLeInt => Cmp::Le,
        Op::GeInt | Op::JumpIfNotGeInt => Cmp::Ge,
        _ => return None,
    })
}

/// Translate one function from its verifier decoding. Its first `arity`
/// stack slots are its arguments.
fn translate(chunk: &Chunk, insts: &[DecodedInst], arity: u8) -> Option<RegFunction> {
    if insts.is_empty() {
        return None;
    }
    // Instruction index by stack bytecode offset.
    let mut index = vec![usize::MAX; chunk.code.len() + 1];
    for (i, inst) in insts.iter().enumerate() {
        index[inst.start] = i;
    }
    let at = |offset: usize| index.get(offset).copied().filter(|&i| i != usize::MAX);

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
        if let StackRule::Exact { delta, .. } = insts[i].stack {
            let after = d.checked_add_signed(delta)?;
            max_depth = max_depth.max(after);
            for &next in &insts[i].successors {
                work.push((at(next)?, after));
            }
        }
    }
    let n_regs = Reg::try_from(max_depth.max(1)).ok()?;

    // Offsets that jumps land on.
    let mut is_target = vec![false; chunk.code.len() + 1];
    for target in insts.iter().filter_map(|inst| inst.jump_target) {
        is_target[target] = true;
    }

    // Each constant once: by chunk constant index, and Unit, true, false.
    let mut consts = Vec::new();
    let mut const_of: Vec<Option<u16>> = vec![None; chunk.constants.len()];
    let mut fixed: [Option<u16>; 3] = [None; 3];
    let mut messages = Vec::new();
    let mut b = Builder::default();
    // Register pc of each jump target, by stack offset, and jumps to patch.
    let mut pc_of = vec![u32::MAX; chunk.code.len() + 1];
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
        if is_target[inst.start] {
            if reachable_prev {
                b.materialize(&mut stack);
                if b.pending > 0 {
                    b.emit(RegOp::Nop);
                }
            }
            stack = (0..d).map(|k| Entry::Reg(k as Reg)).collect();
            pc_of[inst.start] = b.code.len() as u32;
        } else if !reachable_prev {
            stack = (0..d).map(|k| Entry::Reg(k as Reg)).collect();
        }
        debug_assert_eq!(stack.len(), d);
        b.pending += 1;
        if let StackRule::Exact { delta, .. } = inst.stack
            && delta > 0
        {
            // The stack VM's limit applies when this instruction pushes.
            b.pending_peak = b.pending_peak.max(u16::try_from(d + delta as usize).ok()?);
        }
        reachable_prev = true;

        // The u16 operand right after the opcode, for ops that have one.
        let operand = || -> Option<u16> {
            let bytes = chunk.code.get(inst.start + 1..inst.start + 3)?;
            Some(u16::from_le_bytes([bytes[0], bytes[1]]))
        };
        let local = || -> Option<usize> {
            Some(match inst.op {
                Op::GetLocal0 | Op::SetLocal0 => 0,
                Op::GetLocal1 | Op::SetLocal1 => 1,
                Op::GetLocal2 | Op::SetLocal2 => 2,
                Op::GetLocal3 | Op::SetLocal3 => 3,
                _ => operand()? as usize,
            })
        };

        match inst.op {
            Op::Const => {
                let idx = operand()? as usize;
                let value = match chunk.constants.get(idx)? {
                    Constant::Int(n) => Value::Int(*n),
                    Constant::Float(f) => Value::Float(*f),
                    Constant::Char(c) => Value::Char(*c),
                    Constant::Word(w) => Value::Word(*w),
                    // Strings need the heap; stage 1 leaves them to the stack VM.
                    Constant::String(_) => return None,
                };
                stack.push(Entry::Const(intern(
                    &mut consts,
                    const_of.get_mut(idx)?,
                    value,
                )?));
            }
            Op::Unit => stack.push(Entry::Const(intern(
                &mut consts,
                &mut fixed[0],
                Value::Unit,
            )?)),
            Op::True => stack.push(Entry::Const(intern(
                &mut consts,
                &mut fixed[1],
                Value::Bool(true),
            )?)),
            Op::False => stack.push(Entry::Const(intern(
                &mut consts,
                &mut fixed[2],
                Value::Bool(false),
            )?)),
            Op::GetLocal | Op::GetLocal0 | Op::GetLocal1 | Op::GetLocal2 | Op::GetLocal3 => {
                // The value currently in that slot (a register or constant).
                stack.push(*stack.get(local()?)?);
            }
            Op::SetLocal | Op::SetLocal0 | Op::SetLocal1 | Op::SetLocal2 | Op::SetLocal3 => {
                let slot = local()?;
                let value = stack.pop()?;
                if slot >= stack.len() {
                    return None;
                }
                let target = slot as Reg;
                // Entries that still read the slot's old value keep it.
                for (k, entry) in stack.iter_mut().enumerate() {
                    if k != slot && *entry == Entry::Reg(target) {
                        b.spill(RegOp::Move {
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
                b.emit(match (int_op(inst.op), cmp_op(inst.op)) {
                    (Some(op), _) => RegOp::Int { op, dst, a, b: bb },
                    (_, Some(cmp)) => RegOp::CmpInt { cmp, dst, a, b: bb },
                    _ => return None,
                });
                stack.push(Entry::Reg(dst));
            }
            Op::JumpIfNotLtInt | Op::JumpIfNotGtInt | Op::JumpIfNotLeInt | Op::JumpIfNotGeInt => {
                let rhs = stack.pop()?;
                let lhs = stack.pop()?;
                let pos = stack.len();
                b.materialize(&mut stack);
                let a = b.operand(lhs, pos);
                let bb = b.operand(rhs, pos + 1);
                patches.push((b.code.len(), inst.jump_target?));
                b.emit(RegOp::JumpIfNotCmpInt {
                    cmp: cmp_op(inst.op)?,
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
                patches.push((b.code.len(), inst.jump_target?));
                b.emit(RegOp::JumpIfFalse { cond, target: 0 });
            }
            Op::Jump => {
                b.materialize(&mut stack);
                patches.push((b.code.len(), inst.jump_target?));
                b.emit(RegOp::Jump { target: 0 });
                reachable_prev = false;
            }
            Op::CallDirect | Op::CallSelf | Op::TailCallDirect | Op::TailCallSelf => {
                let n_args = match inst.stack {
                    // A call pops its arguments and pushes the result.
                    StackRule::Exact { delta, .. } => usize::try_from(1 - delta).ok()?,
                    StackRule::Terminal { min_depth } => min_depth,
                };
                b.materialize(&mut stack);
                let first = stack.len().checked_sub(n_args)?;
                let first_arg = first as Reg;
                let n_args = u8::try_from(n_args).ok()?;
                b.emit(match inst.op {
                    Op::CallSelf => RegOp::CallSelf { first_arg, n_args },
                    Op::TailCallSelf => RegOp::TailCallSelf { first_arg, n_args },
                    Op::CallDirect => RegOp::CallDirect {
                        proto: operand()?,
                        first_arg,
                        n_args,
                    },
                    _ => RegOp::TailCallDirect {
                        proto: operand()?,
                        first_arg,
                        n_args,
                    },
                });
                if matches!(inst.op, Op::CallDirect | Op::CallSelf) {
                    stack.truncate(first);
                    stack.push(Entry::Reg(first_arg));
                } else {
                    reachable_prev = false;
                }
            }
            Op::Return => {
                let value = stack.pop()?;
                let src = b.operand(value, stack.len());
                b.emit(RegOp::Return { src });
                reachable_prev = false;
            }
            Op::Panic => {
                let Constant::String(message) = chunk.constants.get(operand()? as usize)? else {
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
            // Outside stage 1: the function stays on the stack VM.
            _ => return None,
        }
    }

    for (at, target) in patches {
        let pc = *pc_of.get(target).filter(|&&pc| pc != u32::MAX)?;
        match &mut b.code[at].0 {
            RegOp::JumpIfNotCmpInt { target, .. }
            | RegOp::JumpIfFalse { target, .. }
            | RegOp::Jump { target } => *target = pc,
            _ => return None,
        }
    }

    let function = RegFunction {
        code: b.code,
        src_ip: b.src_ip,
        peak: b.peak,
        consts,
        messages,
        n_regs,
    };
    check(&function).then_some(function)
}

/// The index of `value` in `consts`, adding it the first time; `slot`
/// remembers it.
fn intern(consts: &mut Vec<Value>, slot: &mut Option<u16>, value: Value) -> Option<u16> {
    if slot.is_none() {
        *slot = Some(u16::try_from(consts.len()).ok()?);
        consts.push(value);
    }
    *slot
}

/// Every register below `n_regs`, every constant and jump target valid.
fn check(f: &RegFunction) -> bool {
    let reg = |r: Reg| r < f.n_regs;
    let pc = |t: u32| (t as usize) < f.code.len();
    let konst = |i: u16| (i as usize) < f.consts.len();
    let args =
        |first_arg: Reg, n_args: u8| first_arg as usize + n_args as usize <= f.n_regs as usize;
    f.code.iter().all(|&(op, _)| match op {
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
        | RegOp::CallSelf { first_arg, n_args } => reg(first_arg) && args(first_arg, n_args),
        RegOp::TailCallDirect {
            first_arg, n_args, ..
        }
        | RegOp::TailCallSelf { first_arg, n_args } => args(first_arg, n_args),
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
            .is_some_and(|f| self.register_code.is_register(f.proto_idx))
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
            // A register frame owns registers base .. base + n_regs. Near
            // the stack limit, each instruction checks the depth the stack
            // VM would have reached, so both fail at the same point.
            let top = base + func.n_regs as usize;
            let near_limit = top > DEFAULT_MAX_STACK_SLOTS;
            self.stack.resize(top, Value::Unit);
            let regs = &mut self.stack[base..top];

            // Fail at the current instruction, leaving the frame on it.
            macro_rules! fail {
                ($e:expr) => {{
                    self.frames[fi].ip = pc;
                    return Err($e);
                }};
            }
            macro_rules! r {
                ($reg:expr) => {
                    match regs.get($reg as usize) {
                        Some(v) => *v,
                        None => fail!(error("register out of bounds")),
                    }
                };
            }
            macro_rules! set {
                ($reg:expr, $val:expr) => {{
                    let v = $val;
                    match regs.get_mut($reg as usize) {
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
            // Copy the arguments of a tail call to the frame's first registers.
            macro_rules! shift_args {
                ($first_arg:expr, $n_args:expr) => {{
                    let first = $first_arg as usize;
                    regs.copy_within(first..first + $n_args as usize, 0);
                }};
            }

            loop {
                let Some(&(op, weight)) = func.code.get(pc) else {
                    fail!(error("unexpected end of register code"));
                };
                if let Some(fuel) = self
                    .fuel_lease
                    .as_mut()
                    .and_then(|lease| lease.remaining.as_mut())
                {
                    // An instruction worth more than the fuel left is paid
                    // in part now and the rest after the yield, so every
                    // slice makes progress and the total charged is exactly
                    // the stack VM's.
                    let due = u64::from(weight.saturating_sub(self.register_paid));
                    if *fuel < due {
                        self.register_paid += *fuel as u32;
                        *fuel = 0;
                        fail!(error("fuel exhausted (execution limit reached)"));
                    }
                    *fuel -= due;
                    self.register_paid = 0;
                }
                if near_limit
                    && base + func.peak.get(pc).copied().unwrap_or(0) as usize
                        > DEFAULT_MAX_STACK_SLOTS
                {
                    fail!(error("stack overflow"));
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
                        let (v, overflow) = match op {
                            IntOp::Add => (x.checked_add(y), "integer overflow (addition)"),
                            IntOp::Sub => (x.checked_sub(y), "integer overflow (subtraction)"),
                            IntOp::Mul => (x.checked_mul(y), "integer overflow (multiplication)"),
                        };
                        match v {
                            Some(v) => set!(dst, Value::Int(v)),
                            None => fail!(error(overflow)),
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
                        shift_args!(first_arg, n_args);
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
                        shift_args!(first_arg, n_args);
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
    use super::super::register_check;
    use super::*;

    fn compile(src: &str) -> CompiledProgram {
        register_check::compile(src).expect("does not compile")
    }

    /// VMs for `program`: the stack VM alone, then with register code.
    fn vms(program: &CompiledProgram) -> [VM; 2] {
        register_check::both_codes(program)
            .expect("does not verify")
            .each_ref()
            .map(|code| register_check::vm(program, code))
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
        code.get(idx).is_some()
    }

    /// Run `src` on the stack VM alone and with register code, returning
    /// both outputs (or errors).
    fn run_both(src: &str, fuel: Option<u64>) -> [Result<Vec<String>, String>; 2] {
        let program = compile(src);
        vms(&program).map(|mut vm| {
            if let Some(fuel) = fuel {
                vm.set_fuel(fuel);
            }
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

        let program = compile(src);
        let spans = vms(&program).map(|mut vm| {
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
    fn fuel_stops_at_the_same_call_depth() {
        // Running out of fuel leaves both VMs in the same call frame, so a
        // call is never made with no fuel left for it.
        let program = compile(FIB);
        for fuel in 0..400 {
            let depths = vms(&program).map(|mut vm| {
                vm.set_fuel(fuel);
                let err = vm.run().expect_err("fib needs more fuel");
                assert!(err.is_fuel_exhausted(), "{}", err.message);
                vm.frames.len()
            });
            assert_eq!(depths[0], depths[1], "fuel {fuel}");
        }
    }

    #[test]
    fn stack_overflow_at_the_same_depth() {
        // A register frame reserves all its registers on entry; near the
        // stack limit it still fails only where the stack VM would.
        let src = |n: i64| {
            format!(
                "fun d n = if n < 1 then 0 else 1 + d (n - 1)
                 val _ = println (int_to_string (d {n}))"
            )
        };
        assert!(translates(&src(1), "d"));
        let finishes = |n: i64| run_both(&src(n), None);
        // The deepest n the stack VM finishes.
        let (mut lo, mut hi) = (1i64, 100_000i64);
        assert!(finishes(lo)[0].is_ok() && finishes(hi)[0].is_err());
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if finishes(mid)[0].is_ok() {
                lo = mid
            } else {
                hi = mid
            }
        }
        for n in lo - 2..=lo + 2 {
            let [stack, register] = finishes(n);
            assert_eq!(stack, register, "n = {n}");
        }
    }

    #[test]
    fn one_fuel_slices_make_progress() {
        // A scheduler slice can grant less fuel than one register
        // instruction is worth; the program still finishes, in exactly as
        // many slices as on the stack VM.
        let program = compile(TAK);
        let slices = vms(&program).map(|mut vm| {
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
