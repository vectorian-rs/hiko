use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use codespan_reporting::diagnostic::{Diagnostic, Label, Severity};
use codespan_reporting::files::SimpleFiles;
use codespan_reporting::term;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};

use hiko_builtin_meta::{BuiltinSurface, builtin_meta, core_builtin_names};
use hiko_common::blake3_hex;
use hiko_compile::chunk::{Chunk, CompiledProgram, Constant};
use hiko_compile::compiler::{CompileError, Compiler};
use hiko_compile::op::Op;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::span::Span;
use hiko_vm::builder::VMBuilder;
use hiko_vm::config::RunConfig;
use hiko_vm::vm::StdoutOutputSink;
use serde::Deserialize;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: hiko <command> [args]");
        eprintln!("Commands:");
        eprintln!(
            "  run [--config <hiko.toml>] [--policy <name>] [--strict] [file.hml]  Compile and execute a program"
        );
        eprintln!(
            "  check [--config <hiko.toml>] [--policy <name>] [--strict] <file.hml>  Type-check without executing"
        );
        eprintln!("  hash <file>...  Print BLAKE3 hashes for files");
        eprintln!("  build-vm <config.toml>  Generate a custom VM from a run config");
        process::exit(1);
    }

    match args[1].as_str() {
        "run" => {
            let options = parse_run_args(&args[2..]);
            run_file(&options);
        }
        "check" => {
            let options = parse_check_args(&args[2..]);
            check_file(&options);
        }
        "hash" => {
            if args.len() < 3 {
                eprintln!("Usage: hiko hash <file>...");
                process::exit(1);
            }
            hash_files(&args[2..]);
        }
        "build-vm" => {
            if args.len() < 3 {
                eprintln!("Usage: hiko build-vm <config.toml>");
                process::exit(1);
            }
            build_vm(&args[2]);
        }
        other => {
            eprintln!("Unknown command: {other}");
            eprintln!(
                "Try: hiko run [--config <hiko.toml>] [--policy <name>] [--strict] [file.hml]"
            );
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
    program: CompiledProgram,
    warnings: Vec<hiko_types::infer::Warning>,
    ctx: DiagCtx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptOptions {
    config_path: Option<String>,
    policy_name: Option<String>,
    script_path: Option<String>,
    strict: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ManifestDefaults {
    policy: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPolicy {
    path: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ProjectManifest {
    #[serde(default)]
    #[serde(rename = "project")]
    _project: toml::Table,
    #[serde(default)]
    defaults: ManifestDefaults,
    #[serde(default)]
    policies: HashMap<String, ManifestPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StrictViolation {
    builtin: String,
    capability_path: Option<&'static str>,
    span: Option<Span>,
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

fn load_policy_config(policy_path: &str) -> RunConfig {
    load_run_config(policy_path, "policy")
}

fn load_run_config(config_path: &str, label: &str) -> RunConfig {
    let toml = fs::read_to_string(config_path).unwrap_or_else(|e| {
        eprintln!("Cannot read {label} file '{config_path}': {e}");
        process::exit(1);
    });

    RunConfig::from_toml(&toml).unwrap_or_else(|e| {
        eprintln!("Invalid {label} '{config_path}': {e}");
        process::exit(1);
    })
}

fn hash_files(paths: &[String]) {
    for path in paths {
        let bytes = fs::read(path).unwrap_or_else(|e| {
            eprintln!("Cannot read file '{path}': {e}");
            process::exit(1);
        });
        println!("blake3:{}  {}", blake3_hex(&bytes), path);
    }
}

fn find_project_manifest_from(start_dir: &Path) -> Option<PathBuf> {
    let mut current = Some(start_dir);
    while let Some(dir) = current {
        let candidate = dir.join("hiko.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

fn load_project_manifest(path: &Path) -> Result<ProjectManifest, String> {
    let toml = fs::read_to_string(path)
        .map_err(|e| format!("Cannot read project manifest '{}': {e}", path.display()))?;
    toml::from_str(&toml).map_err(|e| format!("Invalid project manifest '{}': {e}", path.display()))
}

fn resolve_policy_config_path_from(
    manifest_path: &Path,
    policy_name: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    let manifest = load_project_manifest(manifest_path)?;
    let selected = if let Some(policy_name) = policy_name {
        policy_name.to_string()
    } else {
        match manifest.defaults.policy.clone() {
            Some(policy) => policy,
            None => {
                return Err(format!(
                    "Project manifest '{}' does not define defaults.policy; pass --policy <name>",
                    manifest_path.display()
                ));
            }
        }
    };

    let policy = manifest.policies.get(&selected).ok_or_else(|| {
        format!(
            "Project manifest '{}' does not define policy '{}'",
            manifest_path.display(),
            selected
        )
    })?;

    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    Ok(Some(root.join(&policy.path)))
}

fn resolve_policy_path(options: &ScriptOptions) -> Option<String> {
    let cwd = env::current_dir().unwrap_or_else(|e| {
        eprintln!("Cannot determine current working directory: {e}");
        process::exit(1);
    });

    let manifest_path = if let Some(config_path) = &options.config_path {
        PathBuf::from(config_path)
    } else if let Some(path) = find_project_manifest_from(&cwd) {
        path
    } else {
        if let Some(policy_name) = &options.policy_name {
            eprintln!("No hiko.toml found while resolving policy '{policy_name}'");
            process::exit(1);
        }
        return None;
    };

    match resolve_policy_config_path_from(&manifest_path, options.policy_name.as_deref()) {
        Ok(Some(path)) => Some(path.to_string_lossy().into_owned()),
        Ok(None) => None,
        Err(err) => {
            eprintln!("{err}");
            process::exit(1);
        }
    }
}

fn parse_script_args(args: &[String], usage: &str, require_script: bool) -> ScriptOptions {
    let mut config_path = None;
    let mut policy_name = None;
    let mut script_path = None;
    let mut strict = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                let Some(path) = args.get(i + 1) else {
                    eprintln!("{usage}");
                    process::exit(1);
                };
                config_path = Some(path.clone());
                i += 2;
            }
            arg if arg.starts_with("--config=") => {
                let path = arg.trim_start_matches("--config=");
                if path.is_empty() {
                    eprintln!("{usage}");
                    process::exit(1);
                }
                config_path = Some(path.to_string());
                i += 1;
            }
            "--policy" => {
                let Some(name) = args.get(i + 1) else {
                    eprintln!("{usage}");
                    process::exit(1);
                };
                policy_name = Some(name.clone());
                i += 2;
            }
            arg if arg.starts_with("--policy=") => {
                let name = arg.trim_start_matches("--policy=");
                if name.is_empty() {
                    eprintln!("{usage}");
                    process::exit(1);
                }
                policy_name = Some(name.to_string());
                i += 1;
            }
            "--strict" => {
                strict = true;
                i += 1;
            }
            other if other.starts_with('-') => {
                eprintln!("Unknown option: {other}");
                eprintln!("{usage}");
                process::exit(1);
            }
            path => {
                if script_path.is_some() {
                    eprintln!("Unexpected extra argument: {path}");
                    eprintln!("{usage}");
                    process::exit(1);
                }
                script_path = Some(path.to_string());
                i += 1;
            }
        }
    }

    if require_script && script_path.is_none() {
        eprintln!("{usage}");
        process::exit(1);
    }

    ScriptOptions {
        config_path,
        policy_name,
        script_path,
        strict,
    }
}

fn parse_run_args(args: &[String]) -> ScriptOptions {
    parse_script_args(
        args,
        "Usage: hiko run [--config <hiko.toml>] [--policy <name>] [--strict] [file.hml]",
        false,
    )
}

fn parse_check_args(args: &[String]) -> ScriptOptions {
    parse_script_args(
        args,
        "Usage: hiko check [--config <hiko.toml>] [--policy <name>] [--strict] <file.hml>",
        true,
    )
}

fn resolve_run_target(config: Option<&RunConfig>, script_path: Option<&str>) -> String {
    if let Some(path) = script_path {
        return path.to_string();
    }
    if let Some(config) = config
        && let Some(entry) = &config.entry
    {
        return entry.clone();
    }
    eprintln!("Usage: hiko run [--config <hiko.toml>] [--policy <name>] [--strict] [file.hml]");
    process::exit(1);
}

fn read_u8(code: &[u8], ip: &mut usize) -> Result<u8, String> {
    let byte = *code
        .get(*ip)
        .ok_or_else(|| "unexpected end of bytecode".to_string())?;
    *ip += 1;
    Ok(byte)
}

fn read_u16(code: &[u8], ip: &mut usize) -> Result<u16, String> {
    let bytes = code
        .get(*ip..*ip + 2)
        .ok_or_else(|| "unexpected end of bytecode".to_string())?;
    *ip += 2;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn skip_bytes(code: &[u8], ip: &mut usize, n: usize) -> Result<(), String> {
    code.get(*ip..*ip + n)
        .ok_or_else(|| "unexpected end of bytecode".to_string())?;
    *ip += n;
    Ok(())
}

fn read_const_string(chunk: &Chunk, idx: usize) -> Result<&str, String> {
    match chunk.constants.get(idx) {
        Some(Constant::String(s)) => Ok(s),
        Some(_) => Err(format!("expected string constant at index {idx}")),
        None => Err(format!("constant index out of bounds: {idx}")),
    }
}

fn scan_chunk_globals(
    chunk: &Chunk,
    defs: &mut HashSet<String>,
    reads: &mut Vec<(String, Option<Span>)>,
) -> Result<(), String> {
    let mut ip = 0usize;
    let code = &chunk.code;

    while ip < code.len() {
        let offset = ip;
        let op = Op::try_from(read_u8(code, &mut ip)?)
            .map_err(|byte| format!("invalid opcode while scanning bytecode: {byte}"))?;
        match op {
            Op::Const
            | Op::GetLocal
            | Op::SetLocal
            | Op::GetUpvalue
            | Op::Jump
            | Op::JumpIfFalse
            | Op::CallDirect
            | Op::TailCallDirect
            | Op::Panic
            | Op::Perform => {
                skip_bytes(code, &mut ip, 2)?;
            }
            Op::GetGlobal => {
                let idx = read_u16(code, &mut ip)? as usize;
                let name = read_const_string(chunk, idx)?.to_string();
                reads.push((name, chunk.span_at(offset)));
            }
            Op::SetGlobal => {
                let idx = read_u16(code, &mut ip)? as usize;
                let name = read_const_string(chunk, idx)?.to_string();
                defs.insert(name);
            }
            Op::GetField | Op::Call | Op::TailCall | Op::MakeTuple => {
                skip_bytes(code, &mut ip, 1)?;
            }
            Op::MakeData => {
                skip_bytes(code, &mut ip, 3)?;
            }
            Op::MakeClosure => {
                let _proto_idx = read_u16(code, &mut ip)?;
                let n_captures = read_u8(code, &mut ip)? as usize;
                skip_bytes(code, &mut ip, n_captures * 3)?;
            }
            Op::InstallHandler => {
                let n_clauses = read_u16(code, &mut ip)? as usize;
                skip_bytes(code, &mut ip, n_clauses * 4)?;
            }
            Op::Unit
            | Op::True
            | Op::False
            | Op::Pop
            | Op::AddInt
            | Op::SubInt
            | Op::MulInt
            | Op::DivInt
            | Op::ModInt
            | Op::Neg
            | Op::AddFloat
            | Op::SubFloat
            | Op::MulFloat
            | Op::DivFloat
            | Op::NegFloat
            | Op::Eq
            | Op::Ne
            | Op::LtInt
            | Op::GtInt
            | Op::LeInt
            | Op::GeInt
            | Op::LtFloat
            | Op::GtFloat
            | Op::LeFloat
            | Op::GeFloat
            | Op::ConcatString
            | Op::Not
            | Op::GetTag
            | Op::Return
            | Op::Halt
            | Op::RemoveHandler
            | Op::Resume => {}
        }
    }

    Ok(())
}

fn strict_violations(
    program: &CompiledProgram,
    allowed_builtins: &BTreeSet<&'static str>,
) -> Result<Vec<StrictViolation>, String> {
    let mut defs = HashSet::new();
    let mut reads = Vec::new();

    scan_chunk_globals(&program.main, &mut defs, &mut reads)?;
    for proto in &*program.functions {
        scan_chunk_globals(&proto.chunk, &mut defs, &mut reads)?;
    }

    let mut violations = Vec::new();
    let mut seen = BTreeSet::new();

    for (name, span) in reads {
        if defs.contains(&name) {
            continue;
        }
        let Some(meta) = builtin_meta(&name) else {
            continue;
        };
        if allowed_builtins.contains(name.as_str()) {
            continue;
        }
        if seen.insert(name.clone()) {
            violations.push(StrictViolation {
                builtin: name.clone(),
                capability_path: meta.capability_path,
                span,
            });
        }
    }

    Ok(violations)
}

fn validate_strict_surface(
    compiled: &Compiled,
    config: Option<&RunConfig>,
) -> Result<(), Vec<StrictViolation>> {
    let allowed: BTreeSet<&'static str> = if let Some(config) = config {
        config.enabled_builtin_names()
    } else {
        core_builtin_names().collect()
    };

    match strict_violations(&compiled.program, &allowed) {
        Ok(violations) if violations.is_empty() => Ok(()),
        Ok(violations) => Err(violations),
        Err(message) => Err(vec![StrictViolation {
            builtin: message,
            capability_path: None,
            span: None,
        }]),
    }
}

fn strict_message(violation: &StrictViolation, has_config: bool) -> String {
    if let Some(path) = violation.capability_path {
        if has_config {
            format!(
                "builtin '{}' is not enabled by this run config (enable [{}])",
                violation.builtin, path
            )
        } else {
            format!(
                "builtin '{}' is not available in the default core-only run surface (supply --config and enable [{}])",
                violation.builtin, path
            )
        }
    } else if builtin_meta(&violation.builtin)
        .is_some_and(|meta| meta.surface == BuiltinSurface::RuntimeOnly)
    {
        format!(
            "builtin '{}' is runtime-only and not part of the public run-config surface",
            violation.builtin
        )
    } else {
        violation.builtin.clone()
    }
}

// ── Commands ─────────────────────────────────────────────────────────

fn run_file(options: &ScriptOptions) {
    let policy_path = resolve_policy_path(options);
    let config = policy_path.as_deref().map(load_policy_config);
    let path = resolve_run_target(config.as_ref(), options.script_path.as_deref());
    let compiled = match compile_source(&path) {
        Ok(c) => c,
        Err(()) => process::exit(1),
    };

    for w in &compiled.warnings {
        compiled.ctx.warning(&w.message, Some(w.span));
    }

    if options.strict
        && let Err(violations) = validate_strict_surface(&compiled, config.as_ref())
    {
        for violation in &violations {
            compiled
                .ctx
                .error(&strict_message(violation, config.is_some()), violation.span);
        }
        process::exit(1);
    }

    let mut vm = if let Some(config) = &config {
        config.build_vm(compiled.program)
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

fn check_file(options: &ScriptOptions) {
    let path = options
        .script_path
        .as_deref()
        .expect("check requires a script");
    let compiled = match compile_source(path) {
        Ok(c) => c,
        Err(()) => process::exit(1),
    };

    for w in &compiled.warnings {
        compiled.ctx.warning(&w.message, Some(w.span));
    }

    let policy_path = resolve_policy_path(options);
    let config = policy_path.as_deref().map(load_policy_config);
    if options.strict
        && let Err(violations) = validate_strict_surface(&compiled, config.as_ref())
    {
        for violation in &violations {
            compiled
                .ctx
                .error(&strict_message(violation, config.is_some()), violation.span);
        }
        process::exit(1);
    }

    println!("OK");
}

fn build_vm(config_path: &str) {
    let config = load_run_config(config_path, "run config");

    let rust_src = config.to_rust_source();

    let stem = std::path::Path::new(config_path)
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
    println!("Config: {config:?}");
    println!();
    println!("To build:");
    println!("  cd {out_dir} && cargo build --release");
    println!();
    println!("To run a script:");
    if config.entry.is_some() {
        println!("  ./{out_dir}/target/release/{out_dir}");
        println!("  ./{out_dir}/target/release/{out_dir} other-script.hml");
    } else {
        println!("  ./{out_dir}/target/release/{out_dir} script.hml");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Compiled, DiagCtx, ScriptOptions, parse_check_args, parse_run_args,
        resolve_policy_config_path_from, resolve_run_target, strict_violations,
        validate_strict_surface,
    };
    use hiko_builtin_meta::core_builtin_names;
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;
    use hiko_vm::config::RunConfig;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn compile_program(source: &str) -> hiko_compile::chunk::CompiledProgram {
        let tokens = Lexer::new(source, 0).tokenize().expect("lex");
        let program = Parser::new(tokens).parse_program().expect("parse");
        Compiler::compile(program).expect("compile").0
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "hiko-cli-{}-{}-{}",
            name,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_run_args_file_only() {
        let args = vec!["script.hml".to_string()];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: None,
                policy_name: None,
                script_path: Some("script.hml".to_string()),
                strict: false,
            }
        );
    }

    #[test]
    fn parse_run_args_config_and_file() {
        let args = vec![
            "--config".to_string(),
            "hiko.toml".to_string(),
            "tools/read.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: None,
                script_path: Some("tools/read.hml".to_string()),
                strict: false,
            }
        );
    }

    #[test]
    fn parse_run_args_inline_config() {
        let args = vec![
            "--config=hiko.toml".to_string(),
            "tools/read.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: None,
                script_path: Some("tools/read.hml".to_string()),
                strict: false,
            }
        );
    }

    #[test]
    fn parse_run_args_with_strict() {
        let args = vec![
            "--strict".to_string(),
            "--config=hiko.toml".to_string(),
            "tools/read.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: None,
                script_path: Some("tools/read.hml".to_string()),
                strict: true,
            }
        );
    }

    #[test]
    fn parse_check_args_with_config_and_strict() {
        let args = vec![
            "--config".to_string(),
            "hiko.toml".to_string(),
            "--strict".to_string(),
            "tools/read.hml".to_string(),
        ];
        assert_eq!(
            parse_check_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: None,
                script_path: Some("tools/read.hml".to_string()),
                strict: true,
            }
        );
    }

    #[test]
    fn parse_run_args_with_config_and_policy() {
        let args = vec![
            "--config".to_string(),
            "hiko.toml".to_string(),
            "--policy".to_string(),
            "docs-writer".to_string(),
            "tools/read.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: Some("docs-writer".to_string()),
                script_path: Some("tools/read.hml".to_string()),
                strict: false,
            }
        );
    }

    #[test]
    fn parse_run_args_with_policy() {
        let args = vec![
            "--policy".to_string(),
            "docs-writer".to_string(),
            "tools/read.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: None,
                policy_name: Some("docs-writer".to_string()),
                script_path: Some("tools/read.hml".to_string()),
                strict: false,
            }
        );
    }

    #[test]
    fn resolve_run_target_prefers_cli_script() {
        let config = RunConfig {
            entry: Some("scripts/default.hml".to_string()),
            ..RunConfig::default()
        };
        assert_eq!(
            resolve_run_target(Some(&config), Some("scripts/override.hml")),
            "scripts/override.hml"
        );
    }

    #[test]
    fn resolve_run_target_uses_config_entry() {
        let config = RunConfig {
            entry: Some("scripts/default.hml".to_string()),
            ..RunConfig::default()
        };
        assert_eq!(
            resolve_run_target(Some(&config), None),
            "scripts/default.hml"
        );
    }

    #[test]
    fn strict_violations_reports_disabled_builtin() {
        let program = compile_program(r#"val _ = exec ("echo", [])"#);
        let allowed: BTreeSet<&'static str> = core_builtin_names().collect();
        let violations = strict_violations(&program, &allowed).expect("scan should succeed");
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].builtin, "exec");
        assert_eq!(
            violations[0].capability_path,
            Some("capabilities.exec.exec")
        );
    }

    #[test]
    fn strict_violations_ignores_shadowed_builtin_name() {
        let program = compile_program(
            r#"
val exec = fn x => x
val _ = exec ("echo", [])
"#,
        );
        let allowed: BTreeSet<&'static str> = core_builtin_names().collect();
        let violations = strict_violations(&program, &allowed).expect("scan should succeed");
        assert!(violations.is_empty());
    }

    #[test]
    fn strict_validation_uses_run_config_surface() {
        let program = compile_program(r#"val _ = println "ok""#);
        let compiled = Compiled {
            program,
            warnings: Vec::new(),
            ctx: DiagCtx::new("<test>", String::new()),
        };
        let config = RunConfig::from_toml(
            r#"
[capabilities.stdio.println]
enabled = true
"#,
        )
        .expect("config should parse");
        assert!(validate_strict_surface(&compiled, Some(&config)).is_ok());
    }

    #[test]
    fn resolve_policy_path_from_manifest_default_policy() {
        let root = temp_dir("manifest-default-policy");
        fs::create_dir_all(root.join("policies")).unwrap();
        let manifest = root.join("hiko.toml");
        fs::write(
            &manifest,
            r#"
[project]
name = "demo"

[defaults]
policy = "software-developer-role"

[policies.software-developer-role]
path = "policies/user.toml"
"#,
        )
        .unwrap();
        fs::write(root.join("policies/user.toml"), "").unwrap();

        let resolved = resolve_policy_config_path_from(&manifest, None)
            .expect("manifest resolution should succeed")
            .expect("should resolve default policy");
        assert_eq!(resolved, root.join("policies/user.toml"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_policy_path_from_manifest_named_policy() {
        let root = temp_dir("manifest-named-policy");
        fs::create_dir_all(root.join("policies")).unwrap();
        let manifest = root.join("hiko.toml");
        fs::write(
            &manifest,
            r#"
[project]
name = "demo"

[defaults]
policy = "software-developer-role"

[policies.software-developer-role]
path = "policies/user.toml"

[policies.docs-writer]
path = "policies/agent.toml"
"#,
        )
        .unwrap();
        fs::write(root.join("policies/user.toml"), "").unwrap();
        fs::write(root.join("policies/agent.toml"), "").unwrap();

        let resolved = resolve_policy_config_path_from(&manifest, Some("docs-writer"))
            .expect("manifest resolution should succeed")
            .expect("should resolve named policy");
        assert_eq!(resolved, root.join("policies/agent.toml"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_policy_path_from_manifest_without_default_errors() {
        let root = temp_dir("manifest-no-default");
        let manifest = root.join("hiko.toml");
        fs::write(
            &manifest,
            r#"
[project]
name = "demo"
"#,
        )
        .unwrap();

        let err = resolve_policy_config_path_from(&manifest, None)
            .expect_err("manifest without default should require explicit --policy");
        assert!(err.contains("defaults.policy"));

        let _ = fs::remove_dir_all(root);
    }
}
