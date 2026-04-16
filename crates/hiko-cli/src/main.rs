use std::env;
use std::fs;
use std::process;
use std::sync::Arc;

use codespan_reporting::diagnostic::{Diagnostic, Label, Severity};
use codespan_reporting::files::SimpleFiles;
use codespan_reporting::term;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};

use hiko_compile::compiler::{CompileError, Compiler};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::span::Span;
use hiko_vm::builder::VMBuilder;
use hiko_vm::policy::Policy;
use hiko_vm::vm::StdoutOutputSink;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: hiko <command> [args]");
        eprintln!("Commands:");
        eprintln!("  run [--policy <file.toml>] [file.hml]  Compile and execute a program");
        eprintln!("  check <file.hml>       Type-check without executing");
        eprintln!("  build-vm <policy.toml>  Generate a custom VM from a policy file");
        process::exit(1);
    }

    match args[1].as_str() {
        "run" => {
            let (policy_path, script_path) = parse_run_args(&args[2..]);
            run_file(policy_path.as_deref(), script_path.as_deref());
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
            eprintln!("Try: hiko run [--policy <file.toml>] [file.hml]");
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

fn load_policy(policy_path: &str) -> Policy {
    let toml = fs::read_to_string(policy_path).unwrap_or_else(|e| {
        eprintln!("Cannot read policy file '{policy_path}': {e}");
        process::exit(1);
    });

    Policy::from_toml(&toml).unwrap_or_else(|e| {
        eprintln!("Invalid policy '{policy_path}': {e}");
        process::exit(1);
    })
}

fn parse_run_args(args: &[String]) -> (Option<String>, Option<String>) {
    let mut policy_path = None;
    let mut script_path = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--policy" => {
                let Some(path) = args.get(i + 1) else {
                    eprintln!("Usage: hiko run [--policy <file.toml>] [file.hml]");
                    process::exit(1);
                };
                policy_path = Some(path.clone());
                i += 2;
            }
            arg if arg.starts_with("--policy=") => {
                let path = arg.trim_start_matches("--policy=");
                if path.is_empty() {
                    eprintln!("Usage: hiko run [--policy <file.toml>] [file.hml]");
                    process::exit(1);
                }
                policy_path = Some(path.to_string());
                i += 1;
            }
            other if other.starts_with('-') => {
                eprintln!("Unknown run option: {other}");
                eprintln!("Usage: hiko run [--policy <file.toml>] [file.hml]");
                process::exit(1);
            }
            path => {
                if script_path.is_some() {
                    eprintln!("Unexpected extra argument: {path}");
                    eprintln!("Usage: hiko run [--policy <file.toml>] [file.hml]");
                    process::exit(1);
                }
                script_path = Some(path.to_string());
                i += 1;
            }
        }
    }

    (policy_path, script_path)
}

fn resolve_run_target(policy: Option<&Policy>, script_path: Option<&str>) -> String {
    if let Some(path) = script_path {
        return path.to_string();
    }
    if let Some(policy) = policy
        && let Some(entry) = &policy.entry
    {
        return entry.clone();
    }
    eprintln!("Usage: hiko run [--policy <file.toml>] [file.hml]");
    process::exit(1);
}

// ── Commands ─────────────────────────────────────────────────────────

fn run_file(policy_path: Option<&str>, script_path: Option<&str>) {
    let policy = policy_path.map(load_policy);
    let path = resolve_run_target(policy.as_ref(), script_path);
    let compiled = match compile_source(&path) {
        Ok(c) => c,
        Err(()) => process::exit(1),
    };

    for w in &compiled.warnings {
        compiled.ctx.warning(&w.message, Some(w.span));
    }

    let mut vm = if let Some(policy) = &policy {
        policy.build_vm(compiled.program)
    } else {
        VMBuilder::new(compiled.program).with_core().build()
    };
    vm.set_output_sink(Arc::new(StdoutOutputSink::default()));
    match vm.run() {
        Ok(()) => {}
        Err(e) => {
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
    let policy = load_policy(policy_path);

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
    if policy.entry.is_some() {
        println!("  ./{out_dir}/target/release/{out_dir}");
        println!("  ./{out_dir}/target/release/{out_dir} other-script.hml");
    } else {
        println!("  ./{out_dir}/target/release/{out_dir} script.hml");
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_run_args, resolve_run_target};
    use hiko_vm::policy::Policy;

    #[test]
    fn parse_run_args_file_only() {
        let args = vec!["script.hml".to_string()];
        assert_eq!(
            parse_run_args(&args),
            (None, Some("script.hml".to_string()))
        );
    }

    #[test]
    fn parse_run_args_policy_and_file() {
        let args = vec![
            "--policy".to_string(),
            "policies/read.toml".to_string(),
            "tools/read.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            (
                Some("policies/read.toml".to_string()),
                Some("tools/read.hml".to_string())
            )
        );
    }

    #[test]
    fn parse_run_args_inline_policy() {
        let args = vec![
            "--policy=policies/read.toml".to_string(),
            "tools/read.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            (
                Some("policies/read.toml".to_string()),
                Some("tools/read.hml".to_string())
            )
        );
    }

    #[test]
    fn resolve_run_target_prefers_cli_script() {
        let policy = Policy {
            entry: Some("scripts/default.hml".to_string()),
            ..Policy::default()
        };
        assert_eq!(
            resolve_run_target(Some(&policy), Some("scripts/override.hml")),
            "scripts/override.hml"
        );
    }

    #[test]
    fn resolve_run_target_uses_policy_entry() {
        let policy = Policy {
            entry: Some("scripts/default.hml".to_string()),
            ..Policy::default()
        };
        assert_eq!(
            resolve_run_target(Some(&policy), None),
            "scripts/default.hml"
        );
    }
}
