use std::path::Path;

fn run_hiko_file(path: &str) {
    let source = std::fs::read_to_string(path).unwrap_or_else(|_| panic!("cannot read {path}"));
    let tokens = hiko_syntax::lexer::Lexer::new(&source, 0)
        .tokenize()
        .expect("lex error");
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse error");
    let (compiled, warnings) =
        hiko_compile::compiler::Compiler::compile_file(program, Path::new(path))
            .expect("compile error");
    for w in &warnings {
        eprintln!("Warning: {}", w.message);
    }
    let mut vm = hiko_vm::vm::VM::new(compiled);
    vm.run()
        .unwrap_or_else(|_| panic!("runtime error in {path}"));
}

#[test]
fn test_stdlib_list() {
    run_hiko_file("../../tests/run/test_list.hml");
}

#[test]
fn test_stdlib_option() {
    run_hiko_file("../../tests/run/test_option.hml");
}

#[test]
fn test_stdlib_either() {
    run_hiko_file("../../tests/run/test_either.hml");
}

#[test]
fn test_stdlib_result() {
    run_hiko_file("../../tests/run/test_result.hml");
}

#[test]
fn test_stdlib_time() {
    run_hiko_file("../../tests/run/test_time.hml");
}

#[test]
fn test_stdlib_date() {
    run_hiko_file("../../tests/run/test_date.hml");
}

#[test]
fn test_stdlib_hashline() {
    run_hiko_file("../../tests/run/test_hashline.hml");
}

#[test]
fn test_numeric_module_examples() {
    run_hiko_file("../../examples/numeric_modules.hml");
}
