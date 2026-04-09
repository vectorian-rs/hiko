use std::path::Path;

fn run_hiko_file(path: &str) {
    let source = std::fs::read_to_string(path).expect(&format!("cannot read {path}"));
    let tokens = hiko_syntax::lexer::Lexer::new(&source, 0)
        .tokenize()
        .expect("lex error");
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse error");
    let (compiled, warnings) =
        hiko_compile::compiler::Compiler::compile_file(&program, Path::new(path))
            .expect("compile error");
    for w in &warnings {
        eprintln!("Warning: {}", w.message);
    }
    let mut vm = hiko_vm::vm::VM::new(compiled);
    vm.run().expect(&format!("runtime error in {path}"));
}

#[test]
fn test_stdlib_list() {
    run_hiko_file("../../tests/run/test_list.hk");
}

#[test]
fn test_stdlib_option() {
    run_hiko_file("../../tests/run/test_option.hk");
}

#[test]
fn test_stdlib_either() {
    run_hiko_file("../../tests/run/test_either.hk");
}
