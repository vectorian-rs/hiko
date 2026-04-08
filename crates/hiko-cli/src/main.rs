use std::env;
use std::fs;
use std::process;

use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_vm::vm::VM;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: hiko <command> [args]");
        eprintln!("Commands:");
        eprintln!("  run <file.hk>    Compile and execute a program");
        eprintln!("  check <file.hk>  Type-check without executing");
        process::exit(1);
    }

    match args[1].as_str() {
        "run" => {
            if args.len() < 3 {
                eprintln!("Usage: hiko run <file.hk>");
                process::exit(1);
            }
            run_file(&args[2]);
        }
        "check" => {
            if args.len() < 3 {
                eprintln!("Usage: hiko check <file.hk>");
                process::exit(1);
            }
            check_file(&args[2]);
        }
        other => {
            eprintln!("Unknown command: {other}");
            eprintln!("Try: hiko run <file.hk>");
            process::exit(1);
        }
    }
}

fn run_file(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {path}: {e}");
            process::exit(1);
        }
    };

    let tokens = match Lexer::new(&source, 0).tokenize() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Lex error: {}", e.message);
            process::exit(1);
        }
    };

    let program = match Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Parse error: {}", e.message);
            process::exit(1);
        }
    };

    let compiled = match Compiler::compile(&program) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Compile error: {e:?}");
            process::exit(1);
        }
    };

    let mut vm = VM::new(compiled);
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
            eprintln!("Runtime error: {}", e.message);
            process::exit(1);
        }
    }
}

fn check_file(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {path}: {e}");
            process::exit(1);
        }
    };

    let tokens = match Lexer::new(&source, 0).tokenize() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Lex error: {}", e.message);
            process::exit(1);
        }
    };

    let program = match Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Parse error: {}", e.message);
            process::exit(1);
        }
    };

    let mut ctx = hiko_types::infer::InferCtx::new();
    match ctx.infer_program(&program) {
        Ok(()) => println!("OK"),
        Err(e) => {
            eprintln!("Type error: {}", e.message);
            process::exit(1);
        }
    }
}
