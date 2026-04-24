use std::collections::{BTreeMap, BTreeSet, VecDeque};

use hiko_compile::chunk::{Chunk, CompiledProgram, Constant, FunctionProto};
use hiko_compile::op::Op;

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
    verify_chunk(&program.main, None, &program.functions)
        .map_err(|err| VerificationError::new(format!("main chunk: {err}")))?;

    for (proto_idx, proto) in program.functions.iter().enumerate() {
        verify_chunk(&proto.chunk, Some(proto), &program.functions).map_err(|err| {
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
    proto: Option<&FunctionProto>,
    functions: &[FunctionProto],
) -> Result<(), String> {
    let decoded = decode_chunk(chunk, proto, functions)?;
    let initial_depth = proto.map_or(0usize, |p| p.arity as usize);
    verify_stack_effects(&decoded, initial_depth)?;
    Ok(())
}

fn decode_chunk(
    chunk: &Chunk,
    proto: Option<&FunctionProto>,
    functions: &[FunctionProto],
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
        let inst = decode_instruction(chunk, proto, functions, start, op, &mut ip)?;
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

fn decode_instruction(
    chunk: &Chunk,
    proto: Option<&FunctionProto>,
    functions: &[FunctionProto],
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
            let _slot = read_u16(chunk, ip, "GetLocal")? as usize;
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 0,
                delta: 1,
            }
        }
        Op::SetLocal => {
            let _slot = read_u16(chunk, ip, "SetLocal")? as usize;
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
        | Op::ConcatString => {
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 2,
                delta: -1,
            }
        }
        Op::Neg | Op::NegFloat | Op::Not | Op::GetField | Op::GetTag => {
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
                if !is_local && index >= current_captures {
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
                let _effect_tag = read_u16(chunk, ip, "InstallHandler clause")?;
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
            let _effect_tag = read_u16(chunk, ip, "Perform")?;
            successors.push(*ip);
            StackRule::Exact {
                min_depth: 1,
                delta: 0,
            }
        }
        Op::Resume => StackRule::Terminal { min_depth: 2 },
    };

    Ok(DecodedInst {
        start,
        op,
        stack,
        successors,
        handler_clause_targets,
    })
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
    use hiko_compile::chunk::{Chunk, CompiledProgram, Constant};
    use hiko_compile::op::Op;
    use std::sync::Arc;

    fn empty_program(code: Vec<u8>) -> CompiledProgram {
        CompiledProgram {
            main: Arc::new(Chunk {
                code,
                constants: Vec::new(),
                spans: Vec::new(),
            }),
            functions: Arc::from([]),
            effects: Arc::from([]),
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
            functions: Arc::from([]),
            effects: Arc::from([]),
        };
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("expects a string constant"));
    }

    #[test]
    fn rejects_perform_without_payload() {
        let program = empty_program(vec![Op::Perform as u8, 0, 0, Op::Halt as u8]);
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("requires stack depth >= 1"));
    }

    #[test]
    fn rejects_resume_without_continuation_and_argument() {
        let program = empty_program(vec![Op::Resume as u8]);
        let err = verify_program(&program).expect_err("program should fail verification");
        assert!(err.message().contains("requires stack depth >= 2"));
    }
}
