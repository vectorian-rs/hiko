use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_vm::builder::{ExecPolicy, VMBuilder};
use hiko_vm::runtime::Runtime;
use std::sync::Arc;

fn main() {
    let path = std::env::args().nth(1).expect("usage: <script.hml>");
    let source = std::fs::read_to_string(&path).expect("cannot read file");
    let tokens = Lexer::new(&source, 0).tokenize().expect("lex error");
    let program = Parser::new(tokens).parse_program().expect("parse error");
    let (compiled, _) =
        Compiler::compile_file(program, std::path::Path::new(&path)).expect("compile error");
    let mut vm = VMBuilder::new(compiled)
        .with_all()
        .with_exec(ExecPolicy {
            allowed: vec![
                "./target/release/hiko-cli".into(),
                "./hiko-vm-hiko-run-all-policy/target/release/hiko-vm-hiko-run-all-policy".into(),
                "./target/release/hiko-vm-hiko-run-all-policy".into(),
            ],
            timeout: 30,
        })
        .max_fuel(100000000)
        .max_heap(1000000)
        .build();
    vm.set_output_sink(Arc::new(hiko_vm::vm::StdoutOutputSink::default()));
    let mut runtime = Runtime::new();
    let pid = runtime.spawn_root_vm(vm);
    match runtime.run_to_completion() {
        Ok(_) => {
            if let Some(hiko_vm::process::ProcessStatus::Failed(msg)) = runtime.get_status(pid) {
                eprintln!("error: {}", msg);
                std::process::exit(1);
            }
        }
        Err(msg) => {
            eprintln!("error: {}", msg);
            std::process::exit(1);
        }
    }
}
