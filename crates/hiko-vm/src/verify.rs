use std::collections::{BTreeMap, BTreeSet, VecDeque};

use hiko_compile::chunk::{Chunk, CompiledProgram, Constant, FunctionProto};
use hiko_compile::op::{
    HANDLER_CLAUSE_RESUMES, Op, REC_CAPTURE_GROUP, REC_CAPTURE_LOCAL, REC_CAPTURE_UPVALUE,
};

#[derive(Debug, Clone)]
pub struct VerificationError {
    message: String,
}

impl VerificationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for VerificationError {}

#[derive(Debug, Clone)]
struct DecodedInst {
    start: usize,
    op: Op,
    stack: StackRule,
    successors: Vec<usize>,
    handler_clause_targets: Vec<usize>,
}

#[derive(Debug, Clone)]
enum StackRule {
    Exact { min_depth: usize, delta: isize },
    Terminal { min_depth: usize },
}

pub fn verify_program(program: &CompiledProgram) -> Result<(), VerificationError> {
    let effect_tags = program
        .effects
        .iter()
        .map(|effect| effect.tag)
        .collect::<BTreeSet<_>>();
    verify_chunk(
        &program.main,
        program.main_n_locals as usize,
        None,
        &program.functions,
        &effect_tags,
    )
    .map_err(|err| VerificationError::new(format!("main chunk: {err}")))?;

    for (proto_idx, proto) in program.functions.iter().enumerate() {
        verify_chunk(
            &proto.chunk,
            proto.n_locals as usize,
            Some(proto),
            &program.functions,
            &effect_tags,
        )
        .map_err(|err| {
            let name = proto
                .name
                .as_deref()
                .map(|name| format!("function {proto_idx} ('{name}')"))
                .unwrap_or_else(|| format!("function {proto_idx}"));
            VerificationError::new(format!("{name}: {err}"))
        })?;
    }

    Ok(())
}

fn verify_chunk(
    chunk: &Chunk,
    n_locals: usize,
    proto: Option<&FunctionProto>,
    functions: &[FunctionProto],
    effect_tags: &BTreeSet<u16>,
) -> Result<(), String> {
    let decoded = decode_chunk(chunk, n_locals, proto, functions, effect_tags)?;
    let initial_depth = proto.map_or(0usize, |p| p.arity as usize);
    verify_stack_effects(&decoded, initial_depth)?;
    Ok(())
}

fn decode_chunk(
    chunk: &Chunk,
    n_locals: usize,
    proto: Option<&FunctionProto>,
    functions: &[FunctionProto],
    effect_tags: &BTreeSet<u16>,
) -> Result<Vec<DecodedInst>, String> {
    let mut decoded = Vec::with_capacity(chunk.code.len() / 2);
    let mut starts = BTreeSet::new();
    let mut ip = 0usize;

    while ip < chunk.code.len() {
        starts.insert(ip);
        let start = ip;
        let op_byte = chunk.code[ip];
        ip += 1;
        let op =
            Op::try_from(op_byte).map_err(|b| format!("invalid opcode {b} at offset {start}"))?;
        let inst = decode_instruction(
            chunk,
            n_locals,
            proto,
            functions,
            effect_tags,
            start,
            op,
            &mut ip,
        )?;
        decoded.push(inst);
    }

    for inst in &decoded {
        for target in &inst.successors {
            if !starts.contains(target) {
                return Err(format!(
                    "control-flow target {target} does not point to an instruction boundary"
                ));
            }
        }
    }

    Ok(decoded)
}

#[allow(clippy::too_many_arguments)]
fn decode_instruction(
    chunk: &Chunk,
    n_locals: usize,
    proto: Option<&FunctionProto>,
    functions: &[FunctionProto],
    effect_tags: &BTreeSet<u16>,
    start: usize,
    op: Op,
    ip: &mut usize,
) -> Result<DecodedInst, String> {
    let read_u8 = |chunk: &Chunk, ip: &mut usize, what: &str| -> Result<u8, String> {
        if *ip >= chunk.code.len() {
            return Err(format!(
                "{what}: truncated bytecode at offset {start}, expected u8 operand"
            ));
        }
        let value = chunk.code[*ip];
        *ip += 1;
        Ok(value)
    };
    let read_u16 = |chunk: &Chunk, ip: &mut usize, what: &str| -> Result<u16, String> {
        if *ip + 1 >= chunk.code.len() {
            return Err(format!(
                "{what}: truncated bytecode at offset {start}, expected u16 operand"
            ));
        }
        let value = u16::from_le_bytes([chunk.code[*ip], chunk.code[*ip + 1]]);
        *ip += 2;
        Ok(value)
    };
    let read_i16 = |chunk: &Chunk, ip: &mut usize, what: &str| -> Result<i16, String> {
        read_u16(chunk, ip, what).map(|value| value as i16)
    };
    let read_relative_target =
        |base_after_operand: usize, offset: i16, what: &str| -> Result<usize, String> {
            let target = base_after_operand as isize + offset as isize;
            if target < 0 || target as usize > chunk.code.len() {
                return Err(format!(
                    "{what}: relative jump from offset {start} lands outside chunk at {target}"
                ));
            }
            Ok(target as usize)
        };

    let mut successors = Vec::new();
    let mut handler_clause_targets = Vec::new();
    let stack = match op {
        Op::Halt => StackRule::Terminal { min_depth: 0 },
        Op::Const => {
            let idx = read_u16(chunk, ip, "Const")? as usize;
            if idx >= chunk.constants.len() {
                return Err(format!(
                    "Const at offset {start} uses constant index {idx}, but constant pool length is {}",
                    chunk.constants.len()
                ));
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: 1,
            }
        }
        Op::Unit | Op::True | Op::False => {
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: 1,
            }
        }
        Op::GetLocal => {
            let slot = read_u16(chunk, ip, "GetLocal")? as usize;
            if slot >= n_locals {
                return Err(format!(
                    "GetLocal at offset {start} uses local slot {slot}, but chunk only has {n_locals} local(s)"
                ));
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: 1,
            }
        }
        Op::GetLocal0 | Op::GetLocal1 | Op::GetLocal2 | Op::GetLocal3 => {
            let slot = match op {
                Op::GetLocal0 => 0,
                Op::GetLocal1 => 1,
                Op::GetLocal2 => 2,
                Op::GetLocal3 => 3,
                _ => unreachable!(),
            };
            if slot >= n_locals {
                return Err(format!(
                    "GetLocal at offset {start} uses local slot {slot}, but chunk only has {n_locals} local(s)"
                ));
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: 1,
            }
        }
        Op::SetLocal => {
            let slot = read_u16(chunk, ip, "SetLocal")? as usize;
            if slot >= n_locals {
                return Err(format!(
                    "SetLocal at offset {start} uses local slot {slot}, but chunk only has {n_locals} local(s)"
                ));
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 1,
                delta: -1,
            }
        }
        Op::SetLocal0 | Op::SetLocal1 | Op::SetLocal2 | Op::SetLocal3 => {
            let slot = match op {
                Op::SetLocal0 => 0,
                Op::SetLocal1 => 1,
                Op::SetLocal2 => 2,
                Op::SetLocal3 => 3,
                _ => unreachable!(),
            };
            if slot >= n_locals {
                return Err(format!(
                    "SetLocal at offset {start} uses local slot {slot}, but chunk only has {n_locals} local(s)"
                ));
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 1,
                delta: -1,
            }
        }
        Op::GetUpvalue => {
            let idx = read_u16(chunk, ip, "GetUpvalue")? as usize;
            let captures = proto.map_or(0usize, |p| p.n_captures as usize);
            if idx >= captures {
                return Err(format!(
                    "GetUpvalue at offset {start} uses capture index {idx}, but function only declares {captures} capture(s)"
                ));
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: 1,
            }
        }
        Op::GetGlobal | Op::SetGlobal | Op::Panic => {
            let idx = read_u16(chunk, ip, "global/string constant")? as usize;
            match chunk.constants.get(idx) {
                Some(Constant::String(_)) => {}
                Some(_) => {
                    return Err(format!(
                        "{op:?} at offset {start} expects a string constant at index {idx}"
                    ));
                }
                None => {
                    return Err(format!(
                        "{op:?} at offset {start} uses constant index {idx}, but constant pool length is {}",
                        chunk.constants.len()
                    ));
                }
            }
            successors.push(*ip);
            match op {
                Op::GetGlobal => StackRule::Exact {
                    min_depth: 0,
                    delta: 1,
                },
                Op::SetGlobal => StackRule::Exact {
                    min_depth: 1,
                    delta: -1,
                },
                Op::Panic => StackRule::Terminal { min_depth: 0 },
                _ => unreachable!(),
            }
        }
        Op::Pop => {
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 1,
                delta: -1,
            }
        }
        Op::AddInt
        | Op::SubInt
        | Op::MulInt
        | Op::DivInt
        | Op::ModInt
        | Op::AddFloat
        | Op::SubFloat
        | Op::MulFloat
        | Op::DivFloat
        | Op::AddWord
        | Op::SubWord
        | Op::MulWord
        | Op::DivWord
        | Op::ModWord
        | Op::Eq
        | Op::Ne
        | Op::LtInt
        | Op::GtInt
        | Op::LeInt
        | Op::GeInt
        | Op::LtFloat
        | Op::GtFloat
        | Op::LeFloat
        | Op::GeFloat
        | Op::LtWord
        | Op::GtWord
        | Op::LeWord
        | Op::GeWord
        | Op::ConcatString => {
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 2,
                delta: -1,
            }
        }
        Op::Neg
        | Op::NegFloat
        | Op::Not
        | Op::GetField
        | Op::GetField0
        | Op::GetField1
        | Op::GetTag => {
            if matches!(op, Op::GetField) {
                let idx = read_u8(chunk, ip, "GetField")? as usize;
                let _ = idx;
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 1,
                delta: 0,
            }
        }
        Op::MakeTuple => {
            let arity = read_u8(chunk, ip, "MakeTuple")? as usize;
            successors.push(*ip);
            StackRule::Exact {
                min_depth: arity,
                delta: 1 - arity as isize,
            }
        }
        Op::MakeData => {
            let _tag = read_u16(chunk, ip, "MakeData")?;
            let arity = read_u8(chunk, ip, "MakeData")? as usize;
            successors.push(*ip);
            StackRule::Exact {
                min_depth: arity,
                delta: 1 - arity as isize,
            }
        }
        Op::MakeCons => {
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 2,
                delta: -1,
            }
        }
        Op::Jump => {
            let offset = read_i16(chunk, ip, "Jump")?;
            let target = read_relative_target(*ip, offset, "Jump")?;
            successors.push(target);
            StackRule::Exact {
                min_depth: 0,
                delta: 0,
            }
        }
        Op::JumpIfFalse => {
            let offset = read_i16(chunk, ip, "JumpIfFalse")?;
            let target = read_relative_target(*ip, offset, "JumpIfFalse")?;
            successors.push(target);
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 1,
                delta: -1,
            }
        }
        Op::MakeClosure => {
            let target_proto_idx = read_u16(chunk, ip, "MakeClosure")? as usize;
            let target_proto = functions.get(target_proto_idx).ok_or_else(|| {
                format!(
                    "MakeClosure at offset {start} references function proto {target_proto_idx}, but only {} function(s) exist",
                    functions.len()
                )
            })?;
            let n_captures = read_u8(chunk, ip, "MakeClosure")? as usize;
            if n_captures != target_proto.n_captures as usize {
                return Err(format!(
                    "MakeClosure at offset {start} captures {n_captures} value(s), but target function expects {} capture(s)",
                    target_proto.n_captures
                ));
            }
            let current_captures = proto.map_or(0usize, |p| p.n_captures as usize);
            for capture_idx in 0..n_captures {
                let is_local = read_u8(chunk, ip, "MakeClosure capture")? != 0;
                let index = read_u16(chunk, ip, "MakeClosure capture")? as usize;
                if is_local {
                    if index >= n_locals {
                        return Err(format!(
                            "MakeClosure at offset {start} capture #{capture_idx} references local slot {index}, but current chunk only has {n_locals} local(s)"
                        ));
                    }
                } else if index >= current_captures {
                    return Err(format!(
                        "MakeClosure at offset {start} capture #{capture_idx} references upvalue {index}, but current function only has {current_captures} capture(s)"
                    ));
                }
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: 1,
            }
        }
        Op::MakeRecClosures => {
            let n_closures = read_u8(chunk, ip, "MakeRecClosures")? as usize;
            if n_closures == 0 {
                return Err(format!(
                    "MakeRecClosures at offset {start} creates no closures"
                ));
            }
            let current_captures = proto.map_or(0usize, |p| p.n_captures as usize);
            for member in 0..n_closures {
                let target_proto_idx = read_u16(chunk, ip, "MakeRecClosures")? as usize;
                let target_proto = functions.get(target_proto_idx).ok_or_else(|| {
                    format!(
                        "MakeRecClosures at offset {start} member #{member} references function proto {target_proto_idx}, but only {} function(s) exist",
                        functions.len()
                    )
                })?;
                let n_captures = read_u8(chunk, ip, "MakeRecClosures")? as usize;
                if n_captures != target_proto.n_captures as usize {
                    return Err(format!(
                        "MakeRecClosures at offset {start} member #{member} captures {n_captures} value(s), but target function expects {} capture(s)",
                        target_proto.n_captures
                    ));
                }
                for capture_idx in 0..n_captures {
                    let source = read_u8(chunk, ip, "MakeRecClosures capture")?;
                    let index = read_u16(chunk, ip, "MakeRecClosures capture")? as usize;
                    let (what, limit) = match source {
                        REC_CAPTURE_UPVALUE => ("upvalue", current_captures),
                        REC_CAPTURE_LOCAL => ("local slot", n_locals),
                        REC_CAPTURE_GROUP => ("group member", n_closures),
                        _ => {
                            return Err(format!(
                                "MakeRecClosures at offset {start} member #{member} capture #{capture_idx} has unknown source {source}"
                            ));
                        }
                    };
                    if index >= limit {
                        return Err(format!(
                            "MakeRecClosures at offset {start} member #{member} capture #{capture_idx} references {what} {index}, but only {limit} exist"
                        ));
                    }
                }
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: n_closures as isize,
            }
        }
        Op::Call => {
            let arity = read_u8(chunk, ip, "Call")? as usize;
            successors.push(*ip);
            StackRule::Exact {
                min_depth: arity + 1,
                delta: -(arity as isize),
            }
        }
        Op::TailCall => {
            let arity = read_u8(chunk, ip, "TailCall")? as usize;
            StackRule::Terminal {
                min_depth: arity + 1,
            }
        }
        Op::Return => StackRule::Terminal { min_depth: 1 },
        Op::CallDirect | Op::TailCallDirect => {
            let target_proto_idx = read_u16(chunk, ip, "direct call")? as usize;
            let target_proto = functions.get(target_proto_idx).ok_or_else(|| {
                format!(
                    "{op:?} at offset {start} references function proto {target_proto_idx}, but only {} function(s) exist",
                    functions.len()
                )
            })?;
            let arity = target_proto.arity as usize;
            if matches!(op, Op::CallDirect) {
                successors.push(*ip);
                StackRule::Exact {
                    min_depth: arity,
                    delta: 1 - arity as isize,
                }
            } else {
                StackRule::Terminal { min_depth: arity }
            }
        }
        Op::InstallHandler => {
            let n_clauses = read_u16(chunk, ip, "InstallHandler")? as usize;
            for _ in 0..n_clauses {
                let effect_tag = read_u16(chunk, ip, "InstallHandler clause")?;
                validate_effect_tag(effect_tag, effect_tags, "InstallHandler clause")?;
                let flags = read_u8(chunk, ip, "InstallHandler clause")?;
                if flags & !HANDLER_CLAUSE_RESUMES != 0 {
                    return Err(format!(
                        "InstallHandler at offset {start} has clause with invalid flags {flags:#04x}"
                    ));
                }
                let offset = read_i16(chunk, ip, "InstallHandler clause")?;
                let target = read_relative_target(*ip, offset, "InstallHandler clause")?;
                handler_clause_targets.push(target);
            }
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: 0,
            }
        }
        Op::RemoveHandler => {
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: 0,
            }
        }
        Op::Perform => {
            let effect_tag = read_u16(chunk, ip, "Perform")?;
            validate_effect_tag(effect_tag, effect_tags, "Perform")?;
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 1,
                delta: 0,
            }
        }
        Op::Resume => {
            // The resumed continuation runs until its outermost frame
            // returns, then execution continues here with that result.
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 2,
                delta: -1,
            }
        }
    };

    Ok(DecodedInst {
        start,
        op,
        stack,
        successors,
        handler_clause_targets,
    })
}

fn validate_effect_tag(
    effect_tag: u16,
    effect_tags: &BTreeSet<u16>,
    context: &str,
) -> Result<(), String> {
    if effect_tags.contains(&effect_tag) {
        Ok(())
    } else {
        Err(format!(
            "{context} references unknown effect tag {effect_tag}"
        ))
    }
}

fn verify_stack_effects(decoded: &[DecodedInst], initial_depth: usize) -> Result<(), String> {
    if decoded.is_empty() {
        return Ok(());
    }

    let mut by_start = BTreeMap::new();
    for (idx, inst) in decoded.iter().enumerate() {
        by_start.insert(inst.start, idx);
    }

    let mut pending = VecDeque::new();
    let mut depths = BTreeMap::new();
    pending.push_back((decoded[0].start, initial_depth));

    while let Some((start, depth)) = pending.pop_front() {
        match depths.get(&start).copied() {
            Some(existing) if depth >= existing => continue,
            Some(_) => {
                depths.insert(start, depth);
            }
            None => {
                depths.insert(start, depth);
            }
        }

        let inst = &decoded[*by_start
            .get(&start)
            .expect("decoded instruction starts should be indexed")];

        let successor_depth = match &inst.stack {
            StackRule::Exact { min_depth, delta } => {
                if depth < *min_depth {
                    return Err(format!(
                        "{:?} at offset {} requires stack depth >= {}, but verifier reached it with depth {}",
                        inst.op, inst.start, min_depth, depth
                    ));
                }
                apply_delta(inst, depth, *delta)?
            }
            StackRule::Terminal { min_depth } => {
                if depth < *min_depth {
                    return Err(format!(
                        "{:?} at offset {} requires stack depth >= {}, but verifier reached it with depth {}",
                        inst.op, inst.start, min_depth, depth
                    ));
                }
                continue;
            }
        };

        for target in &inst.successors {
            pending.push_back((*target, successor_depth));
        }
        if !inst.handler_clause_targets.is_empty() {
            // Every clause enters with the payload and a continuation slot.
            // For a clause without HANDLER_CLAUSE_RESUMES the slot holds a
            // Unit placeholder instead of a captured continuation.
            let clause_depth = successor_depth.checked_add(2).ok_or_else(|| {
                format!(
                    "{:?} at offset {} overflows stack depth for handler clause entry",
                    inst.op, inst.start
                )
            })?;
            for target in &inst.handler_clause_targets {
                pending.push_back((*target, clause_depth));
            }
        }
    }

    Ok(())
}
fn apply_delta(inst: &DecodedInst, depth: usize, delta: isize) -> Result<usize, String> {
    if delta >= 0 {
        depth.checked_add(delta as usize).ok_or_else(|| {
            format!(
                "{:?} at offset {} overflows stack depth",
                inst.op, inst.start
            )
        })
    } else {
        depth.checked_sub((-delta) as usize).ok_or_else(|| {
            format!(
                "{:?} at offset {} underflows stack depth while applying verifier delta {}",
                inst.op, inst.start, delta
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::verify_program;
    use hiko_compile::chunk::{Chunk, CompiledProgram, Constant, EffectMeta, FunctionProto};
    use hiko_compile::op::{HANDLER_CLAUSE_RESUMES, Op, REC_CAPTURE_GROUP, REC_CAPTURE_UPVALUE};
    use std::sync::Arc;

    fn empty_program(code: Vec<u8>) -> CompiledProgram {
        program_with_effects(code, 0, Vec::new())
    }

    fn program_with_effects(
        code: Vec<u8>,
        main_n_locals: u16,
        effects: Vec<EffectMeta>,
    ) -> CompiledProgram {
        CompiledProgram {
            main: Arc::new(Chunk {
                code,
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            main_n_locals,
            functions: Arc::from([]),
            effects: Arc::from(effects),
        }
    }

    fn proto(code: Vec<u8>, arity: u8, n_captures: u8, n_locals: u16) -> FunctionProto {
        FunctionProto {
            name: None,
            arity,
            n_captures,
            n_locals,
            chunk: Chunk {
                code,
                constants: Vec::new(),
                spans: Vec::new(),
            },
        }
    }

    fn effect_zero() -> EffectMeta {
        EffectMeta {
            name: "Test".into(),
            tag: 0,
        }
    }

    #[test]
    fn rejects_out_of_bounds_constant_index() {
        let program = empty_program(vec![Op::Const as u8, 0, 0, Op::Halt as u8]);
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("constant index 0"));
    }

    #[test]
    fn rejects_jump_outside_chunk() {
        let program = empty_program(vec![Op::Jump as u8, 0xff, 0x7f]);
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("lands outside chunk"));
    }

    #[test]
    fn rejects_string_constant_type_mismatch() {
        let program = CompiledProgram {
            main: Arc::new(Chunk {
                code: vec![Op::GetGlobal as u8, 0, 0, Op::Halt as u8],
                constants: vec![Constant::Int(1)],
                spans: Vec::new(),
            }),
            main_n_locals: 0,
            functions: Arc::from([]),
            effects: Arc::from([]),
        };
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("expects a string constant"));
    }

    #[test]
    fn rejects_main_get_local_out_of_bounds() {
        let program = empty_program(vec![Op::GetLocal as u8, 0, 0, Op::Halt as u8]);
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("GetLocal"));
        assert!(err.message().contains("local slot 0"));
    }

    #[test]
    fn accepts_main_get_local_with_declared_local_slot() {
        let program = program_with_effects(
            vec![Op::GetLocal as u8, 0, 0, Op::Pop as u8, Op::Halt as u8],
            1,
            Vec::new(),
        );
        verify_program(&program).expect("declared main local slot should verify");
    }

    #[test]
    fn accepts_quick_local_and_make_cons_stack_rules() {
        let program = program_with_effects(
            vec![
                Op::GetLocal0 as u8,
                Op::GetLocal1 as u8,
                Op::MakeCons as u8,
                Op::Pop as u8,
                Op::Halt as u8,
            ],
            2,
            Vec::new(),
        );
        verify_program(&program).expect("quick local and MakeCons should verify");
    }

    #[test]
    fn rejects_quick_local_out_of_bounds() {
        let program =
            program_with_effects(vec![Op::GetLocal3 as u8, Op::Halt as u8], 3, Vec::new());
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("GetLocal"));
        assert!(err.message().contains("local slot 3"));
    }

    #[test]
    fn rejects_set_local_out_of_bounds_in_main() {
        let cases = [
            (vec![Op::Unit as u8, Op::SetLocal3 as u8, Op::Halt as u8], 3),
            (
                vec![Op::Unit as u8, Op::SetLocal as u8, 2, 0, Op::Halt as u8],
                2,
            ),
        ];
        for (code, slot) in cases {
            let program = program_with_effects(code, slot, Vec::new());
            let err = verify_program(&program).expect_err("program should fail verification");
            assert!(err.message().contains("SetLocal"), "{}", err.message());
            assert!(
                err.message().contains(&format!("local slot {slot}")),
                "{}",
                err.message()
            );
        }
    }

    #[test]
    fn rejects_function_get_local_out_of_bounds() {
        let function = proto(vec![Op::GetLocal as u8, 1, 0, Op::Return as u8], 0, 0, 1);
        let program = CompiledProgram {
            main: Arc::new(Chunk {
                code: vec![Op::Halt as u8],
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            main_n_locals: 0,
            functions: Arc::from([function]),
            effects: Arc::from([]),
        };
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("GetLocal"));
        assert!(err.message().contains("local slot 1"));
    }

    #[test]
    fn rejects_function_set_local_out_of_bounds() {
        let function = proto(
            vec![Op::Unit as u8, Op::SetLocal as u8, 1, 0, Op::Return as u8],
            0,
            0,
            1,
        );
        let program = CompiledProgram {
            main: Arc::new(Chunk {
                code: vec![Op::Halt as u8],
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            main_n_locals: 0,
            functions: Arc::from([function]),
            effects: Arc::from([]),
        };
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("SetLocal"));
        assert!(err.message().contains("local slot 1"));
    }

    #[test]
    fn rejects_make_closure_local_capture_out_of_bounds() {
        let captured = proto(vec![Op::GetUpvalue as u8, 0, 0, Op::Return as u8], 0, 1, 0);
        let maker = proto(
            vec![Op::MakeClosure as u8, 0, 0, 1, 1, 1, 0, Op::Return as u8],
            0,
            0,
            1,
        );
        let program = CompiledProgram {
            main: Arc::new(Chunk {
                code: vec![Op::Halt as u8],
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            main_n_locals: 0,
            functions: Arc::from([captured, maker]),
            effects: Arc::from([]),
        };
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("MakeClosure"));
        assert!(err.message().contains("local slot 1"));
    }

    /// Main chunk running `code` with one function that has one capture.
    fn rec_closures_program(code: Vec<u8>) -> CompiledProgram {
        let member = proto(vec![Op::GetUpvalue as u8, 0, 0, Op::Return as u8], 1, 1, 1);
        CompiledProgram {
            main: Arc::new(Chunk {
                code,
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            main_n_locals: 2,
            functions: Arc::from([member]),
            effects: Arc::from([]),
        }
    }

    #[test]
    fn accepts_make_rec_closures_group() {
        // Two closures of function 0, each capturing the other.
        let program = rec_closures_program(vec![
            Op::MakeRecClosures as u8,
            2,
            0,
            0,
            1,
            REC_CAPTURE_GROUP,
            1,
            0,
            0,
            0,
            1,
            REC_CAPTURE_GROUP,
            0,
            0,
            Op::Pop as u8,
            Op::Pop as u8,
            Op::Halt as u8,
        ]);
        verify_program(&program).expect("MakeRecClosures group should verify");
    }

    #[test]
    fn rejects_make_rec_closures_bad_captures() {
        let cases: [(Vec<u8>, &str); 4] = [
            // Group member index past the group size.
            (
                vec![
                    Op::MakeRecClosures as u8,
                    1,
                    0,
                    0,
                    1,
                    REC_CAPTURE_GROUP,
                    1,
                    0,
                ],
                "group member 1",
            ),
            // Main chunk has no upvalues.
            (
                vec![
                    Op::MakeRecClosures as u8,
                    1,
                    0,
                    0,
                    1,
                    REC_CAPTURE_UPVALUE,
                    0,
                    0,
                ],
                "upvalue 0",
            ),
            // Unknown capture source.
            (
                vec![Op::MakeRecClosures as u8, 1, 0, 0, 1, 3, 0, 0],
                "unknown source 3",
            ),
            // Capture count differs from the function's.
            (
                vec![Op::MakeRecClosures as u8, 1, 0, 0, 0],
                "expects 1 capture",
            ),
        ];
        for (mut code, expected) in cases {
            code.push(Op::Halt as u8);
            let err = verify_program(&rec_closures_program(code))
                .expect_err("program should fail verification");
            assert!(
                err.message().contains(expected),
                "expected '{expected}' in: {}",
                err.message()
            );
        }
        let err = verify_program(&rec_closures_program(vec![
            Op::MakeRecClosures as u8,
            0,
            Op::Halt as u8,
        ]))
        .expect_err("empty group should fail verification");
        assert!(err.message().contains("creates no closures"));
    }

    #[test]
    fn rejects_perform_without_payload() {
        let program = program_with_effects(
            vec![Op::Perform as u8, 0, 0, Op::Halt as u8],
            0,
            vec![effect_zero()],
        );
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("requires stack depth >= 1"));
    }

    #[test]
    fn rejects_perform_unknown_effect_tag() {
        let program = empty_program(vec![
            Op::Unit as u8,
            Op::Perform as u8,
            7,
            0,
            Op::Halt as u8,
        ]);
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("unknown effect tag 7"));
    }

    #[test]
    fn rejects_install_handler_unknown_effect_tag() {
        let program = empty_program(vec![
            Op::InstallHandler as u8,
            1,
            0,
            7,
            0,
            HANDLER_CLAUSE_RESUMES,
            0,
            0,
            Op::Halt as u8,
        ]);
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("unknown effect tag 7"));
    }

    /// One-clause handler for effect tag 0 whose clause pops `clause_pops`
    /// values and halts. The clause starts at offset 10.
    fn one_clause_handler(flags: u8, clause_pops: usize) -> CompiledProgram {
        let mut code = vec![
            Op::InstallHandler as u8,
            1,
            0,
            0,
            0,
            flags,
            2,
            0,
            Op::Unit as u8,
            Op::Halt as u8,
        ];
        code.extend(std::iter::repeat_n(Op::Pop as u8, clause_pops));
        code.push(Op::Halt as u8);
        program_with_effects(code, 0, vec![effect_zero()])
    }

    #[test]
    fn accepts_install_handler_clause_flags() {
        for flags in [0, HANDLER_CLAUSE_RESUMES] {
            verify_program(&one_clause_handler(flags, 2))
                .unwrap_or_else(|err| panic!("flags {flags}: {err}"));
        }
    }

    #[test]
    fn rejects_install_handler_invalid_clause_flags() {
        for flags in [0x02, 0x80, 0xff] {
            let err = verify_program(&one_clause_handler(flags, 2))
                .expect_err("program should fail verification");
            assert!(
                err.message().contains("invalid flags"),
                "flags {flags}: {}",
                err.message()
            );
        }
    }

    #[test]
    fn handler_clause_entry_has_payload_and_continuation_slots() {
        // Both kinds of clause enter with two values: the payload and either
        // the continuation or a placeholder.
        for flags in [0, HANDLER_CLAUSE_RESUMES] {
            let err = verify_program(&one_clause_handler(flags, 3))
                .expect_err("program should fail verification");
            assert!(
                err.message().contains("Pop at offset 12"),
                "flags {flags}: {}",
                err.message()
            );
        }
    }

    #[test]
    fn rejects_resume_without_continuation_and_argument() {
        let program = empty_program(vec![Op::Resume as u8, Op::Halt as u8]);
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("requires stack depth >= 2"));
    }

    #[test]
    fn verifies_stack_effects_after_resume() {
        let program = empty_program(vec![
            Op::Unit as u8,
            Op::Unit as u8,
            Op::Resume as u8,
            Op::Pop as u8,
            Op::Pop as u8,
            Op::Halt as u8,
        ]);
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(
            err.message().contains("Pop at offset 4"),
            "{}",
            err.message()
        );
    }
}
