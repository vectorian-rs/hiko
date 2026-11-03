//! Structured random programs → verify → run. Each chunk is a list of
//! instructions, an opcode byte followed by a few operand bytes, so most
//! inputs get past opcode decoding and exercise the verifier's checks.
#![no_main]
use std::sync::Arc;

use arbitrary::Arbitrary;
use hiko_compile::chunk::{Chunk, CompiledProgram, Constant, EffectMeta, FunctionProto};
use hiko_vm::verify::verify_program;
use libfuzzer_sys::fuzz_target;

/// One past the highest opcode (`JumpIfLocalNotTag` = 83).
const OPCODE_COUNT: u8 = 84;

#[derive(Arbitrary, Debug)]
struct Inst {
    op: u8,
    operands: Vec<u8>,
}

#[derive(Arbitrary, Debug)]
enum Const {
    Int(i64),
    Float(f64),
    Word(u64),
    String(String),
    Char(char),
}

#[derive(Arbitrary, Debug)]
struct FuzzChunk {
    code: Vec<Inst>,
    constants: Vec<Const>,
}

#[derive(Arbitrary, Debug)]
struct FuzzFunction {
    arity: u8,
    n_captures: u8,
    n_locals: u8,
    chunk: FuzzChunk,
}

#[derive(Arbitrary, Debug)]
struct FuzzProgram {
    main: FuzzChunk,
    main_n_locals: u8,
    functions: Vec<FuzzFunction>,
    effect_tags: Vec<u16>,
    globals: Vec<String>,
}

fn chunk(fuzz: FuzzChunk) -> Chunk {
    let mut chunk = Chunk::default();
    for inst in fuzz.code {
        chunk.code.push(inst.op % OPCODE_COUNT);
        chunk.code.extend(inst.operands.iter().take(6));
    }
    chunk.constants = fuzz
        .constants
        .into_iter()
        .map(|c| match c {
            Const::Int(n) => Constant::Int(n),
            Const::Float(x) => Constant::Float(x),
            Const::Word(w) => Constant::Word(w),
            Const::String(s) => Constant::String(s),
            Const::Char(c) => Constant::Char(c),
        })
        .collect();
    chunk
}

fuzz_target!(|fuzz: FuzzProgram| {
    let functions: Vec<_> = fuzz
        .functions
        .into_iter()
        .map(|f| FunctionProto {
            name: None,
            arity: f.arity,
            n_captures: f.n_captures,
            n_locals: f.n_locals.into(),
            chunk: chunk(f.chunk),
        })
        .collect();
    let effects: Vec<_> = fuzz
        .effect_tags
        .into_iter()
        .map(|tag| EffectMeta {
            name: format!("E{tag}"),
            tag,
        })
        .collect();
    let program = CompiledProgram {
        main: Arc::new(chunk(fuzz.main)),
        main_n_locals: fuzz.main_n_locals.into(),
        functions: functions.into(),
        effects: effects.into(),
        globals: fuzz.globals.into_iter().map(Arc::from).collect(),
    };
    if verify_program(&program).is_ok() {
        hiko_vm_fuzz::run_bounded(program);
    }
});
