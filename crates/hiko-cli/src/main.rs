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
#[cfg(feature = "cli-hash")]
use hiko_common::blake3_hex;
use hiko_compile::chunk::{Chunk, CompiledProgram};
use hiko_compile::compiler::{
    CompileError, Compiler, ExtensionSurface, ModuleLoaderOptions, bounded_lockfile_path,
    bounded_project_path, toml_error_text,
};
use hiko_compile::op::{HANDLER_CLAUSE_BYTES, Op};
use hiko_syntax::format::{FormatError, format_source};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::span::Span;
use hiko_vm::config::RunConfig;
use hiko_vm::extension::Extension;
use hiko_vm::threaded::ThreadedRuntime;
use hiko_vm::vm::{StderrOutputSink, StdoutOutputSink, VM};
use serde::Deserialize;

mod build_vm;
pub(crate) use hiko_compile::compiler::find_project_manifest_from;
#[cfg(feature = "cli-lock")]
mod install;
#[cfg(feature = "cli-lock")]
mod lock_verify;

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
        eprintln!("  fmt [--check] [--recurse] <file.hml|dir>...  Format Hiko source files");
        #[cfg(feature = "cli-lock")]
        eprintln!(
            "  install [--config <hiko.toml>] [--check]  Lock and download the dependencies in hiko.toml"
        );
        #[cfg(feature = "cli-lock")]
        eprintln!("  lock verify [--config <hiko.toml>]  Verify locked remote modules");
        eprintln!("  inspect-work <file.hml>  Print static bytecode opcode counts");
        #[cfg(feature = "cli-hash")]
        eprintln!("  hash <file>...  Print BLAKE3 hashes for files");
        eprintln!(
            "  build-vm <config.toml> [--lock <hiko.lock.toml>] [--script <entry.hml>]  Generate a custom VM from a run config"
        );
        process::exit(1);
    }

    match args[1].as_str() {
        "run" => {
            let options = parse_run_args(&args[2..]);
            #[cfg(feature = "heap-metrics")]
            let metrics = hiko_vm::heap_metrics::Collection::from_env().unwrap_or_else(|error| {
                eprintln!("{error}");
                process::exit(1);
            });
            run_file(&options);
            // run_file's VM/runtime (including retained threaded root and joined
            // workers) must be dropped before validating heap completeness.
            #[cfg(feature = "heap-metrics")]
            if let Some(metrics) = metrics {
                metrics.finish().unwrap_or_else(|error| {
                    eprintln!("{error}");
                    process::exit(1);
                });
            }
        }
        "check" => {
            let options = parse_check_args(&args[2..]);
            check_file(&options);
        }
        "fmt" => {
            let options = parse_fmt_args(&args[2..]);
            fmt_files(&options);
        }
        "inspect-work" => {
            let path = parse_inspect_work_args(&args[2..]);
            inspect_work_file(&path);
        }
        "install" => {
            #[cfg(feature = "cli-lock")]
            install::install_command(&args[2..]);
            #[cfg(not(feature = "cli-lock"))]
            {
                eprintln!("This build of hiko was compiled without the 'install' command");
                process::exit(1);
            }
        }
        "lock" => {
            #[cfg(feature = "cli-lock")]
            lock_verify::lock_command(&args[2..]);
            #[cfg(not(feature = "cli-lock"))]
            {
                eprintln!("This build of hiko was compiled without the 'lock' command");
                process::exit(1);
            }
        }
        "hash" => {
            #[cfg(feature = "cli-hash")]
            {
                if args.len() < 3 {
                    eprintln!("Usage: hiko hash <file>...");
                    process::exit(1);
                }
                hash_files(&args[2..]);
            }
            #[cfg(not(feature = "cli-hash"))]
            {
                eprintln!("This build of hiko was compiled without the 'hash' command");
                process::exit(1);
            }
        }
        "build-vm" => {
            let options = build_vm::parse_args(&args[2..]).unwrap_or_else(|e| {
                eprintln!("{e}");
                process::exit(1);
            });
            build_vm(&options);
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
    ctx: DiagCtx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptOptions {
    config_path: Option<String>,
    policy_name: Option<String>,
    script_path: Option<String>,
    strict: bool,
    /// Worker threads for the threaded runtime.
    workers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FmtOptions {
    check: bool,
    recurse: bool,
    paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ManifestDefaults {
    policy: Option<String>,
    lockfile: Option<String>,
    /// Read by the compiler's module loader.
    #[serde(rename = "lib_cache")]
    _lib_cache: Option<String>,
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
    #[serde(rename = "registries")]
    _registries: toml::Table,
    #[serde(default)]
    #[serde(rename = "dependencies")]
    _dependencies: toml::Table,
    #[serde(default)]
    policies: HashMap<String, ManifestPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StrictViolation {
    builtin: String,
    capability_path: Option<&'static str>,
    span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ManifestSource {
    ExplicitConfig,
    AutoDiscovered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedPolicy {
    manifest_path: PathBuf,
    manifest_source: ManifestSource,
    policy_name: String,
    policy_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeSurface {
    CheckOnly,
    Policy(ResolvedPolicy),
}

impl ResolvedPolicy {
    fn source_label(&self) -> &'static str {
        match self.manifest_source {
            ManifestSource::ExplicitConfig => "--config",
            ManifestSource::AutoDiscovered => "auto-discovered hiko.toml",
        }
    }

    fn describe(&self) -> String {
        format!(
            "using policy '{}' from '{}' (manifest: '{}', source: {})",
            self.policy_name,
            self.policy_path.display(),
            self.manifest_path.display(),
            self.source_label()
        )
    }

    fn error_context(&self) -> String {
        format!(
            "policy '{}' from '{}'",
            self.policy_name,
            self.policy_path.display()
        )
    }
}

impl RuntimeSurface {
    fn describe(&self) -> String {
        match self {
            Self::CheckOnly => "checking core builtins without a runtime policy".to_string(),
            Self::Policy(policy) => policy.describe(),
        }
    }
}

pub(crate) fn read_source_or_exit(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {path}: {e}");
        process::exit(1);
    })
}

/// Applies the lockfile bound to the project of the script at `path`, so a
/// script with no import meets it too. Reads no lockfile.
pub(crate) fn check_script_project(path: &Path) -> Result<(), String> {
    let script = fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    let Some(manifest_path) = script.parent().and_then(find_project_manifest_from) else {
        return Ok(());
    };
    let manifest = load_project_manifest(&manifest_path)?;
    bounded_lockfile_path(&manifest_path, manifest.defaults.lockfile.as_deref()).map(|_| ())
}

/// Compiles `path`, printing any diagnostics. Warnings are printed and
/// compilation continues; errors are printed and the process exits with 1.
/// Extensions linked into this binary, by the `ext-*` cargo features. A run
/// config turns one on with an `[extensions.<name>]` table.
fn linked_extensions() -> Vec<&'static Extension> {
    [
        #[cfg(feature = "ext-github")]
        &hiko_ext_github::GITHUB,
        #[cfg(feature = "ext-aws")]
        &hiko_ext_aws::AWS,
    ]
    .into_iter()
    .collect()
}

/// What the extensions `config` turns on add to the compiler.
fn extension_surface_or_exit(config: Option<&RunConfig>) -> ExtensionSurface {
    let Some(config) = config else {
        return ExtensionSurface::default();
    };
    match config.configured_extensions(&linked_extensions()) {
        Ok(configured) => Extension::compile_surface(&configured),
        Err(message) => {
            eprintln!("error: {message}");
            process::exit(1);
        }
    }
}

fn build_vm_or_exit(config: &RunConfig, program: CompiledProgram) -> hiko_vm::vm::VM {
    config
        .build_vm_with_extensions(program, &linked_extensions())
        .unwrap_or_else(|message| {
            eprintln!("error: {message}");
            process::exit(1);
        })
}

fn compile_or_exit(path: &str, extensions: ExtensionSurface) -> Compiled {
    let source = read_source_or_exit(path);
    let ctx = DiagCtx::new(path, source.clone());
    check_script_project(Path::new(path)).unwrap_or_else(|message| {
        ctx.error(&message, None);
        process::exit(1);
    });

    let tokens = Lexer::new(&source, 0).tokenize().unwrap_or_else(|e| {
        ctx.error(&e.message, Some(e.span));
        process::exit(1);
    });
    let program = Parser::new(tokens).parse_program().unwrap_or_else(|e| {
        ctx.error(&e.message, Some(e.span));
        process::exit(1);
    });
    let options = ModuleLoaderOptions {
        extensions,
        ..Default::default()
    };
    let (program, warnings) =
        Compiler::compile_file_with_options(program, Path::new(path), options).unwrap_or_else(
            |e| {
                match &e {
                    CompileError::Type(te) => ctx.error(&te.message, Some(te.span)),
                    CompileError::Codegen(msg) => ctx.error(msg, None),
                }
                process::exit(1);
            },
        );

    for warning in &warnings {
        ctx.warning(&warning.message, Some(warning.span));
    }
    Compiled { program, ctx }
}

fn load_policy_config(policy: &ResolvedPolicy) -> RunConfig {
    load_run_config(
        &policy.policy_path.to_string_lossy(),
        &format!("policy '{}'", policy.policy_name),
    )
}

/// Loads the run config, or `None` for checking without a policy.
fn load_surface_config(surface: &RuntimeSurface) -> Option<RunConfig> {
    match surface {
        RuntimeSurface::CheckOnly => None,
        RuntimeSurface::Policy(policy) => Some(load_policy_config(policy)),
    }
}

fn load_run_config(config_path: &str, label: &str) -> RunConfig {
    let toml = read_source_or_exit(config_path);

    RunConfig::from_toml(&toml).unwrap_or_else(|e| {
        eprintln!("Invalid {label} '{config_path}': {e}");
        process::exit(1);
    })
}

#[cfg(feature = "cli-hash")]
fn hash_files(paths: &[String]) {
    for path in paths {
        let bytes = fs::read(path).unwrap_or_else(|e| {
            eprintln!("Cannot read file '{path}': {e}");
            process::exit(1);
        });
        println!("blake3:{}  {}", blake3_hex(&bytes), path);
    }
}

fn load_project_manifest(path: &Path) -> Result<ProjectManifest, String> {
    let toml = fs::read_to_string(path)
        .map_err(|e| format!("Cannot read project manifest '{}': {e}", path.display()))?;
    toml::from_str(&toml).map_err(|e| {
        format!(
            "Invalid project manifest '{}': {}",
            path.display(),
            toml_error_text(&e, &toml)
        )
    })
}

fn resolve_policy_from_manifest(
    manifest_path: &Path,
    manifest_source: ManifestSource,
    policy_name: Option<&str>,
) -> Result<ResolvedPolicy, String> {
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

    let policy_path = bounded_project_path(manifest_path, "policy path", &policy.path)?;
    Ok(ResolvedPolicy {
        manifest_path: manifest_path.to_path_buf(),
        manifest_source,
        policy_name: selected,
        policy_path,
    })
}

fn resolve_runtime_surface(options: &ScriptOptions) -> RuntimeSurface {
    let cwd = env::current_dir().unwrap_or_else(|e| {
        eprintln!("Cannot determine current working directory: {e}");
        process::exit(1);
    });

    let (manifest_path, manifest_source) = if let Some(config_path) = &options.config_path {
        (PathBuf::from(config_path), ManifestSource::ExplicitConfig)
    } else if let Some(path) = auto_project_manifest(options.script_path.as_deref(), &cwd)
        .unwrap_or_else(|message| {
            eprintln!("{message}");
            process::exit(1);
        })
    {
        (path, ManifestSource::AutoDiscovered)
    } else {
        if let Some(policy_name) = &options.policy_name {
            eprintln!("No hiko.toml found while resolving policy '{policy_name}'");
            process::exit(1);
        }
        return RuntimeSurface::CheckOnly;
    };

    match resolve_policy_from_manifest(
        &manifest_path,
        manifest_source,
        options.policy_name.as_deref(),
    ) {
        Ok(policy) => RuntimeSurface::Policy(policy),
        Err(err) => {
            eprintln!("{err}");
            process::exit(1);
        }
    }
}

/// Auto-selection derives authority from the real script, never an alias or
/// an unrelated working directory. Explicit --config bypasses this selection.
fn auto_project_manifest(script: Option<&str>, cwd: &Path) -> Result<Option<PathBuf>, String> {
    let cwd = fs::canonicalize(cwd)
        .map_err(|e| format!("Cannot resolve working directory '{}': {e}", cwd.display()))?;
    let Some(script) = script else {
        return Ok(find_project_manifest_from(&cwd));
    };
    let script =
        fs::canonicalize(script).map_err(|e| format!("Cannot read script '{script}': {e}"))?;
    let manifest = script.parent().and_then(find_project_manifest_from);
    if let Some(manifest) = &manifest
        && !cwd.starts_with(manifest.parent().expect("manifest has a directory"))
    {
        return Err(format!(
            "Working directory '{}' is outside script project '{}'; select policy explicitly with --config '{}' [--policy <name>]",
            cwd.display(),
            manifest.display(),
            manifest.display()
        ));
    }
    Ok(manifest)
}

/// Parses `<flag> <value>` or `<flag>=<value>` starting at `args[index]`.
///
/// Returns `None` when `args[index]` is not `flag`. Otherwise returns the value
/// and the number of arguments consumed. A missing or empty value prints
/// `usage` and exits.
fn parse_flag_value<'a>(
    args: &'a [String],
    flag: &str,
    index: usize,
    usage: &str,
) -> Option<(&'a str, usize)> {
    let arg = &args[index];
    let (value, consumed) = if arg == flag {
        (args.get(index + 1).map(String::as_str), 2)
    } else {
        let value = arg
            .strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))?;
        (Some(value), 1)
    };
    match value {
        Some(value) if !value.is_empty() => Some((value, consumed)),
        _ => {
            eprintln!("{usage}");
            process::exit(1);
        }
    }
}

fn parse_script_args(args: &[String], usage: &str, require_script: bool) -> ScriptOptions {
    let mut config_path = None;
    let mut policy_name = None;
    let mut script_path = None;
    let mut strict = false;
    let mut workers = 1;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some((value, consumed)) = parse_flag_value(args, "--config", i, usage) {
            config_path = Some(value.to_string());
            i += consumed;
        } else if let Some((value, consumed)) = parse_flag_value(args, "--policy", i, usage) {
            policy_name = Some(value.to_string());
            i += consumed;
        } else if let Some((value, consumed)) = parse_flag_value(args, "--workers", i, usage) {
            workers = match value.parse::<usize>() {
                Ok(n) if n > 0 => n,
                _ => {
                    eprintln!("--workers expects a positive integer, got '{value}'");
                    eprintln!("{usage}");
                    process::exit(1);
                }
            };
            i += consumed;
        } else if arg == "--strict" {
            strict = true;
            i += 1;
        } else if arg.starts_with('-') {
            eprintln!("Unknown option: {arg}");
            eprintln!("{usage}");
            process::exit(1);
        } else {
            if script_path.is_some() {
                eprintln!("Unexpected extra argument: {arg}");
                eprintln!("{usage}");
                process::exit(1);
            }
            script_path = Some(arg.clone());
            i += 1;
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
        workers,
    }
}

fn parse_run_args(args: &[String]) -> ScriptOptions {
    parse_script_args(
        args,
        "Usage: hiko run [--config <hiko.toml>] [--policy <name>] [--strict] [--workers <n>] [file.hml]",
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

fn parse_fmt_args(args: &[String]) -> FmtOptions {
    let usage = "Usage: hiko fmt [--check] [--recurse] <file.hml|dir>...";
    let mut check = false;
    let mut recurse = false;
    let mut paths = Vec::new();

    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--recurse" => recurse = true,
            _ if arg.starts_with('-') => {
                eprintln!("Unknown option: {arg}");
                eprintln!("{usage}");
                process::exit(1);
            }
            _ => paths.push(arg.clone()),
        }
    }

    if paths.is_empty() {
        eprintln!("{usage}");
        process::exit(1);
    }

    FmtOptions {
        check,
        recurse,
        paths,
    }
}

fn parse_inspect_work_args(args: &[String]) -> String {
    let usage = "Usage: hiko inspect-work <file.hml>";
    if args.len() != 1 || args[0].starts_with('-') {
        eprintln!("{usage}");
        process::exit(1);
    }
    args[0].clone()
}

fn resolve_run_target(config: &RunConfig, script_path: Option<&str>) -> String {
    if let Some(path) = script_path {
        return path.to_string();
    }
    if let Some(entry) = &config.entry {
        return entry.clone();
    }
    eprintln!(
        "Usage: hiko run [--config <hiko.toml>] [--policy <name>] [--strict] [--workers <n>] [file.hml]"
    );
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

fn global_name(globals: &[Arc<str>], slot: usize) -> Result<String, String> {
    globals
        .get(slot)
        .map(|name| name.to_string())
        .ok_or_else(|| format!("global slot out of bounds: {slot}"))
}

fn advance_operands(code: &[u8], ip: &mut usize, op: Op) -> Result<(), String> {
    match op {
        Op::Const
        | Op::GetLocal
        | Op::GetLocalField0
        | Op::GetLocalField1
        | Op::SetLocal
        | Op::GetUpvalue
        | Op::GetGlobal
        | Op::SetGlobal
        | Op::Jump
        | Op::JumpIfFalse
        | Op::JumpIfNotEq
        | Op::JumpIfEq
        | Op::JumpIfNotLtInt
        | Op::JumpIfNotGtInt
        | Op::JumpIfNotLeInt
        | Op::JumpIfNotGeInt
        | Op::CallDirect
        | Op::TailCallDirect
        | Op::Panic
        | Op::Perform => {
            skip_bytes(code, ip, 2)?;
        }
        Op::GetField | Op::Call | Op::TailCall | Op::MakeTuple => {
            skip_bytes(code, ip, 1)?;
        }
        Op::MakeData => {
            skip_bytes(code, ip, 3)?;
        }
        Op::JumpIfNotTag => {
            skip_bytes(code, ip, 4)?;
        }
        Op::JumpIfLocalNotTag => {
            skip_bytes(code, ip, 6)?;
        }
        Op::MakeClosure => {
            let _proto_idx = read_u16(code, ip)?;
            let n_captures = read_u8(code, ip)? as usize;
            skip_bytes(code, ip, n_captures * 3)?;
        }
        Op::MakeRecClosures => {
            let n_closures = read_u8(code, ip)? as usize;
            for _ in 0..n_closures {
                let _proto_idx = read_u16(code, ip)?;
                let n_captures = read_u8(code, ip)? as usize;
                skip_bytes(code, ip, n_captures * 3)?;
            }
        }
        Op::InstallHandler => {
            let n_clauses = read_u16(code, ip)? as usize;
            skip_bytes(code, ip, n_clauses * HANDLER_CLAUSE_BYTES)?;
        }
        Op::Unit
        | Op::CallSelf
        | Op::TailCallSelf
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
        | Op::AddWord
        | Op::SubWord
        | Op::MulWord
        | Op::DivWord
        | Op::ModWord
        | Op::LtWord
        | Op::GtWord
        | Op::LeWord
        | Op::GeWord
        | Op::ConcatString
        | Op::Not
        | Op::GetLocal0
        | Op::GetLocal1
        | Op::GetLocal2
        | Op::GetLocal3
        | Op::SetLocal0
        | Op::SetLocal1
        | Op::SetLocal2
        | Op::SetLocal3
        | Op::GetField0
        | Op::GetField1
        | Op::MakeCons
        | Op::Return
        | Op::Halt
        | Op::RemoveHandler => {}
    }
    Ok(())
}

fn scan_chunk_globals(
    chunk: &Chunk,
    globals: &[Arc<str>],
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
            Op::GetGlobal => {
                let slot = read_u16(code, &mut ip)? as usize;
                reads.push((global_name(globals, slot)?, chunk.span_at(offset)));
            }
            Op::SetGlobal => {
                let slot = read_u16(code, &mut ip)? as usize;
                defs.insert(global_name(globals, slot)?);
            }
            _ => advance_operands(code, &mut ip, op)?,
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

    scan_chunk_globals(&program.main, &program.globals, &mut defs, &mut reads)?;
    for proto in &*program.functions {
        scan_chunk_globals(&proto.chunk, &program.globals, &mut defs, &mut reads)?;
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
    program: &CompiledProgram,
    config: Option<&RunConfig>,
) -> Result<(), Vec<StrictViolation>> {
    let allowed: BTreeSet<&'static str> = if let Some(config) = config {
        config.enabled_builtin_names()
    } else {
        core_builtin_names().collect()
    };

    match strict_violations(program, &allowed) {
        Ok(violations) if violations.is_empty() => Ok(()),
        Ok(violations) => Err(violations),
        Err(message) => Err(vec![StrictViolation {
            builtin: message,
            capability_path: None,
            span: None,
        }]),
    }
}

/// Under `--strict`, reports builtins the active surface does not enable and
/// exits with 1 if there are any.
fn enforce_strict_or_exit(
    compiled: &Compiled,
    config: Option<&RunConfig>,
    surface: &RuntimeSurface,
) {
    if let Err(violations) = validate_strict_surface(&compiled.program, config) {
        for violation in &violations {
            compiled
                .ctx
                .error(&strict_message(violation, surface), violation.span);
        }
        process::exit(1);
    }
}

fn strict_message(violation: &StrictViolation, surface: &RuntimeSurface) -> String {
    if let Some(path) = violation.capability_path {
        match surface {
            RuntimeSurface::CheckOnly => format!(
                "builtin '{}' is not enabled for checking without a policy (pass --config and enable [{}])",
                violation.builtin, path
            ),
            RuntimeSurface::Policy(policy) => format!(
                "builtin '{}' is not enabled by the active policy (layer: RunConfig::build_vm; {}; enable [{}])",
                violation.builtin,
                policy.error_context(),
                path
            ),
        }
    } else if builtin_meta(&violation.builtin)
        .is_some_and(|meta| meta.surface == BuiltinSurface::RuntimeOnly)
    {
        match surface {
            RuntimeSurface::CheckOnly => format!(
                "builtin '{}' is runtime-only and not part of the public checking surface (layer: internal builtin alias)",
                violation.builtin
            ),
            RuntimeSurface::Policy(policy) => format!(
                "builtin '{}' is runtime-only and not part of the public run-config surface (layer: internal builtin alias; {})",
                violation.builtin,
                policy.error_context()
            ),
        }
    } else {
        violation.builtin.clone()
    }
}

fn runtime_error_message(message: &str, policy: &ResolvedPolicy) -> String {
    let Some(name) = message.strip_prefix("undefined global: ") else {
        return message.to_string();
    };
    let Some(meta) = builtin_meta(name) else {
        return message.to_string();
    };
    if let Some(path) = meta.capability_path {
        return format!(
            "builtin '{}' was not registered by the active policy-filtered VM (layer: RunConfig::build_vm; {}; enable [{}])",
            name,
            policy.error_context(),
            path
        );
    }
    if meta.surface == BuiltinSurface::RuntimeOnly {
        return format!(
            "runtime builtin '{}' was not registered by the active policy-filtered VM (layer: internal builtin alias / RunConfig::build_vm; {}). This usually means the supporting public capability family is missing from the active policy.",
            name,
            policy.error_context()
        );
    }
    message.to_string()
}

// ── Commands ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionWorkStat {
    name: Option<String>,
    arity: u8,
    static_opcodes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkInspection {
    main_static_opcodes: usize,
    functions: Vec<FunctionWorkStat>,
}

impl WorkInspection {
    fn total_static_opcodes(&self) -> usize {
        self.main_static_opcodes
            + self
                .functions
                .iter()
                .map(|function| function.static_opcodes)
                .sum::<usize>()
    }
}

fn count_chunk_opcodes(chunk: &Chunk) -> Result<usize, String> {
    let mut ip = 0usize;
    let code = &chunk.code;
    let mut count = 0usize;

    while ip < code.len() {
        count += 1;
        let op = Op::try_from(read_u8(code, &mut ip)?)
            .map_err(|byte| format!("invalid opcode while counting bytecode: {byte}"))?;
        advance_operands(code, &mut ip, op)?;
    }

    Ok(count)
}

fn inspect_program_work(program: &CompiledProgram) -> Result<WorkInspection, String> {
    let main_static_opcodes = count_chunk_opcodes(&program.main)?;
    let mut functions = Vec::with_capacity(program.functions.len());

    for proto in &*program.functions {
        functions.push(FunctionWorkStat {
            name: proto.name.clone(),
            arity: proto.arity,
            static_opcodes: count_chunk_opcodes(&proto.chunk)?,
        });
    }

    Ok(WorkInspection {
        main_static_opcodes,
        functions,
    })
}

fn inspect_work_file(path: &str) {
    let compiled = compile_or_exit(path, ExtensionSurface::default());
    let inspection = inspect_program_work(&compiled.program).unwrap_or_else(|message| {
        eprintln!("error: {message}");
        process::exit(1);
    });

    println!("file: {path}");
    println!("main_static_opcodes: {}", inspection.main_static_opcodes);
    println!("function_count: {}", inspection.functions.len());
    for (idx, function) in inspection.functions.iter().enumerate() {
        let name = function.name.as_deref().unwrap_or("<lambda>");
        println!(
            "function[{idx}]: name={name} arity={} static_opcodes={}",
            function.arity, function.static_opcodes
        );
    }
    println!(
        "total_static_opcodes: {}",
        inspection.total_static_opcodes()
    );
    println!(
        "note: max_work currently counts executed opcodes, so loops, recursion, and repeated calls can consume more work than these static totals."
    );
}

fn run_file(options: &ScriptOptions) {
    let surface = resolve_runtime_surface(options);
    let RuntimeSurface::Policy(policy) = &surface else {
        eprintln!(
            "run requires a runtime policy; register a named policy in hiko.toml and use: hiko run --config /path/to/hiko.toml --policy core script.hml"
        );
        process::exit(1);
    };
    eprintln!("info: {}", surface.describe());
    let config = load_policy_config(policy);
    let path = resolve_run_target(&config, options.script_path.as_deref());
    if options.config_path.is_none() && options.script_path.is_none() {
        let cwd = env::current_dir().unwrap_or_else(|e| {
            eprintln!("Cannot determine current working directory: {e}");
            process::exit(1);
        });
        let target = auto_project_manifest(Some(&path), &cwd).unwrap_or_else(|message| {
            eprintln!("{message}");
            process::exit(1);
        });
        if target.as_ref() != Some(&policy.manifest_path) {
            eprintln!(
                "Configured entry belongs to a different project; select policy explicitly with --config '{}'",
                policy.manifest_path.display()
            );
            process::exit(1);
        }
    }
    let compiled = compile_or_exit(&path, extension_surface_or_exit(Some(&config)));
    if options.strict {
        enforce_strict_or_exit(&compiled, Some(&config), &surface);
    }

    let output_sink = Arc::new(StdoutOutputSink::default());
    if config.requires_runtime() {
        let mut vm = build_vm_or_exit(&config, compiled.program);
        report_exec_notes(&vm);
        vm.set_output_sink(output_sink);
        vm.set_error_sink(Arc::new(StderrOutputSink));

        let runtime = ThreadedRuntime::new(options.workers);
        let pid = runtime.spawn_root_vm(vm);
        if let Err(message) = runtime.run_to_completion() {
            compiled.ctx.error(&message, None);
            process::exit(1);
        }

        if let Some(failure) = runtime.failure(pid) {
            let message = runtime_error_message(&failure.to_string(), policy);
            compiled.ctx.error(&message, None);
            process::exit(1);
        }
        return;
    }

    let mut vm = build_vm_or_exit(&config, compiled.program);
    report_exec_notes(&vm);
    vm.set_output_sink(output_sink);
    vm.set_error_sink(Arc::new(StderrOutputSink));
    if let Err(e) = vm.run() {
        let message = runtime_error_message(&e.message, policy);
        compiled.ctx.error(&message, vm.error_span());
        process::exit(1);
    }
}

/// Allowed executables with weaker protection than this platform gives.
fn report_exec_notes(vm: &VM) {
    for note in vm.exec_policy_notes() {
        eprintln!("info: {note}");
    }
}

fn check_file(options: &ScriptOptions) {
    let surface = resolve_runtime_surface(options);
    eprintln!("info: {}", surface.describe());
    let path = options
        .script_path
        .as_deref()
        .expect("check requires a script");
    let config = load_surface_config(&surface);
    let compiled = compile_or_exit(path, extension_surface_or_exit(config.as_ref()));
    if options.strict {
        enforce_strict_or_exit(&compiled, config.as_ref(), &surface);
    }

    println!("OK");
}

fn fmt_files(options: &FmtOptions) {
    let mut had_error = false;
    let mut needs_formatting = false;

    let paths = resolve_fmt_paths(options).unwrap_or_else(|message| {
        eprintln!("error: {message}");
        process::exit(1);
    });

    for path in &paths {
        let display_path = path.display().to_string();
        let source = fs::read_to_string(path).unwrap_or_else(|error| {
            eprintln!("error: cannot read {display_path}: {error}");
            process::exit(1);
        });
        let ctx = DiagCtx::new(&display_path, source.clone());
        let formatted = match format_source(&source, 0) {
            Ok(formatted) => formatted,
            Err(error) => {
                let (message, span) = match error {
                    FormatError::Lex(error) => (error.message, Some(error.span)),
                    FormatError::Parse(error) => (error.message, Some(error.span)),
                    FormatError::UnsupportedComment { message, span } => (message, Some(span)),
                };
                ctx.error(&message, span);
                had_error = true;
                continue;
            }
        };

        if formatted == source {
            continue;
        }

        if options.check {
            println!("{display_path}");
            needs_formatting = true;
            continue;
        }

        fs::write(path, formatted).unwrap_or_else(|error| {
            eprintln!("error: cannot write {display_path}: {error}");
            process::exit(1);
        });
        println!("formatted {display_path}");
    }

    if had_error || (options.check && needs_formatting) {
        process::exit(1);
    }
}

fn resolve_fmt_paths(options: &FmtOptions) -> Result<Vec<PathBuf>, String> {
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::new();

    for raw_path in &options.paths {
        collect_fmt_path(
            &PathBuf::from(raw_path),
            options.recurse,
            &mut seen,
            &mut resolved,
        )?;
    }

    Ok(resolved)
}

fn collect_fmt_path(
    path: &Path,
    recurse: bool,
    seen: &mut BTreeSet<PathBuf>,
    resolved: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("cannot access {}: {error}", path.display()))?;

    if metadata.is_file() {
        push_unique_path(path, seen, resolved);
        return Ok(());
    }

    if !metadata.is_dir() {
        return Err(format!(
            "{} is not a regular file or directory",
            path.display()
        ));
    }

    if !recurse {
        return Err(format!(
            "{} is a directory; pass --recurse to format directories",
            path.display()
        ));
    }

    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("cannot read directory {}: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot read directory {}: {error}", path.display()))?;
    entries.sort_by_key(|entry| entry.path());

    // `DirEntry::metadata` does not follow symlinks, so symlinked files and
    // directories below an explicit argument are skipped rather than followed.
    for entry in entries {
        let entry_path = entry.path();
        let entry_metadata = entry
            .metadata()
            .map_err(|error| format!("cannot access {}: {error}", entry_path.display()))?;
        if entry_metadata.is_dir() {
            collect_fmt_path(&entry_path, true, seen, resolved)?;
        } else if entry_metadata.is_file() && entry_path.extension().is_some_and(|ext| ext == "hml")
        {
            push_unique_path(&entry_path, seen, resolved);
        }
    }

    Ok(())
}

fn push_unique_path(path: &Path, seen: &mut BTreeSet<PathBuf>, resolved: &mut Vec<PathBuf>) {
    if seen.insert(path.to_path_buf()) {
        resolved.push(path.to_path_buf());
    }
}

fn build_vm(options: &build_vm::BuildVmOptions) {
    let config = load_run_config(&options.config_path, "run config");
    let out_dir = build_vm::generate(options, &config, &linked_extensions()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    });
    let name = build_vm::package_name(&out_dir);
    let binary = if out_dir.is_absolute() {
        format!("{}/target/release/{name}", out_dir.display())
    } else {
        format!("./{}/target/release/{name}", out_dir.display())
    };
    let out_dir = out_dir.display();

    println!("Generated custom VM project: {out_dir}/");
    println!("Config: {config:?}");
    if options.lock.is_some() || options.script.is_some() {
        println!("Sealed: imports resolve only from the sources in {out_dir}/src/embedded.rs");
    }
    println!();
    println!("To build:");
    println!("  cd {out_dir} && cargo build --release");
    println!();
    if options.script.is_some() {
        println!("To run its program:");
        println!("  {binary}");
    } else if config.entry.is_some() {
        println!("To run a script:");
        println!("  {binary}");
        println!("  {binary} other-script.hml");
    } else {
        println!("To run a script:");
        println!("  {binary} script.hml");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FmtOptions, ManifestSource, ResolvedPolicy, RuntimeSurface, ScriptOptions,
        inspect_program_work, parse_check_args, parse_fmt_args, parse_inspect_work_args,
        parse_run_args, resolve_policy_from_manifest, resolve_run_target, runtime_error_message,
        strict_message, strict_violations, validate_strict_surface,
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
                workers: 1,
            }
        );
    }

    #[test]
    fn parse_run_args_config_and_file() {
        let args = vec![
            "--config".to_string(),
            "hiko.toml".to_string(),
            "script.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: None,
                script_path: Some("script.hml".to_string()),
                strict: false,
                workers: 1,
            }
        );
    }

    #[test]
    fn parse_run_args_inline_config() {
        let args = vec!["--config=hiko.toml".to_string(), "script.hml".to_string()];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: None,
                script_path: Some("script.hml".to_string()),
                strict: false,
                workers: 1,
            }
        );
    }

    #[test]
    fn parse_run_args_with_strict() {
        let args = vec![
            "--strict".to_string(),
            "--config=hiko.toml".to_string(),
            "script.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: None,
                script_path: Some("script.hml".to_string()),
                strict: true,
                workers: 1,
            }
        );
    }

    #[test]
    fn parse_check_args_with_config_and_strict() {
        let args = vec![
            "--config".to_string(),
            "hiko.toml".to_string(),
            "--strict".to_string(),
            "script.hml".to_string(),
        ];
        assert_eq!(
            parse_check_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: None,
                script_path: Some("script.hml".to_string()),
                strict: true,
                workers: 1,
            }
        );
    }

    #[test]
    fn parse_fmt_args_file_list() {
        let args = vec!["examples/hello.hml".to_string(), "script.hml".to_string()];
        assert_eq!(
            parse_fmt_args(&args),
            FmtOptions {
                check: false,
                recurse: false,
                paths: vec!["examples/hello.hml".to_string(), "script.hml".to_string()],
            }
        );
    }

    #[test]
    fn parse_fmt_args_with_check() {
        let args = vec![
            "--check".to_string(),
            "examples/hello.hml".to_string(),
            "script.hml".to_string(),
        ];
        assert_eq!(
            parse_fmt_args(&args),
            FmtOptions {
                check: true,
                recurse: false,
                paths: vec!["examples/hello.hml".to_string(), "script.hml".to_string()],
            }
        );
    }

    #[test]
    fn parse_fmt_args_with_recurse() {
        let args = vec!["--recurse".to_string(), "examples".to_string()];
        assert_eq!(
            parse_fmt_args(&args),
            FmtOptions {
                check: false,
                recurse: true,
                paths: vec!["examples".to_string()],
            }
        );
    }

    #[test]
    fn resolve_fmt_paths_recurse_collects_hml_files() {
        let root = temp_dir("fmt-recurse");
        let subdir = root.join("sub");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(root.join("one.hml"), "val x=1\n").unwrap();
        fs::write(root.join("skip.txt"), "hello\n").unwrap();
        fs::write(subdir.join("two.hml"), "val y=2\n").unwrap();

        let paths = super::resolve_fmt_paths(&FmtOptions {
            check: false,
            recurse: true,
            paths: vec![root.to_string_lossy().into_owned()],
        })
        .expect("directory resolution should succeed");

        let resolved: Vec<String> = paths
            .into_iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(resolved, vec!["one.hml", "sub/two.hml"]);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_fmt_paths_recurse_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let outside = temp_dir("fmt-symlink-outside");
        fs::write(outside.join("outside.hml"), "val z=3\n").unwrap();
        let root = temp_dir("fmt-symlink-root");
        fs::write(root.join("inside.hml"), "val x=1\n").unwrap();
        symlink(outside.join("outside.hml"), root.join("linked.hml")).unwrap();
        symlink(&outside, root.join("linked-dir")).unwrap();
        symlink(&root, root.join("loop")).unwrap();

        let paths = super::resolve_fmt_paths(&FmtOptions {
            check: false,
            recurse: true,
            paths: vec![root.to_string_lossy().into_owned()],
        })
        .expect("directory resolution should succeed");

        assert_eq!(paths, vec![root.join("inside.hml")]);

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn resolve_fmt_paths_rejects_directory_without_recurse() {
        let root = temp_dir("fmt-no-recurse");
        let error = super::resolve_fmt_paths(&FmtOptions {
            check: false,
            recurse: false,
            paths: vec![root.to_string_lossy().into_owned()],
        })
        .expect_err("directory input should require --recurse");
        assert!(error.contains("pass --recurse"));
    }

    #[test]
    fn parse_run_args_with_config_and_policy() {
        let args = vec![
            "--config".to_string(),
            "hiko.toml".to_string(),
            "--policy".to_string(),
            "docs-writer".to_string(),
            "script.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: Some("hiko.toml".to_string()),
                policy_name: Some("docs-writer".to_string()),
                script_path: Some("script.hml".to_string()),
                strict: false,
                workers: 1,
            }
        );
    }

    #[test]
    fn parse_run_args_inline_policy() {
        let args = vec!["--policy=docs-writer".to_string(), "script.hml".to_string()];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: None,
                policy_name: Some("docs-writer".to_string()),
                script_path: Some("script.hml".to_string()),
                strict: false,
                workers: 1,
            }
        );
    }

    #[test]
    fn parse_flag_value_ignores_other_flags() {
        let args = vec!["--configuration".to_string(), "x".to_string()];
        assert_eq!(super::parse_flag_value(&args, "--config", 0, "usage"), None);
    }

    #[test]
    fn parse_run_args_with_policy() {
        let args = vec![
            "--policy".to_string(),
            "docs-writer".to_string(),
            "script.hml".to_string(),
        ];
        assert_eq!(
            parse_run_args(&args),
            ScriptOptions {
                config_path: None,
                policy_name: Some("docs-writer".to_string()),
                script_path: Some("script.hml".to_string()),
                strict: false,
                workers: 1,
            }
        );
    }

    #[test]
    fn parse_inspect_work_args_file_only() {
        let args = vec!["examples/work_budget_demo.hml".to_string()];
        assert_eq!(
            parse_inspect_work_args(&args),
            "examples/work_budget_demo.hml".to_string()
        );
    }

    #[test]
    fn resolve_run_target_prefers_cli_script() {
        let config = RunConfig {
            entry: Some("scripts/default.hml".to_string()),
            ..RunConfig::default()
        };
        assert_eq!(
            resolve_run_target(&config, Some("scripts/override.hml")),
            "scripts/override.hml"
        );
    }

    #[test]
    fn resolve_run_target_uses_config_entry() {
        let config = RunConfig {
            entry: Some("scripts/default.hml".to_string()),
            ..RunConfig::default()
        };
        assert_eq!(resolve_run_target(&config, None), "scripts/default.hml");
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
            Some("capabilities.exec.run")
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
        let config = RunConfig::from_toml(
            r#"
[capabilities.stdio.println]
enabled = true
"#,
        )
        .expect("config should parse");
        assert!(validate_strict_surface(&program, Some(&config)).is_ok());
    }

    #[test]
    fn inspect_program_work_reports_named_function_counts() {
        let program = compile_program(
            r#"
fun inc x = x + 1
val answer = inc 41
"#,
        );
        let inspection = inspect_program_work(&program).expect("inspect should succeed");
        assert!(inspection.main_static_opcodes > 0);
        assert_eq!(inspection.functions.len(), 1);
        assert_eq!(inspection.functions[0].name.as_deref(), Some("inc"));
        assert_eq!(
            inspection.total_static_opcodes(),
            inspection.main_static_opcodes + inspection.functions[0].static_opcodes
        );
    }

    #[test]
    fn manifest_accepts_lib_cache() {
        let manifest: super::ProjectManifest = toml::from_str(
            "[defaults]\npolicy = \"p\"\nlockfile = \"hiko.lock.toml\"\nlib_cache = \".cache\"\n",
        )
        .expect("lib_cache is a known [defaults] key");
        assert_eq!(manifest.defaults.policy.as_deref(), Some("p"));
    }

    #[test]
    fn resolve_policy_from_manifest_default_policy() {
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

        let resolved =
            resolve_policy_from_manifest(&manifest, ManifestSource::AutoDiscovered, None)
                .expect("manifest resolution should succeed");
        assert_eq!(resolved.policy_name, "software-developer-role");
        assert_eq!(resolved.policy_path, root.join("policies/user.toml"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_policy_from_manifest_named_policy() {
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

        let resolved = resolve_policy_from_manifest(
            &manifest,
            ManifestSource::AutoDiscovered,
            Some("docs-writer"),
        )
        .expect("manifest resolution should succeed");
        assert_eq!(resolved.policy_name, "docs-writer");
        assert_eq!(resolved.policy_path, root.join("policies/agent.toml"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_policy_from_manifest_without_default_errors() {
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

        let err = resolve_policy_from_manifest(&manifest, ManifestSource::AutoDiscovered, None)
            .expect_err("manifest without default should require explicit --policy");
        assert!(err.contains("defaults.policy"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn strict_message_includes_layer_and_policy_context() {
        let violation = super::StrictViolation {
            builtin: "exec".to_string(),
            capability_path: Some("capabilities.exec.run"),
            span: None,
        };
        let surface = RuntimeSurface::Policy(ResolvedPolicy {
            manifest_path: PathBuf::from("/tmp/hiko.toml"),
            manifest_source: ManifestSource::ExplicitConfig,
            policy_name: "dev".to_string(),
            policy_path: PathBuf::from("/tmp/policies/dev.toml"),
        });
        let message = strict_message(&violation, &surface);
        assert!(message.contains("RunConfig::build_vm"), "got: {message}");
        assert!(message.contains("policy 'dev'"), "got: {message}");
        assert!(message.contains("capabilities.exec.run"), "got: {message}");
    }

    #[test]
    fn runtime_error_message_names_the_policy() {
        let policy = ResolvedPolicy {
            manifest_path: PathBuf::from("/tmp/hiko.toml"),
            manifest_source: ManifestSource::ExplicitConfig,
            policy_name: "core".to_string(),
            policy_path: PathBuf::from("/tmp/policies/core.policy.toml"),
        };
        let message = runtime_error_message("undefined global: exec", &policy);
        assert!(message.contains("policy 'core'"), "got: {message}");
        assert!(message.contains("capabilities.exec.run"), "got: {message}");
    }
}
