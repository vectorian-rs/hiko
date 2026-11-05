//! `hiko build-vm`: generate a Rust crate whose binary bakes in one run
//! config and, when sealed (#59), the exact locked modules and program.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use hiko_compile::compiler::LockedModuleSource;
#[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
use hiko_compile::compiler::{
    CompileError, Compiler, EmbeddedFile, EmbeddedModule, EmbeddedSources, ModuleLoaderOptions,
    codegen_message, local_use_files, resolve_directory_of,
};
#[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
use hiko_syntax::{lexer::Lexer, parser::Parser};
use hiko_vm::config::{GeneratedProgram, RunConfig};
use hiko_vm::extension::Extension;

/// Extensions a generated crate can link: run-config name, crate, and the
/// Rust path of its `Extension`.
const KNOWN_EXTENSIONS: &[(&str, &str, &str)] = &[
    ("github", "hiko-ext-github", "hiko_ext_github::GITHUB"),
    ("aws", "hiko-ext-aws", "hiko_ext_aws::AWS"),
];

pub const USAGE: &str = "Usage: hiko build-vm <config.toml> [--lock <hiko.lock.toml>] [--script <entry.hml>] [--hiko-path <repo>] [--out <dir>]";

#[derive(Debug, Default, PartialEq)]
pub struct BuildVmOptions {
    pub config_path: String,
    /// Embed every module in this lockfile; imports resolve only from them.
    pub lock: Option<PathBuf>,
    /// Embed this program and the local files it uses; the binary runs only
    /// it.
    pub script: Option<PathBuf>,
    /// Depend on the hiko crates in this checkout instead of crates.io.
    pub hiko_path: Option<PathBuf>,
    pub out_dir: Option<PathBuf>,
    /// Where `--lock` reads modules from before the cache and the network
    /// (`ModuleLoaderOptions`); tests point these at the repository.
    pub local_package_root: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
}

pub fn parse_args(args: &[String]) -> Result<BuildVmOptions, String> {
    let mut options = BuildVmOptions::default();
    let mut config = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |flag: &str| {
            iter.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--lock" => options.lock = Some(value("--lock")?),
            "--script" => options.script = Some(value("--script")?),
            "--hiko-path" => options.hiko_path = Some(value("--hiko-path")?),
            "--out" => options.out_dir = Some(value("--out")?),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}\n{USAGE}")),
            path if config.is_none() => config = Some(path.to_string()),
            extra => return Err(format!("unexpected argument {extra}\n{USAGE}")),
        }
    }
    options.config_path = config.ok_or_else(|| USAGE.to_string())?;
    Ok(options)
}

/// What a sealed artifact embeds.
#[derive(Debug, Default)]
struct Sealed {
    modules: Vec<LockedModuleSource>,
    /// The entry script and its local files, with `--script`.
    script: Option<(String, Vec<(String, String)>)>,
}

/// Generate the crate. Returns its directory.
///
/// `linked` are the extensions this `hiko` binary links: a configured one is
/// checked with its `configure` now, and a `--script` program is compiled
/// with them.
pub fn generate(
    options: &BuildVmOptions,
    config: &RunConfig,
    linked: &[&'static Extension],
) -> Result<PathBuf, String> {
    let mut extensions = Vec::new();
    for (name, table) in &config.extensions {
        let known = KNOWN_EXTENSIONS
            .iter()
            .find(|(known, _, _)| known == name)
            .ok_or_else(|| {
                format!("build-vm cannot link extension '{name}'; it knows github and aws")
            })?;
        match linked.iter().find(|e| e.name == name) {
            Some(extension) => {
                (extension.configure)(table).map_err(|e| format!("[extensions.{name}]: {e}"))?;
            }
            // A --script program is compiled now, with the extension's types
            // and modules, so it must be linked here.
            None if options.script.is_some() => {
                return Err(format!(
                    "--script with [extensions.{name}] needs a hiko built with feature ext-{name}"
                ));
            }
            // Otherwise the generated binary checks the table when it starts
            // and reports a bad one as an error.
            None => {}
        }
        extensions.push(*known);
    }
    let configured: Vec<&'static Extension> = linked
        .iter()
        .filter(|e| config.extensions.contains_key(e.name))
        .copied()
        .collect();

    let sealed = if options.lock.is_some() || options.script.is_some() {
        Some(collect_sealed(options, &configured)?)
    } else {
        None
    };

    let out_dir = match &options.out_dir {
        Some(dir) => dir.clone(),
        None => {
            let stem = Path::new(&options.config_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("custom")
                .replace('.', "-");
            PathBuf::from(format!("hiko-vm-{stem}"))
        }
    };
    let name = package_name(&out_dir);
    if sealed.is_none() && SEALED_FILES.iter().any(|f| out_dir.join(f).exists()) {
        return Err(format!(
            "'{}' holds a sealed build (build.rs, src/embedded.rs, sealed/); generate an unsealed one into a new directory",
            out_dir.display()
        ));
    }

    let program = match &sealed {
        None => GeneratedProgram::ScriptArgument,
        Some(sealed) => GeneratedProgram::Sealed {
            script: sealed.script.is_some(),
        },
    };
    let paths: Vec<(&str, &str)> = extensions.iter().map(|(n, _, p)| (*n, *p)).collect();
    write(
        &out_dir.join("src/main.rs"),
        &config.to_rust_source_for(program, &paths)?,
    )?;
    if let Some(sealed) = &sealed {
        write_sealed(&out_dir, sealed)?;
    }
    write(
        &out_dir.join("Cargo.toml"),
        &cargo_toml(
            &name,
            options.hiko_path.as_deref(),
            sealed.as_ref(),
            &extensions,
        )?,
    )?;
    Ok(out_dir)
}

/// Files only a sealed build has. An unsealed build into the same directory
/// would leave them behind, and `build.rs` would then fail.
const SEALED_FILES: &[&str] = &["build.rs", "src/embedded.rs", "sealed"];

/// Names Cargo refuses for a package: Rust keywords and its own reserved
/// names.
const RESERVED_NAMES: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "abstract",
    "become",
    "box",
    "do",
    "final",
    "macro",
    "override",
    "priv",
    "typeof",
    "unsized",
    "virtual",
    "yield",
    "try",
    "gen",
    "build",
    "deps",
    "examples",
    "incremental",
    "test",
    "doc",
    "std",
    "core",
    "alloc",
    "proc_macro",
    "proc-macro",
];

/// A Cargo package name from the output directory: ASCII letters, digits,
/// `-` and `_`, starting with a letter.
pub fn package_name(out_dir: &Path) -> String {
    let base = std::fs::canonicalize(out_dir)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .or_else(|| {
            out_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let name: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_end_matches('-').to_string();
    if !name.starts_with(|c: char| c.is_ascii_alphabetic())
        || RESERVED_NAMES.contains(&name.as_str())
    {
        return format!("hiko-vm-{name}").trim_end_matches('-').to_string();
    }
    name
}

/// Sealing reads every locked module through the verified loader and
/// compiles the program against embedded sources.
#[cfg(not(all(feature = "loader-integrity-blake3", feature = "loader-embedded")))]
fn collect_sealed(
    _options: &BuildVmOptions,
    _extensions: &[&'static Extension],
) -> Result<Sealed, String> {
    Err("--lock and --script need a hiko built with cargo features loader-integrity-blake3 and loader-embedded".into())
}

#[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
fn collect_sealed(
    options: &BuildVmOptions,
    extensions: &[&'static Extension],
) -> Result<Sealed, String> {
    // Resolve --lock once; the check and the loader then see one project.
    let lock = match &options.lock {
        Some(lock) => {
            if lock.file_name().is_none() {
                return Err("--lock must name a lockfile, not a directory".to_string());
            }
            let unresolved = |e| format!("cannot resolve --lock '{}': {e}", lock.display());
            let entry = resolve_directory_of(lock).map_err(unresolved)?;
            let real = fs::canonicalize(&entry).map_err(unresolved)?;
            let used = Compiler::project_lockfile_path(&entry).map_err(codegen_message)?;
            if fs::canonicalize(&used).ok().as_ref() != Some(&real) {
                return Err(format!(
                    "--lock '{}' is not the lockfile its project uses ('{}'); named imports resolve through the project's hiko.toml",
                    real.display(),
                    used.display()
                ));
            }
            Some(entry)
        }
        None => None,
    };
    let modules = match &lock {
        Some(lock) => Compiler::locked_module_sources(
            lock,
            ModuleLoaderOptions {
                local_package_root: options.local_package_root.clone(),
                cache_dir: options.cache_dir.clone(),
                ..ModuleLoaderOptions::default()
            },
        )
        .map_err(|e| {
            format!(
                "cannot read the locked modules of '{}': {}",
                lock.display(),
                codegen_message(e)
            )
        })?,
        None => Vec::new(),
    };
    let script = match &options.script {
        Some(path) => {
            let source = fs::read_to_string(path)
                .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
            crate::check_script_project(path)?;
            let files = local_use_files(path, &source).map_err(|e| format!("{e:?}"))?;
            Some((source, files))
        }
        None => None,
    };
    let sealed = Sealed { modules, script };
    check_sealed_program(&sealed, extensions)?;
    Ok(sealed)
}

/// Compile the embedded program once, now, against exactly the embedded
/// sources, so a missing module or file fails `build-vm` instead of the
/// artifact.
#[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
fn check_sealed_program(sealed: &Sealed, extensions: &[&'static Extension]) -> Result<(), String> {
    let Some((source, files)) = &sealed.script else {
        return Ok(());
    };
    let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
    let modules: Vec<EmbeddedModule> = sealed
        .modules
        .iter()
        .map(|m| EmbeddedModule {
            import_name: leak(&m.import_name),
            blake3: leak(&m.blake3),
            source: leak(&m.source),
        })
        .collect();
    let files: Vec<EmbeddedFile> = files
        .iter()
        .map(|(path, source)| EmbeddedFile {
            path: leak(path),
            source: leak(source),
        })
        .collect();
    let embedded: &'static EmbeddedSources = Box::leak(Box::new(EmbeddedSources {
        modules: Box::leak(modules.into_boxed_slice()),
        files: Some(Box::leak(files.into_boxed_slice())),
    }));
    let tokens = Lexer::new(source, 0)
        .tokenize()
        .map_err(|e| format!("the script does not lex: {}", e.message))?;
    let program = Parser::new(tokens)
        .parse_program()
        .map_err(|e| format!("the script does not parse: {}", e.message))?;
    let options = ModuleLoaderOptions {
        embedded: Some(embedded),
        extensions: Extension::compile_surface(extensions),
        ..ModuleLoaderOptions::default()
    };
    Compiler::compile_embedded(program, options)
        .map(|_| ())
        .map_err(|e| {
            let message = match e {
                CompileError::Codegen(message) => message,
                CompileError::Type(error) => error.message,
            };
            format!("the script does not compile with only the embedded sources: {message}")
        })
}

/// `src/embedded.rs`, the sources under `sealed/`, and `build.rs`, which
/// refuses to build if a module no longer matches its lockfile hash.
fn write_sealed(out_dir: &Path, sealed: &Sealed) -> Result<(), String> {
    let mut rs = String::new();
    rs.push_str("//! Sources sealed into this artifact by `hiko build-vm` (generated).\n//!\n");
    rs.push_str("//! Named imports resolve only from these modules; each is checked\n");
    rs.push_str("//! against its lockfile hash by `build.rs` and again when imported.\n//!\n");
    if sealed.modules.is_empty() {
        rs.push_str("//! Modules: none.\n");
    } else {
        rs.push_str("//! | Module | Lockfile hash |\n//! |---|---|\n");
        for m in &sealed.modules {
            writeln!(rs, "//! | `{}` | `{}` |", m.import_name, m.blake3).unwrap();
        }
    }
    if let Some((_, files)) = &sealed.script {
        rs.push_str(
            "//!\n//! Program: `sealed/main.hml`, which the binary runs and nothing else.\n",
        );
        for (path, _) in files {
            writeln!(rs, "//! Local file: `{path}`").unwrap();
        }
    }
    rs.push_str(
        "\nuse hiko_compile::compiler::{EmbeddedFile, EmbeddedModule, EmbeddedSources};\n\n",
    );
    if let Some((source, _)) = &sealed.script {
        write(&out_dir.join("sealed/main.hml"), source)?;
        rs.push_str("pub static SCRIPT: &str = include_str!(\"../sealed/main.hml\");\n\n");
    }
    rs.push_str("pub static SOURCES: EmbeddedSources = EmbeddedSources {\n    modules: &[\n");
    // Modules are stored by index: import names that differ only in case
    // would collide on a case-insensitive file system.
    for (i, m) in sealed.modules.iter().enumerate() {
        let file = format!("sealed/modules/{i}.hml");
        write(&out_dir.join(&file), &m.source)?;
        writeln!(
            rs,
            "        EmbeddedModule {{ import_name: {:?}, blake3: {:?}, source: include_str!(\"../{file}\") }},",
            m.import_name, m.blake3
        )
        .unwrap();
    }
    rs.push_str("    ],\n");
    match &sealed.script {
        Some((_, files)) => {
            rs.push_str("    files: Some(&[\n");
            // Files are stored by index: a path may contain `..`.
            for (i, (path, source)) in files.iter().enumerate() {
                let file = format!("sealed/files/{i}.hml");
                write(&out_dir.join(&file), source)?;
                writeln!(
                    rs,
                    "        EmbeddedFile {{ path: {path:?}, source: include_str!(\"../{file}\") }},"
                )
                .unwrap();
            }
            rs.push_str("    ]),\n");
        }
        None => rs.push_str("    files: None,\n"),
    }
    rs.push_str("};\n");
    write(&out_dir.join("src/embedded.rs"), &rs)?;

    let mut build = String::new();
    build.push_str("// Generated by `hiko build-vm`: refuse to build if a sealed module no\n");
    build.push_str("// longer matches its lockfile hash.\nfn main() {\n    let modules: &[(&str, &str)] = &[\n");
    for (i, m) in sealed.modules.iter().enumerate() {
        writeln!(
            build,
            "        (\"sealed/modules/{i}.hml\", {:?}), // {}",
            m.digest, m.import_name
        )
        .unwrap();
    }
    build.push_str("    ];\n    for (path, expected) in modules {\n");
    build.push_str("        println!(\"cargo:rerun-if-changed={path}\");\n");
    build.push_str("        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!(\"cannot read {path}: {e}\"));\n");
    build.push_str("        let actual = blake3::hash(&bytes).to_hex();\n");
    build.push_str("        if actual.as_str() != *expected {\n");
    build.push_str("            panic!(\"{path} does not match its lockfile hash: expected {expected}, got {actual}\");\n");
    build.push_str("        }\n    }\n}\n");
    write(&out_dir.join("build.rs"), &build)
}

fn cargo_toml(
    name: &str,
    hiko_path: Option<&Path>,
    sealed: Option<&Sealed>,
    extensions: &[(&str, &str, &str)],
) -> Result<String, String> {
    let version = env!("CARGO_PKG_VERSION");
    let source_in = |dir: &str, krate: &str| -> Result<String, String> {
        match hiko_path {
            Some(repo) => {
                let dir = repo.join(dir).join(krate);
                let dir = fs::canonicalize(&dir)
                    .map_err(|e| format!("--hiko-path: cannot find '{}': {e}", dir.display()))?;
                Ok(format!("path = {:?}", dir.to_string_lossy()))
            }
            None => Ok(format!("version = \"{version}\"")),
        }
    };
    let source = |krate: &str| source_in("crates", krate);
    let mut toml = format!(
        "[workspace]\n\n[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nhiko-syntax = {{ {} }}\n",
        source("hiko-syntax")?
    );
    if sealed.is_some() {
        // No HTTP loader: a sealed artifact cannot fetch modules.
        writeln!(
            toml,
            "hiko-compile = {{ {}, default-features = false, features = [\"builtin-all\", \"loader-embedded\"] }}",
            source("hiko-compile")?
        )
        .unwrap();
    } else {
        writeln!(
            toml,
            "hiko-compile = {{ {}, features = [\"builtin-all\"] }}",
            source("hiko-compile")?
        )
        .unwrap();
    }
    writeln!(
        toml,
        "hiko-vm = {{ {}, features = [\"builtin-all\"] }}",
        source("hiko-vm")?
    )
    .unwrap();
    for (_, krate, _) in extensions {
        writeln!(toml, "{krate} = {{ {} }}", source_in("extensions", krate)?).unwrap();
    }
    if !extensions.is_empty() {
        // The same toml as hiko-vm, for the [extensions] tables.
        toml.push_str("toml = \"0.8\"\n");
    }
    if sealed.is_some() {
        toml.push_str("\n[build-dependencies]\nblake3 = \"1\"\n");
    }
    Ok(toml)
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create '{}': {e}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|e| format!("cannot write '{}': {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "hiko-build-vm-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    /// A script that imports a locked module and uses a local file.
    fn script_project(dir: &Path) -> PathBuf {
        write(
            &dir.join("main.hml"),
            "import Std.Option\nuse \"./lib/greet.hml\"\nval _ = println (greeting (Option.is_some (Option.Some 1)))\n",
        )
        .unwrap();
        write(
            &dir.join("lib/greet.hml"),
            "fun greeting (b : bool) = if b then \"sealed: some\" else \"sealed: none\"\n",
        )
        .unwrap();
        dir.join("main.hml")
    }

    fn config() -> RunConfig {
        RunConfig::from_toml("[capabilities.stdio.println]\nenabled = true\n").unwrap()
    }

    /// The file under `sealed/modules/` that holds `import_name`, from the
    /// generated `src/embedded.rs`.
    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    fn module_file(out: &Path, import_name: &str) -> PathBuf {
        let embedded = read(out.join("src/embedded.rs"));
        let line = embedded
            .lines()
            .find(|l| l.contains(&format!("import_name: \"{import_name}\"")))
            .unwrap_or_else(|| panic!("{import_name} is not embedded"));
        let start = line.find("../sealed/modules/").unwrap() + 3;
        let end = line[start..].find('"').unwrap() + start;
        out.join(&line[start..end])
    }

    fn read(path: PathBuf) -> String {
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn parse_args_reads_flags_in_any_order() {
        let args: Vec<String> = ["--script", "a.hml", "run.toml", "--lock", "hiko.lock.toml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let options = parse_args(&args).unwrap();
        assert_eq!(options.config_path, "run.toml");
        assert_eq!(options.script, Some(PathBuf::from("a.hml")));
        assert_eq!(options.lock, Some(PathBuf::from("hiko.lock.toml")));
        assert!(parse_args(&["--lock".to_string()]).is_err());
        assert!(parse_args(&["a".to_string(), "b".to_string()]).is_err());
    }

    /// Options that seal the modules of `lock`, read from the repository.
    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    fn sealing(lock: PathBuf, out_dir: PathBuf) -> BuildVmOptions {
        BuildVmOptions {
            config_path: "run.toml".into(),
            lock: Some(lock),
            script: None,
            hiko_path: Some(repo()),
            out_dir: Some(out_dir),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
        }
    }

    #[cfg(all(unix, feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    #[test]
    fn a_lock_linked_into_a_nested_project_seals_its_own_project() {
        let dir = temp_dir("lock-link");
        write(&dir.join("hiko.toml"), "").unwrap();
        write(
            &dir.join("nested/hiko.toml"),
            "[defaults]\nlockfile = \"other.lock\"\n",
        )
        .unwrap();
        write(&dir.join("nested/other.lock"), "schema_version = 1\n").unwrap();
        fs::copy(repo().join("hiko.lock.toml"), dir.join("nested/real.lock")).unwrap();
        std::os::unix::fs::symlink("nested/real.lock", dir.join("hiko.lock.toml")).unwrap();

        let options = sealing(dir.join("hiko.lock.toml"), dir.join("out"));
        let out = generate(&options, &config(), &[]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            read(module_file(&out, "Std.Option")),
            read(repo().join("libraries/Std-v0.1.0/modules/Option.hml"))
        );
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    #[test]
    fn a_lock_reached_through_dot_dot_belongs_to_the_directory_it_is_in() {
        let dir = temp_dir("lock-parent");
        write(
            &dir.join("hiko.toml"),
            "[defaults]\nlockfile = \"sub/l.toml\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("sub/deep")).unwrap();
        fs::copy(repo().join("hiko.lock.toml"), dir.join("sub/l.toml")).unwrap();
        write(&dir.join("sub/deep/hiko.toml"), "").unwrap();
        write(&dir.join("sub/deep/hiko.lock.toml"), "schema_version = 1\n").unwrap();

        let options = sealing(dir.join("sub/deep/../l.toml"), dir.join("out"));
        let out = generate(&options, &config(), &[]).unwrap_or_else(|e| panic!("{e}"));
        assert!(module_file(&out, "Std.Option").is_file());
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    #[test]
    fn sealed_script_artifact_embeds_modules_files_and_hashes() {
        let dir = temp_dir("sealed");
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            lock: Some(repo().join("hiko.lock.toml")),
            script: Some(script_project(&dir)),
            hiko_path: Some(repo()),
            out_dir: Some(dir.join("hiko-vm-sealed")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
        };
        let out = generate(&options, &config(), &[]).unwrap_or_else(|e| panic!("{e}"));

        let main = read(out.join("src/main.rs"));
        assert!(main.contains("mod embedded;"));
        assert!(main.contains("takes no arguments"));
        assert!(main.contains("Compiler::compile_embedded(program, options)"));
        assert!(
            !main.contains("read_to_string"),
            "the program is not read from disk"
        );

        let embedded = read(out.join("src/embedded.rs"));
        let option_hash = "blake3:bfef3fd8c6aa36f674c79e39ce0948dfbfe7f15cf69adeb8231c51b79ee56a84";
        assert!(embedded.contains(&format!("| `Std.Option` | `{option_hash}` |")));
        assert!(embedded.contains("path: \"lib/greet.hml\""));
        assert_eq!(
            read(module_file(&out, "Std.Option")),
            read(repo().join("libraries/Std-v0.1.0/modules/Option.hml"))
        );

        let build = read(out.join("build.rs"));
        let file = module_file(&out, "Std.Option");
        let file = file
            .strip_prefix(&out)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            build.contains(&format!("(\"{file}\", \"bfef3fd8")),
            "{build}"
        );

        let cargo = read(out.join("Cargo.toml"));
        assert!(cargo.contains(
            "default-features = false, features = [\"builtin-all\", \"loader-embedded\"]"
        ));
        assert!(cargo.contains("path = "));
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    #[test]
    fn sealed_script_must_compile_with_only_the_embedded_sources() {
        let dir = temp_dir("missing-module");
        // No --lock: Std.Option is not embedded, so build-vm refuses.
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            script: Some(script_project(&dir)),
            out_dir: Some(dir.join("out")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
            ..BuildVmOptions::default()
        };
        let err = generate(&options, &config(), &[]).expect_err("Std.Option is not embedded");
        assert!(err.contains("not embedded in this artifact"), "{err}");
        assert!(!dir.join("out").exists(), "nothing is written on failure");
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    #[test]
    fn lock_only_artifact_reads_scripts_but_embeds_modules() {
        let dir = temp_dir("lock-only");
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            lock: Some(repo().join("hiko.lock.toml")),
            out_dir: Some(dir.join("out")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
            ..BuildVmOptions::default()
        };
        let out = generate(&options, &config(), &[]).unwrap_or_else(|e| panic!("{e}"));
        let main = read(out.join("src/main.rs"));
        assert!(main.contains("compile_file_with_options"));
        assert!(main.contains("embedded: Some(&embedded::SOURCES)"));
        assert!(read(out.join("src/embedded.rs")).contains("files: None"));
    }

    fn github_config() -> RunConfig {
        RunConfig::from_toml(
            "[capabilities.stdio.println]\nenabled = true\n\n[extensions.github]\nview = [\"vectorian-rs/hiko\"]\n",
        )
        .unwrap()
    }

    #[cfg(feature = "ext-github")]
    #[test]
    fn extensions_are_linked_and_configured_in_the_generated_crate() {
        let dir = temp_dir("ext");
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            hiko_path: Some(repo()),
            out_dir: Some(dir.join("out")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
            ..BuildVmOptions::default()
        };
        let out = generate(&options, &github_config(), &[&hiko_ext_github::GITHUB])
            .unwrap_or_else(|e| panic!("{e}"));
        let main = read(out.join("src/main.rs"));
        assert!(main.contains("static EXTENSIONS: &str"));
        assert!(main.contains("vectorian-rs/hiko"));
        assert!(main.contains("[&hiko_ext_github::GITHUB]"));
        assert!(main.contains("Extension::compile_surface(&extensions)"));
        assert!(
            main.contains("builder.with_extension(extensions[0], &extension_tables[\"github\"])")
        );
        let cargo = read(out.join("Cargo.toml"));
        assert!(cargo.contains("hiko-ext-github = { path = "), "{cargo}");
        assert!(cargo.contains("toml = \"0.8\""));
    }

    #[test]
    fn unlinked_extensions_are_checked_when_the_artifact_starts() {
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            hiko_path: Some(repo()),
            out_dir: Some(temp_dir("unlinked-ext").join("out")),
            ..BuildVmOptions::default()
        };
        let out = generate(&options, &github_config(), &[]).unwrap_or_else(|e| panic!("{e}"));
        let main = read(out.join("src/main.rs"));
        assert!(main.contains("[&hiko_ext_github::GITHUB]"), "{main}");
        assert!(main.contains("error: [extensions.github]"), "{main}");
        assert!(!main.contains(".expect(\"extension"), "{main}");
        assert!(read(out.join("Cargo.toml")).contains("hiko-ext-github = { path = "));
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    #[test]
    fn lock_must_be_the_projects_lockfile() {
        let dir = temp_dir("other-lock");
        let other = dir.join("release.lock.toml");
        fs::copy(repo().join("hiko.lock.toml"), &other).unwrap();
        fs::copy(repo().join("hiko.toml"), dir.join("hiko.toml")).unwrap();
        // The project's own lockfile, which hiko.toml names.
        fs::copy(repo().join("hiko.lock.toml"), dir.join("hiko.lock.toml")).unwrap();
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            lock: Some(other),
            out_dir: Some(dir.join("out")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
            ..BuildVmOptions::default()
        };
        let err = generate(&options, &config(), &[]).expect_err("release.lock.toml is not used");
        assert!(
            err.contains("is not the lockfile its project uses"),
            "{err}"
        );
    }

    #[test]
    fn unsealed_build_refuses_a_directory_with_a_sealed_one() {
        let dir = temp_dir("stale");
        let out = dir.join("out");
        write(&out.join("build.rs"), "fn main() {}").unwrap();
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            out_dir: Some(out),
            ..BuildVmOptions::default()
        };
        let err = generate(&options, &config(), &[]).expect_err("stale build.rs");
        assert!(err.contains("holds a sealed build"), "{err}");
    }

    #[test]
    fn package_names_are_valid_cargo_names() {
        assert_eq!(package_name(Path::new("hiko-vm-reader")), "hiko-vm-reader");
        assert_eq!(package_name(Path::new("vm.sealed")), "vm-sealed");
        assert_eq!(package_name(Path::new("my vm")), "my-vm");
        assert_eq!(package_name(Path::new("1st")), "hiko-vm-1st");
        assert!(!package_name(Path::new(".")).is_empty());
        assert_eq!(package_name(Path::new("build")), "hiko-vm-build");
        assert_eq!(package_name(Path::new("self")), "hiko-vm-self");
    }

    #[test]
    fn unknown_extensions_are_refused() {
        let config = RunConfig::from_toml("[extensions.nope]\n").unwrap();
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            out_dir: Some(temp_dir("unknown-ext").join("out")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
            ..BuildVmOptions::default()
        };
        let err = generate(&options, &config, &[]).expect_err("nope is not an extension");
        assert!(err.contains("cannot link extension 'nope'"), "{err}");
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    #[test]
    fn script_with_an_extension_needs_it_linked_here() {
        let dir = temp_dir("script-ext");
        write(&dir.join("main.hml"), "val _ = println \"x\"\n").unwrap();
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            script: Some(dir.join("main.hml")),
            out_dir: Some(dir.join("out")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
            ..BuildVmOptions::default()
        };
        let err = generate(&options, &github_config(), &[]).expect_err("github is not linked");
        assert!(err.contains("feature ext-github"), "{err}");
    }

    #[cfg(feature = "ext-github")]
    #[test]
    fn extension_tables_are_checked_by_configure() {
        let config = RunConfig::from_toml("[extensions.github]\nbogus = 1\n").unwrap();
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            out_dir: Some(temp_dir("bad-ext").join("out")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
            ..BuildVmOptions::default()
        };
        let err =
            generate(&options, &config, &[&hiko_ext_github::GITHUB]).expect_err("unknown key");
        assert!(err.contains("[extensions.github]"), "{err}");
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    #[cfg(feature = "ext-aws")]
    #[test]
    fn a_sealed_script_must_embed_what_its_extension_modules_import() {
        // Aws.S3 imports Std.Result; without --lock nothing is sealed, so
        // build-vm refuses the script instead of building an artifact that
        // fails on its import. A script that imports no AWS module needs
        // nothing.
        let config = RunConfig::from_toml("[extensions.aws]\n").unwrap();
        let build = |name: &str, source: &str| {
            let dir = temp_dir(name);
            write(&dir.join("main.hml"), source).unwrap();
            let options = BuildVmOptions {
                config_path: "run.toml".into(),
                script: Some(dir.join("main.hml")),
                out_dir: Some(dir.join("out")),
                local_package_root: Some(repo().join("libraries")),
                cache_dir: Some(temp_dir("cache")),
                ..BuildVmOptions::default()
            };
            generate(&options, &config, &[&hiko_ext_aws::AWS])
        };
        let err =
            build("ext-s3", "import Aws.S3\nval _ = println \"hi\"\n").expect_err("not embedded");
        assert_eq!(
            err,
            "the script does not compile with only the embedded sources: module Aws.S3 of extension 'aws' needs Std.Result, which this artifact does not embed; build it with --lock and a lockfile that pins Std.Result"
        );
        build("ext-none", "val _ = println \"hi\"\n").expect("needs nothing");
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    /// Builds a sealed crate that links the GitHub extension (about a
    /// minute): `cargo test -p hiko-cli --features ext-github build_vm --
    /// --ignored`.
    #[cfg(feature = "ext-github")]
    #[test]
    #[ignore]
    fn sealed_artifact_with_an_extension_builds_and_runs() {
        let dir = temp_dir("e2e-ext");
        write(
            &dir.join("main.hml"),
            "import Github.Issue\nval _ = println \"extension linked\"\n",
        )
        .unwrap();
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            script: Some(dir.join("main.hml")),
            hiko_path: Some(repo()),
            out_dir: Some(dir.join("hiko-vm-e2e-ext")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
            ..BuildVmOptions::default()
        };
        let out = generate(&options, &github_config(), &[&hiko_ext_github::GITHUB])
            .unwrap_or_else(|e| panic!("{e}"));
        let built = std::process::Command::new(env!("CARGO"))
            .args(["build", "--release", "--quiet"])
            .current_dir(&out)
            .status()
            .unwrap();
        assert!(built.success());
        let run = std::process::Command::new(out.join("target/release/hiko-vm-e2e-ext"))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            "extension linked\n",
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[cfg(all(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
    /// Builds the generated crate with cargo (about a minute):
    /// `cargo test -p hiko-cli build_vm -- --ignored`.
    #[test]
    #[ignore]
    fn sealed_artifact_builds_runs_and_refuses_tampering() {
        let dir = temp_dir("e2e");
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            lock: Some(repo().join("hiko.lock.toml")),
            script: Some(script_project(&dir)),
            hiko_path: Some(repo()),
            out_dir: Some(dir.join("hiko-vm-e2e")),
            local_package_root: Some(repo().join("libraries")),
            cache_dir: Some(temp_dir("cache")),
        };
        let out = generate(&options, &config(), &[]).unwrap_or_else(|e| panic!("{e}"));
        let cargo = |dir: &Path| {
            std::process::Command::new(env!("CARGO"))
                .args(["build", "--release", "--quiet"])
                .current_dir(dir)
                .status()
                .unwrap()
                .success()
        };
        assert!(cargo(&out));
        let binary = out.join("target/release/hiko-vm-e2e");
        // An empty HOME: no module cache to fall back on.
        let run = std::process::Command::new(&binary)
            .env("HOME", dir.join("empty-home"))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&run.stdout), "sealed: some\n");
        let refused = std::process::Command::new(&binary)
            .arg("x.hml")
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(2));

        let module = module_file(&out, "Std.Option");
        let mut text = read(module.clone());
        text.push_str("(* tampered *)\n");
        fs::write(&module, text).unwrap();
        assert!(!cargo(&out), "a tampered module must fail the build");
    }
}
