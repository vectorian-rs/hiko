use std::env;
use std::fs;
use std::process;

use codespan_reporting::diagnostic::{Diagnostic, Label, Severity};
use codespan_reporting::files::SimpleFiles;
use codespan_reporting::term;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};

use hiko_compile::compiler::{CompileError, Compiler};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::span::Span;
use hiko_vm::vm::VM;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: hiko <command> [args]");
        eprintln!("Commands:");
        eprintln!("  run <file.hml>         Compile and execute a program");
        eprintln!("  check <file.hml>       Type-check without executing");
        eprintln!("  build-vm <policy.toml>  Generate a custom VM from a policy file");
        process::exit(1);
    }

    match args[1].as_str() {
        "run" => {
            if args.len() < 3 {
                eprintln!("Usage: hiko run <file.hml>");
                process::exit(1);
            }
            run_file(&args[2]);
        }
        "check" => {
            if args.len() < 3 {
                eprintln!("Usage: hiko check <file.hml>");
                process::exit(1);
            }
            check_file(&args[2]);
        }
        "build-vm" => {
            if args.len() < 3 {
                eprintln!("Usage: hiko build-vm <policy.toml>");
                process::exit(1);
            }
            build_vm(&args[2]);
        }
        other => {
            eprintln!("Unknown command: {other}");
            eprintln!("Try: hiko run <file.hml>");
            process::exit(1);
        }
    }
}

// ── Diagnostics ──────────────────────────────────────────────────────

struct DiagCtx {
    files: SimpleFiles<String, String>,
    file_id: usize,
}

impl DiagCtx {
    fn new(name: &str, source: String) -> Self {
        let mut files = SimpleFiles::new();
        let file_id = files.add(name.to_string(), source);
        Self { files, file_id }
    }

    fn emit(&self, severity: Severity, message: &str, span: Option<Span>) {
        let writer = StandardStream::stderr(ColorChoice::Auto);
        let config = term::Config::default();
        let diag = if let Some(span) = span {
            Diagnostic::new(severity)
                .with_message(message)
                .with_labels(vec![Label::primary(
                    self.file_id,
                    span.start as usize..span.end as usize,
                )])
        } else {
            Diagnostic::new(severity).with_message(message)
        };
        term::emit(&mut writer.lock(), &config, &self.files, &diag).ok();
    }

    fn error(&self, message: &str, span: Option<Span>) {
        self.emit(Severity::Error, message, span);
    }

    fn warning(&self, message: &str, span: Option<Span>) {
        self.emit(Severity::Warning, message, span);
    }
}

// ── Shared pipeline ──────────────────────────────────────────────────

struct Compiled {
    program: hiko_compile::chunk::CompiledProgram,
    warnings: Vec<hiko_types::infer::Warning>,
    ctx: DiagCtx,
}

fn compile_source(path: &str) -> Result<Compiled, ()> {
    let source = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {path}: {e}");
        process::exit(1);
    });

    let ctx = DiagCtx::new(path, source.clone());

    let tokens = match Lexer::new(&source, 0).tokenize() {
        Ok(t) => t,
        Err(e) => {
            ctx.error(&e.message, Some(e.span));
            return Err(());
        }
    };

    let program = match Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(e) => {
            ctx.error(&e.message, Some(e.span));
            return Err(());
        }
    };

    match Compiler::compile_file(program, std::path::Path::new(path)) {
        Ok((compiled, warnings)) => Ok(Compiled {
            program: compiled,
            warnings,
            ctx,
        }),
        Err(e) => {
            match &e {
                CompileError::Type(te) => ctx.error(&te.message, Some(te.span)),
                CompileError::Codegen(msg) => ctx.error(msg, None),
            }
            Err(())
        }
    }
}

// ── Commands ─────────────────────────────────────────────────────────

fn run_file(path: &str) {
    let compiled = match compile_source(path) {
        Ok(c) => c,
        Err(()) => process::exit(1),
    };

    for w in &compiled.warnings {
        compiled.ctx.warning(&w.message, Some(w.span));
    }

    let mut vm = VM::new(compiled.program);
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
            compiled.ctx.error(&e.message, vm.error_span());
            process::exit(1);
        }
    }
}

fn check_file(path: &str) {
    let compiled = match compile_source(path) {
        Ok(c) => c,
        Err(()) => process::exit(1),
    };

    for w in &compiled.warnings {
        compiled.ctx.warning(&w.message, Some(w.span));
    }
    println!("OK");
}

fn build_vm(policy_path: &str) {
    let toml = match fs::read_to_string(policy_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Cannot read policy file '{policy_path}': {e}");
            process::exit(1);
        }
    };

    let policy = match hiko_vm::policy::Policy::from_toml(&toml) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Invalid policy: {e}");
            process::exit(1);
        }
    };

    let rust_src = policy.to_rust_source();

    let stem = std::path::Path::new(policy_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("custom")
        .replace('.', "-");
    let out_dir = format!("hiko-vm-{stem}");

    fs::create_dir_all(format!("{out_dir}/src")).expect("cannot create output directory");

    fs::write(format!("{out_dir}/src/main.rs"), &rust_src).expect("cannot write main.rs");

    let version = env!("CARGO_PKG_VERSION");
    let cargo_toml = format!(
        r#"[workspace]

[package]
name = "{out_dir}"
version = "0.1.0"
edition = "2024"

[dependencies]
hiko-syntax = "{version}"
hiko-compile = "{version}"
hiko-vm = "{version}"
"#
    );

    fs::write(format!("{out_dir}/Cargo.toml"), &cargo_toml).expect("cannot write Cargo.toml");

    println!("Generated custom VM project: {out_dir}/");
    println!("Policy: {policy:?}");
    println!();
    println!("To build:");
    println!("  cd {out_dir} && cargo build --release");
    println!();
    println!("To run a script:");
    println!("  ./{out_dir}/target/release/{out_dir} script.hml");
}
