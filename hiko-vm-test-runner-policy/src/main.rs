use hiko_vm::builder::VMBuilder;
use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;

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
        .max_fuel(100000000)
        .max_heap(1000000)
        .build();
    match vm.run() {
        Ok(()) => {
            for line in vm.get_output() {
                print!("{line}");
            }
        }
        Err(e) => {
            for line in vm.get_output() {
                print!("{line}");
            }
            eprintln!("error: {}", e.message);
            std::process::exit(1);
        }
    }
}
