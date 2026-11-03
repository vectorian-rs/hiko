//! `hiko build-vm`: generate a Rust crate whose binary bakes in one run
//! config and, when sealed (#59), the exact locked modules and program.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use hiko_compile::compiler::{
    Compiler, EmbeddedFile, EmbeddedModule, EmbeddedSources, LockedModuleSource,
    ModuleLoaderOptions, local_use_files,
};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
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
            None => {
                // Without the extension here its table cannot be checked, and
                // a bad table would only fail when the artifact starts.
                return Err(format!(
                    "[extensions.{name}] needs a hiko built with feature ext-{name}, so build-vm can check the table"
                ));
            }
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

/// A Cargo package name from the output directory: ASCII letters, digits,
/// `-` and `_`, starting with a letter.
pub(crate) fn package_name(out_dir: &Path) -> String {
    let base = std::fs::canonicalize(out_dir)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .or_else(|| {
            out_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let mut name: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name.insert_str(0, "hiko-vm-");
    }
    name.trim_end_matches('-').to_string()
}

fn collect_sealed(
    options: &BuildVmOptions,
    extensions: &[&'static Extension],
) -> Result<Sealed, String> {
    if let Some(lock) = &options.lock {
        let used = Compiler::project_lockfile_path(lock).map_err(|e| format!("{e:?}"))?;
        let same = match (fs::canonicalize(lock), fs::canonicalize(&used)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same {
            return Err(format!(
                "--lock '{}' is not the lockfile its project uses ('{}'); named imports resolve through the project's hiko.toml",
                lock.display(),
                used.display()
            ));
        }
    }
    let modules = match &options.lock {
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
                "cannot read the locked modules of '{}': {e:?}",
                lock.display()
            )
        })?,
        None => Vec::new(),
    };
    let script = match &options.script {
        Some(path) => {
            let source = fs::read_to_string(path)
                .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
            let files = local_use_files(path, &source).map_err(|e| format!("{e:?}"))?;
            Some((source, files))
        }
        None => None,
    };
    for m in &modules {
        lockfile_hex(&m.blake3).ok_or_else(|| {
            format!(
                "module '{}' has lockfile hash '{}', not blake3:<64 hex digits>",
                m.import_name, m.blake3
            )
        })?;
    }
    let sealed = Sealed { modules, script };
    check_sealed_program(&sealed, extensions)?;
    Ok(sealed)
}

/// The hex digest of a lockfile hash, normalized as the compiler does
/// (one optional `blake3:` prefix, lower case), if it is 64 hex digits.
fn lockfile_hex(hash: &str) -> Option<String> {
    let hex = hash.trim();
    let hex = hex
        .strip_prefix("blake3:")
        .unwrap_or(hex)
        .to_ascii_lowercase();
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

/// Compile the embedded program once, now, against exactly the embedded
/// sources, so a missing module or file fails `build-vm` instead of the
/// artifact.
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
        .map_err(|e| format!("the script does not compile with only the embedded sources: {e:?}"))
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
        let hash = lockfile_hex(&m.blake3).expect("checked in collect_sealed");
        writeln!(
            build,
            "        (\"sealed/modules/{i}.hml\", {hash:?}), // {}",
            m.import_name
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
    fn configured_extensions_must_be_linked_to_be_checked() {
        let options = BuildVmOptions {
            config_path: "run.toml".into(),
            out_dir: Some(temp_dir("unlinked-ext").join("out")),
            ..BuildVmOptions::default()
        };
        let err = generate(&options, &github_config(), &[]).expect_err("github is not linked");
        assert!(err.contains("feature ext-github"), "{err}");
    }

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
    }

    #[test]
    fn lockfile_hashes_are_normalized_like_the_compiler() {
        let hex = "ab".repeat(32);
        assert_eq!(
            lockfile_hex(&format!("blake3:{hex}")).as_deref(),
            Some(hex.as_str())
        );
        assert_eq!(
            lockfile_hex(&hex.to_uppercase()).as_deref(),
            Some(hex.as_str())
        );
        assert_eq!(lockfile_hex(&format!("blake3:blake3:{hex}")), None);
        assert_eq!(lockfile_hex("blake3:abc"), None);
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
