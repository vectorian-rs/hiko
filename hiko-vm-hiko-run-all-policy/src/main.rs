use hiko_vm::builder::VMBuilder;
use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use std::sync::Arc;

fn main() {
    let path = std::env::args().nth(1).expect("usage: <script.hml>");
    let source = std::fs::read_to_string(&path).expect("cannot read file");
    let tokens = Lexer::new(&source, 0).tokenize().expect("lex error");
    let program = Parser::new(tokens).parse_program().expect("parse error");
    let (compiled, _) = Compiler::compile_file(program, std::path::Path::new(&path)).expect("compile error");
    let mut vm = VMBuilder::new(compiled)
        .with_core()
        .with_filesystem(hiko_vm::builder::FilesystemPolicy {
            root: ".".into(),
            allow_read: true,
            allow_write: false,
            allow_delete: false,
        })
        .with_exec(hiko_vm::builder::ExecPolicy {
            allowed: vec!["./target/release/hiko-cli".into()],
            timeout: 30,
        })
        .max_fuel(10000000)
        .max_heap(100000)
        .build();
    vm.set_output_sink(Arc::new(hiko_vm::vm::StdoutOutputSink::default()));
    match vm.run() {
        Ok(()) => {}
        Err(e) => {
            eprintln!("error: {}", e.message);
            std::process::exit(1);
        }
    }
}
