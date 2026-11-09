//! Source → type check, under a small work limit. A program that type
//! checks (and imports nothing) is then compiled; the compiler's output must
//! pass the verifier, and running it must not panic.
//! Bounded regression seeds: `seeds/fuzz_typecheck` (relative to this fuzz crate);
//! pass that directory to cargo-fuzz as its initial corpus.
#![no_main]
use hiko_compile::compiler::Compiler;
use hiko_syntax::ast::Program;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_types::infer::InferCtx;
use hiko_vm::verify::verify_program;
use libfuzzer_sys::fuzz_target;

const MAX_TYPE_WORK: u64 = 200_000;

fn parse(source: &str) -> Option<Program> {
    let tokens = Lexer::new(source, 0).tokenize().ok()?;
    Parser::new(tokens).parse_program().ok()
}

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let Some(program) = parse(source) else {
        return;
    };
    let program = hiko_syntax::desugar::desugar_program(program);
    let program = hiko_syntax::constfold::fold_program(program);
    let mut ctx = InferCtx::new();
    ctx.interner = program.interner.clone();
    ctx.set_max_type_work(MAX_TYPE_WORK);
    if ctx.infer_program(&program).is_err() || source.contains("use") {
        // Imports read files; the type check above still covered them.
        return;
    }

    let program = parse(source).expect("parsed once already");
    let Ok((compiled, _)) = Compiler::compile(program) else {
        return;
    };
    if let Err(e) = verify_program(&compiled) {
        panic!("compiler output failed verification: {e}");
    }
    hiko_vm_fuzz::run_bounded(compiled);
});
