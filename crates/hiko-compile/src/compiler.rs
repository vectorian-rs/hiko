use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "loader-integrity-blake3")]
use std::time::Duration;

use hiko_builtin_meta::{internal_builtin_module, is_internal_builtin_package};
#[cfg(any(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
use hiko_common::blake3_hex;
#[cfg(feature = "loader-integrity-blake3")]
use hiko_common::http_get_text_limited;
use hiko_syntax::ast::*;
use hiko_syntax::intern::{StringInterner, Symbol};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::span::Span;
use hiko_syntax::token::{Token, TokenKind};
use hiko_types::infer::{InferCtx, TypeError};
use hiko_types::ty::Type;
use serde::Deserialize;

use crate::chunk::{Chunk, CompiledProgram, Constant, FunctionProto};
use crate::op::{
    HANDLER_CLAUSE_PROVIDER, Op, REC_CAPTURE_GROUP, REC_CAPTURE_LOCAL, REC_CAPTURE_UPVALUE,
};

/// Overrides for resolving named imports listed in `hiko.lock.toml`.
///
/// The defaults match the CLI: modules come from `~/.hiko/lib-cache` or are
/// downloaded from the lockfile `base_url`. Whatever the source, a module is
/// only accepted if its BLAKE3 hash matches the lockfile entry.
#[derive(Debug, Clone, Default)]
pub struct ModuleLoaderOptions {
    /// Module cache directory. `None` uses the manifest's `[defaults]
    /// lib_cache` (relative to `hiko.toml`), else `$HOME/.hiko/lib-cache`.
    pub cache_dir: Option<PathBuf>,
    /// Local package directory checked before the cache and the network.
    /// A module `Pkg.Mod` locked at version `V` is read from
    /// `<root>/Pkg-vV/modules/Mod.hml` if that file exists (the layout of the
    /// repository's `libraries/` directory). A hash mismatch is an error.
    pub local_package_root: Option<PathBuf>,
    /// Types, builtins and modules contributed by extensions.
    pub extensions: ExtensionSurface,
    /// Sources compiled into a sealed `build-vm` artifact. When set, named
    /// imports resolve only from these and never from the lockfile, the
    /// cache or the network.
    pub embedded: Option<&'static EmbeddedSources>,
}

/// What extensions add to the compiler: opaque types, builtin signatures,
/// and Hiko modules resolved by import name before the lockfile.
#[derive(Debug, Clone, Default)]
pub struct ExtensionSurface {
    pub types: Vec<&'static str>,
    pub builtins: Vec<(&'static str, hiko_builtin_meta::BuiltinTypeSignature)>,
    /// `(import name, source)`, such as `("Github.Issue", "structure ...")`.
    pub modules: Vec<(&'static str, &'static str)>,
    /// Library modules that extension modules import, per extension.
    pub requires: Vec<ExtensionRequires>,
}

/// The library modules, such as `Std.Result`, that one extension's modules
/// may import. They resolve like any named import, through the project's
/// lockfile or a sealed artifact's embedded sources. While an extension
/// module is compiled, each library module it imports directly must be listed
/// here, and one that is not available fails with an error naming the module
/// and the extension. A library module's own imports are not checked.
#[derive(Debug, Clone, Copy)]
pub struct ExtensionRequires {
    pub extension: &'static str,
    /// The extension's own module import names.
    pub modules: &'static [(&'static str, &'static str)],
    pub imports: &'static [&'static str],
}

impl ExtensionSurface {
    fn module(&self, import_name: &str) -> Option<(&'static str, &'static str)> {
        self.modules
            .iter()
            .find(|(name, _)| *name == import_name)
            .copied()
    }

    fn requires_for(&self, module: &str) -> Option<ExtensionRequires> {
        self.requires
            .iter()
            .find(|r| r.modules.iter().any(|(name, _)| *name == module))
            .copied()
    }

    fn has_builtin(&self, name: &str) -> bool {
        self.builtins.iter().any(|(builtin, _)| *builtin == name)
    }
}

/// Sources compiled into a sealed `build-vm` artifact (#59).
///
/// Named imports resolve only from `modules`, each checked against its
/// BLAKE3 hash when it is loaded. When `files` is `Some`, local `use` resolves
/// only from `files` and never reads the filesystem; their paths are relative
/// to the entry script's directory, normalized by [`embedded_file_path`].
#[derive(Debug)]
pub struct EmbeddedSources {
    pub modules: &'static [EmbeddedModule],
    pub files: Option<&'static [EmbeddedFile]>,
}

/// The local files (`use "..."`) that the script at `entry` uses, directly
/// or through other local files, keyed as [`embedded_file_path`] names them
/// relative to the script's directory. `build-vm --script` embeds these.
/// `source` is the entry script's text, read once by the caller. A path that
/// reaches its file through a symbolic link is an error. Sorted by key.
pub fn local_use_files(entry: &Path, source: &str) -> Result<Vec<(String, String)>, CompileError> {
    // Resolve from the script's real directory, as `hiko run` does.
    let entry = std::fs::canonicalize(entry)
        .map_err(|e| CompileError::codegen(format!("cannot resolve '{}': {e}", entry.display())))?;
    let root = entry.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut found = std::collections::BTreeMap::new();
    let mut pending = vec![(PathBuf::new(), source.to_string())];
    while let Some((dir, source)) = pending.pop() {
        let tokens = Lexer::new(&source, 0)
            .tokenize()
            .map_err(|e| CompileError::codegen(format!("lex error: {}", e.message)))?;
        let program = Parser::new(tokens)
            .parse_program()
            .map_err(|e| CompileError::codegen(format!("parse error: {}", e.message)))?;
        let program = hiko_syntax::desugar::desugar_program(program);
        let mut uses = Vec::new();
        collect_uses(&program.decls, &mut uses);
        for path in uses {
            let key = embedded_file_path(&dir, path)?;
            if found.contains_key(&key) {
                continue;
            }
            // The artifact resolves `..` lexically; `hiko run` follows
            // symbolic links. Resolve the path as written and refuse it if
            // the two differ.
            let written = root.join(&dir).join(path);
            let canonical = std::fs::canonicalize(&written).map_err(|e| {
                CompileError::codegen(format!(
                    "cannot read local use '{}': {e}",
                    written.display()
                ))
            })?;
            if canonical != lexical_normalize(&root.join(&key)) {
                return Err(CompileError::codegen(format!(
                    "local use '{path}' resolves through a symbolic link to '{}'; a sealed program needs plain paths",
                    canonical.display()
                )));
            }
            let text = std::fs::read_to_string(&canonical).map_err(|e| {
                CompileError::codegen(format!(
                    "cannot read local use '{}': {e}",
                    canonical.display()
                ))
            })?;
            let key_dir = Path::new(&key)
                .parent()
                .unwrap_or(Path::new(""))
                .to_path_buf();
            found.insert(key, text.clone());
            pending.push((key_dir, text));
        }
    }
    Ok(found.into_iter().collect())
}

/// `path` with `.` dropped and `..` applied, without touching the file system.
fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The paths of the `use` declarations in `decls`, including those inside
/// `local` blocks (a desugared structure is one).
fn collect_uses<'a>(decls: &'a [Decl], uses: &mut Vec<&'a str>) {
    for decl in decls {
        match &decl.kind {
            DeclKind::Use(path) => uses.push(path),
            DeclKind::Local(locals, body) => {
                collect_uses(locals, uses);
                collect_uses(body, uses);
            }
            _ => {}
        }
    }
}

/// The path of the first `use` in `tokens`, wherever it sits.
fn first_use_path(tokens: &[Token]) -> Option<&str> {
    tokens
        .windows(2)
        .find_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (TokenKind::Use, TokenKind::StringLit(path)) => Some(path.as_str()),
            _ => None,
        })
}

fn remote_use_refused(path: &str) -> CompileError {
    CompileError::codegen(format!(
        "local use '{path}' is not allowed in a remote module; use a named import instead"
    ))
}

/// A locked module's verified source, from [`Compiler::locked_module_sources`].
#[derive(Debug, Clone)]
pub struct LockedModuleSource {
    pub import_name: String,
    /// The lockfile hash, as written there.
    pub blake3: String,
    /// The same hash normalized as the loader compares it: 64 lower-case hex
    /// digits, which `source` has been verified against.
    pub digest: String,
    pub source: String,
}

/// A locked module in a sealed artifact.
#[derive(Debug)]
pub struct EmbeddedModule {
    /// Import name, such as `Std.List`.
    pub import_name: &'static str,
    /// Lockfile hash, `blake3:<hex>` or `<hex>`.
    pub blake3: &'static str,
    pub source: &'static str,
}

/// A local file (`use "..."`) in a sealed artifact.
#[derive(Debug)]
pub struct EmbeddedFile {
    pub path: &'static str,
    pub source: &'static str,
}

/// The key of an embedded local file: `path` joined to `base_dir` and
/// normalized lexically (`.` dropped, `..` applied), with `/` separators.
/// Absolute paths are refused. `build-vm` uses the same function to name the
/// files it embeds, so a lookup matches exactly when the paths agree.
pub fn embedded_file_path(base_dir: &Path, path: &str) -> Result<String, CompileError> {
    use std::path::Component;
    let mut parts: Vec<String> = Vec::new();
    for component in base_dir.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|p| p != "..") {
                    parts.pop();
                } else {
                    parts.push("..".to_string());
                }
            }
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::RootDir | Component::Prefix(_) => {
                return Err(CompileError::codegen(format!(
                    "local use '{path}' is an absolute path; an embedded program can only use relative paths"
                )));
            }
        }
    }
    Ok(parts.join("/"))
}

#[derive(Debug)]
pub enum CompileError {
    Type(TypeError),
    Codegen(String),
}

impl From<TypeError> for CompileError {
    fn from(e: TypeError) -> Self {
        CompileError::Type(e)
    }
}

impl CompileError {
    fn codegen(msg: impl Into<String>) -> Self {
        CompileError::Codegen(msg.into())
    }
}

/// How a project gets a package: the end of every error whose fix is to
/// declare and install it.
#[cfg(feature = "loader-integrity-blake3")]
const PACKAGE_ENTRY_AND_INSTALL: &str =
    "with its version and registry (see docs/language/imports.md), then run 'hiko install'";

/// The fix for a lockfile that disagrees with hiko.toml. Never the fix for a
/// hash failure: install would replace the pin that caught it.
#[cfg(feature = "loader-integrity-blake3")]
const INSTALL_REWRITES_LOCKFILE: &str =
    "run 'hiko install', which fetches the modules and rewrites the lockfile from hiko.toml";

/// The fix for a module file that does not read.
#[cfg(feature = "loader-integrity-blake3")]
const REPAIR_MODULE_FILE: &str = "repair or remove that file";

/// The fix for a module cache that cannot be written. `hiko install` writes
/// to the same path, so it does not repair this.
#[cfg(feature = "loader-integrity-blake3")]
const REPAIR_MODULE_CACHE: &str =
    "repair that path: the module cache must be a directory this user can write";

/// The fix for a module that fails its lockfile hash.
#[cfg(feature = "loader-integrity-blake3")]
const HASH_DIFFERS_FROM_PIN: &str = "the source differs from the lockfile's pin, so find out why at the registry before 'hiko install' replaces the pin";

#[cfg(feature = "loader-integrity-blake3")]
fn declare_package_fix(package: &str) -> String {
    format!("if hiko.toml does not declare {package}, add it {PACKAGE_ENTRY_AND_INSTALL}")
}

/// `importer`, as an error about an extension module names it.
fn extension_module(requires: &ExtensionRequires, importer: &str) -> String {
    format!("module {importer} of extension '{}'", requires.extension)
}

/// `error` as text for a user: the message of a loader error.
#[cfg(any(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
pub fn codegen_message(error: CompileError) -> String {
    match error {
        CompileError::Codegen(message) => message,
        other => format!("{other:?}"),
    }
}

/// `error` from loading `import`; when `extension`'s module imported it, the
/// error names that module and ends in `fix`.
#[cfg(any(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
fn in_extension_module(
    extension: Option<(ExtensionRequires, &'static str)>,
    import: &str,
    fix: &str,
    error: CompileError,
) -> CompileError {
    match extension {
        Some((requires, importer)) => CompileError::codegen(format!(
            "{} needs {import}: {}; {fix}",
            extension_module(&requires, importer),
            codegen_message(error)
        )),
        None => error,
    }
}

/// The fix for a loader error, when the module being loaded needs `package`.
#[cfg(feature = "loader-integrity-blake3")]
fn loader_error_fix(package: Option<&str>) -> String {
    package
        .map(|package| format!("; {}", declare_package_fix(package)))
        .unwrap_or_default()
}

/// Why a `hiko.toml` or a lockfile did not parse: line, column and reason. A
/// path can reach any file, so this leaves out the source line that
/// `toml::de::Error` prints.
pub fn toml_error_text(error: &toml::de::Error, source: &str) -> String {
    let Some(span) = error.span() else {
        return error.message().to_string();
    };
    let before = &source.as_bytes()[..span.start.min(source.len())];
    let line_start = before
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let line = before.iter().filter(|b| **b == b'\n').count() + 1;
    let column = String::from_utf8_lossy(&before[line_start..])
        .chars()
        .count()
        + 1;
    format!("line {line}, column {column}: {}", error.message())
}

/// The lockfile of the project whose manifest is `manifest_path`: `lockfile`,
/// relative to the manifest, else `hiko.lock.toml` next to it. A path that
/// leaves the manifest's directory is an error, whether or not it exists yet.
pub fn bounded_lockfile_path(
    manifest_path: &Path,
    lockfile: Option<&str>,
) -> Result<PathBuf, String> {
    let dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let lockfile = lockfile.unwrap_or("hiko.lock.toml");
    // `--config hiko.toml` has an empty parent.
    let root = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let root = std::fs::canonicalize(root).map_err(|e| {
        format!(
            "cannot resolve the directory of project manifest '{}': {e}",
            manifest_path.display()
        )
    })?;
    let outside = || {
        format!(
            "project manifest '{}' sets lockfile to '{lockfile}', which is outside its directory; use a path inside it",
            manifest_path.display()
        )
    };

    // A `..` above the deepest path that exists could climb anywhere. A
    // symlink that does not resolve cannot be shown to stay inside.
    let target = root.join(lockfile);
    let mut existing = target.as_path();
    while existing.symlink_metadata().is_err() {
        match existing.parent() {
            Some(parent) if existing.components().next_back() != Some(Component::ParentDir) => {
                existing = parent;
            }
            _ => return Err(outside()),
        }
    }
    match std::fs::canonicalize(existing) {
        Ok(real) if real.starts_with(&root) => Ok(dir.join(lockfile)),
        Ok(_) => Err(outside()),
        Err(e) => Err(format!(
            "project manifest '{}' uses lockfile '{lockfile}', but '{}' cannot be resolved ({e}); fix or remove it",
            manifest_path.display(),
            existing.display()
        )),
    }
}

/// `path` with its directory resolved and its file name kept: a link at the
/// file name must not select the project it leads to.
#[cfg(feature = "loader-integrity-blake3")]
pub fn resolve_directory_of(path: &Path) -> std::io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Ok(std::fs::canonicalize(dir)?.join(name))
}

#[cfg(feature = "loader-integrity-blake3")]
fn find_project_manifest_path(entry_file: &Path) -> Option<PathBuf> {
    let mut dir = entry_file.parent();
    while let Some(current) = dir {
        let candidate = current.join("hiko.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = current.parent();
    }
    None
}

#[cfg(feature = "loader-integrity-blake3")]
fn load_import_loader_config(
    entry_file: &Path,
    needed_package: Option<&str>,
) -> Result<ImportLoaderConfig, CompileError> {
    let manifest_path = find_project_manifest_path(entry_file).ok_or_else(|| {
        CompileError::codegen(format!(
            "named imports require a project hiko.toml above '{}'",
            entry_file.display()
        ))
    })?;
    let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|e| {
        CompileError::codegen(format!(
            "cannot read project manifest '{}': {e}",
            manifest_path.display()
        ))
    })?;
    let manifest: ImportProjectManifest = toml::from_str(&manifest_text).map_err(|e| {
        CompileError::codegen(format!(
            "invalid project manifest '{}': {}",
            manifest_path.display(),
            toml_error_text(&e, &manifest_text)
        ))
    })?;
    let lockfile_path =
        bounded_lockfile_path(&manifest_path, manifest.defaults.lockfile.as_deref())
            .map_err(CompileError::codegen)?;
    if !lockfile_path.is_file() {
        return Err(CompileError::codegen(format!(
            "project manifest '{}' references missing module lockfile '{}'{}",
            manifest_path.display(),
            lockfile_path.display(),
            loader_error_fix(needed_package)
        )));
    }
    Ok(ImportLoaderConfig {
        manifest_path,
        lockfile_path,
        manifest,
    })
}

#[cfg(feature = "loader-integrity-blake3")]
fn default_module_cache_dir() -> Result<PathBuf, CompileError> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home).join(".hiko").join("lib-cache"));
    }
    Err(CompileError::codegen(
        "cannot determine module cache directory; HOME is not set",
    ))
}

/// The module cache of the project whose manifest is `manifest_path`: its
/// `[defaults] lib_cache`, relative to the manifest, else
/// `$HOME/.hiko/lib-cache`. An embedder's `ModuleLoaderOptions::cache_dir`
/// overrides both; `hiko install` has none.
#[cfg(feature = "loader-integrity-blake3")]
pub fn project_module_cache_dir(
    manifest_path: &Path,
    lib_cache: Option<&str>,
) -> Result<PathBuf, CompileError> {
    match lib_cache {
        Some(dir) => Ok(manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(dir)),
        None => default_module_cache_dir(),
    }
}

/// The cache file for the module at `module_url` with BLAKE3 `hash` (hex,
/// without the `blake3:` prefix).
#[cfg(feature = "loader-integrity-blake3")]
pub fn module_cache_path(cache_dir: &Path, module_url: &str, hash: &str) -> PathBuf {
    let url_hash = blake3_hex(module_url.as_bytes());
    cache_dir.join(format!("{url_hash}-{hash}.hml"))
}

/// Store `source`, already checked against `hash`, as the cached copy of
/// the module at `module_url`.
#[cfg(feature = "loader-integrity-blake3")]
pub fn store_cached_module(
    cache_dir: &Path,
    module_url: &str,
    hash: &str,
    source: &str,
) -> Result<PathBuf, CompileError> {
    std::fs::create_dir_all(cache_dir).map_err(|e| {
        CompileError::codegen(format!(
            "cannot create module cache '{}': {e}",
            cache_dir.display()
        ))
    })?;
    let cache_path = module_cache_path(cache_dir, module_url, hash);
    write_verified_cache_file(&cache_path, source.as_bytes())?;
    Ok(cache_path)
}

#[cfg(feature = "loader-integrity-blake3")]
const REMOTE_MODULE_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(feature = "loader-integrity-blake3")]
const REMOTE_MODULE_MAX_BYTES: u64 = 1024 * 1024;

#[cfg(feature = "loader-integrity-blake3")]
fn fetch_remote_module(url: &str, module_name: &str) -> Result<String, CompileError> {
    http_get_text_limited(url, REMOTE_MODULE_TIMEOUT, REMOTE_MODULE_MAX_BYTES).map_err(|e| {
        CompileError::codegen(format!(
            "cannot fetch named import '{module_name}' from '{url}': {e}"
        ))
    })
}

#[cfg(any(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
fn verify_blake3(bytes: &[u8], expected_hash: &str, label: &str) -> Result<(), CompileError> {
    let actual = blake3_hex(bytes);
    if actual == expected_hash {
        Ok(())
    } else {
        Err(CompileError::codegen(format!(
            "{label} failed BLAKE3 verification: expected {expected_hash}, got {actual}"
        )))
    }
}

#[cfg(feature = "loader-integrity-blake3")]
fn write_verified_cache_file(cache_path: &Path, bytes: &[u8]) -> Result<(), CompileError> {
    let file_name = cache_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("module-cache.hml");
    let tmp_path = cache_path.with_file_name(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp_path, bytes).map_err(|e| {
        CompileError::codegen(format!(
            "cannot write cached module '{}': {e}",
            tmp_path.display()
        ))
    })?;
    if let Err(err) = std::fs::rename(&tmp_path, cache_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(CompileError::codegen(format!(
            "cannot write cached module '{}': {err}",
            cache_path.display()
        )));
    }
    Ok(())
}

#[cfg(feature = "loader-integrity-blake3")]
fn validate_lockfile(lockfile: &ImportLockfile, path: &Path) -> Result<(), CompileError> {
    if lockfile.schema_version != 1 {
        return Err(CompileError::codegen(format!(
            "unsupported module lockfile schema_version {} in '{}'; expected 1",
            lockfile.schema_version,
            path.display()
        )));
    }
    for (package_name, package) in &lockfile.packages {
        validate_package_name(
            package_name,
            &format!("package entry in '{}'", path.display()),
        )?;
        if package.version.trim().is_empty() {
            return Err(CompileError::codegen(format!(
                "package '{package_name}' in '{}' is missing version",
                path.display()
            )));
        }
        if package.base_url.trim().is_empty() {
            return Err(CompileError::codegen(format!(
                "package '{package_name}' in '{}' is missing base_url",
                path.display()
            )));
        }
        validate_remote_base_url(package_name, &package.base_url, path)?;
        for (module_name, hash) in &package.modules {
            if module_name.trim().is_empty() {
                return Err(CompileError::codegen(format!(
                    "package '{package_name}' in '{}' contains an empty module name",
                    path.display()
                )));
            }
            if module_name.contains('.') {
                return Err(CompileError::codegen(format!(
                    "module '{package_name}.{module_name}' in '{}' must be a single identifier",
                    path.display()
                )));
            }
            if hash.trim().is_empty() {
                return Err(CompileError::codegen(format!(
                    "module '{package_name}.{module_name}' in '{}' is missing a BLAKE3 hash",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(feature = "loader-integrity-blake3")]
fn validate_lockfile_against_manifest(
    lockfile: &ImportLockfile,
    lockfile_path: &Path,
    config: &ImportLoaderConfig,
    needed_package: Option<&str>,
) -> Result<(), CompileError> {
    let needed = |package: &str| needed_package.filter(|p| *p == package);
    let fix = |package: &str| loader_error_fix(needed(package));
    let rewrite_fix = |package: &str| {
        needed(package)
            .map(|_| format!("; {INSTALL_REWRITES_LOCKFILE}"))
            .unwrap_or_default()
    };
    let dependencies = &config.manifest.dependencies;
    if dependencies.is_empty() {
        return Ok(());
    }

    for package_name in lockfile.packages.keys() {
        if !dependencies.contains_key(package_name) {
            return Err(CompileError::codegen(format!(
                "package '{package_name}' in '{}' is not declared in project manifest '{}'{}",
                lockfile_path.display(),
                config.manifest_path.display(),
                fix(package_name)
            )));
        }
    }

    for (package_name, dependency) in dependencies {
        let package = lockfile.packages.get(package_name).ok_or_else(|| {
            CompileError::codegen(format!(
                "dependency '{package_name}' declared in project manifest '{}' is missing from module lockfile '{}'{}",
                config.manifest_path.display(),
                lockfile_path.display(),
                fix(package_name)
            ))
        })?;
        if package.version != dependency.version {
            return Err(CompileError::codegen(format!(
                "dependency '{package_name}' version mismatch: project manifest '{}' requires {}, but module lockfile '{}' resolves {}{}",
                config.manifest_path.display(),
                dependency.version,
                lockfile_path.display(),
                package.version,
                rewrite_fix(package_name)
            )));
        }
        let registry = config
            .manifest
            .registries
            .get(&dependency.registry)
            .ok_or_else(|| {
                CompileError::codegen(format!(
                    "dependency '{package_name}' references unknown registry '{}' in project manifest '{}'{}",
                    dependency.registry,
                    config.manifest_path.display(),
                    needed(package_name)
                        .map(|_| format!(
                            "; add [registries.{}] to hiko.toml (see docs/language/imports.md), then run 'hiko install'",
                            dependency.registry
                        ))
                        .unwrap_or_default()
                ))
            })?;
        let expected_base_url = format!(
            "{}/{}-v{}",
            registry.url.trim_end_matches('/'),
            package_name,
            dependency.version
        );
        if package.base_url.trim_end_matches('/') != expected_base_url {
            return Err(CompileError::codegen(format!(
                "dependency '{package_name}' source mismatch: project manifest '{}' expects '{}', but module lockfile '{}' resolves '{}'{}",
                config.manifest_path.display(),
                expected_base_url,
                lockfile_path.display(),
                package.base_url,
                rewrite_fix(package_name)
            )));
        }
    }

    Ok(())
}

#[cfg(feature = "loader-integrity-blake3")]
fn validate_remote_base_url(
    package_name: &str,
    base_url: &str,
    path: &Path,
) -> Result<(), CompileError> {
    let base_url = base_url.trim();
    if base_url.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = base_url.strip_prefix("http://") {
        // The authority ends at the first '/', '?' or '#'. '\' is treated as a
        // terminator too, since some URL parsers read it as a path separator.
        let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or_default();
        let host = authority
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(authority);
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.split_once(']').map(|(host, _)| host))
            .unwrap_or_else(|| host.split(':').next().unwrap_or(host));
        if matches!(host, "localhost" | "127.0.0.1" | "::1") {
            return Ok(());
        }
    }
    Err(CompileError::codegen(format!(
        "package '{package_name}' in '{}' uses insecure base_url '{base_url}'; use https:// or local http://127.0.0.1/localhost for development",
        path.display()
    )))
}

fn validate_package_name(package_name: &str, context: &str) -> Result<(), CompileError> {
    if package_name.trim().is_empty() {
        return Err(CompileError::codegen(format!(
            "{context} has an empty package name"
        )));
    }
    if package_name.starts_with("__") {
        return Err(CompileError::codegen(format!(
            "{context} uses reserved package name '{package_name}'; names starting with '__' are reserved"
        )));
    }
    Ok(())
}

fn split_import_name(module_name: &str) -> Result<(&str, &str), CompileError> {
    let Some((package, module)) = module_name.split_once('.') else {
        return Err(CompileError::codegen(format!(
            "named import '{module_name}' must use the shape Package.Module"
        )));
    };
    if module.contains('.') {
        return Err(CompileError::codegen(format!(
            "named import '{module_name}' must use the shape Package.Module"
        )));
    }
    if !is_internal_builtin_package(package) {
        validate_package_name(package, &format!("named import '{module_name}'"))?;
    }
    Ok((package, module))
}

#[cfg(any(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
fn normalize_blake3(hash: &str) -> String {
    hash.trim()
        .strip_prefix("blake3:")
        .unwrap_or(hash.trim())
        .to_ascii_lowercase()
}

#[cfg(feature = "loader-embedded")]
fn resolve_embedded_module(
    embedded: &'static EmbeddedSources,
    module_name: &str,
    extension: Option<(ExtensionRequires, &'static str)>,
) -> Result<ResolvedImport, CompileError> {
    let module = embedded
        .modules
        .iter()
        .find(|m| m.import_name == module_name)
        .ok_or_else(|| {
            CompileError::codegen(format!(
                "named import '{module_name}' is not embedded in this artifact"
            ))
        })?;
    verify_blake3(
        module.source.as_bytes(),
        &normalize_blake3(module.blake3),
        &format!("embedded module '{module_name}'"),
    )
    .map_err(|e| {
        in_extension_module(
            extension,
            module_name,
            "the artifact's source differs from its pin, so rebuild it with build-vm --lock",
            e,
        )
    })?;
    Ok(ResolvedImport::Embedded {
        key: ImportKey::Synthetic(module.import_name),
        display_name: module_name.to_string(),
        source: module.source,
        base_dir: PathBuf::new(),
        module: true,
    })
}

#[cfg(not(feature = "loader-embedded"))]
fn resolve_embedded_module(
    _embedded: &'static EmbeddedSources,
    module_name: &str,
    _extension: Option<(ExtensionRequires, &'static str)>,
) -> Result<ResolvedImport, CompileError> {
    Err(CompileError::codegen(format!(
        "named import '{module_name}' needs embedded sources; enable cargo feature 'loader-embedded'"
    )))
}

struct Local {
    name: String,
    depth: u32,
    /// Set when a variable reference resolves to this local, either directly
    /// or as an upvalue of a nested function.
    used: Cell<bool>,
    /// A handler clause's continuation variable. It is not a value: its
    /// only use is `resume k e` at the end of a provider clause.
    continuation: bool,
}

struct UpvalueDesc {
    is_local: bool,
    index: u16,
}

/// A compiled function prototype whose closure has not been emitted yet.
struct PendingClosure {
    proto_idx: u16,
    n_captures: u8,
    upvalues: Vec<UpvalueDesc>,
}

struct FuncCtx {
    chunk: Chunk,
    locals: Vec<Local>,
    max_locals: usize,
    upvalues: Vec<UpvalueDesc>,
    scope_depth: u32,
    arity: u8,
    name: Option<String>,
    /// Constant pool indexes already used by this chunk, so repeated
    /// literals and global names share one entry.
    string_constants: HashMap<String, u16>,
    scalar_constants: HashMap<ScalarConstant, u16>,
    /// Slot of the continuation variable when this function is a provider
    /// clause; `resume k e` in its tail position returns `e`.
    provider_cont_slot: Option<u16>,
    /// Proto index reserved for this function before its body was compiled,
    /// so calls inside the body can be direct.
    reserved_proto: Option<u16>,
    /// This is the outermost function of a local recursive binding, so a
    /// call of its own name in its body is `CallSelf`.
    self_callable: bool,
}

impl FuncCtx {
    fn new(name: Option<String>, arity: u8) -> Self {
        FuncCtx {
            chunk: Chunk::default(),
            locals: Vec::new(),
            max_locals: 0,
            upvalues: Vec::new(),
            scope_depth: 0,
            arity,
            name,
            string_constants: HashMap::new(),
            scalar_constants: HashMap::new(),
            provider_cont_slot: None,
            reserved_proto: None,
            self_callable: false,
        }
    }
}

/// A top-level function whose tuple parameter is passed as separate
/// arguments: `flat` has one parameter per component, `wrapper` takes the
/// tuple and calls `flat`.
#[derive(Clone, Copy)]
struct FlatEntry {
    wrapper: u16,
    flat: u16,
    width: usize,
}

/// Hashable identity of a non-string constant. Floats compare by bit
/// pattern, so `0.0` and `-0.0` stay distinct.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ScalarConstant {
    Int(i64),
    Float(u64),
    Word(u64),
    Char(char),
}

#[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
#[derive(Debug, Clone)]
struct ImportLoaderConfig {
    manifest_path: PathBuf,
    lockfile_path: PathBuf,
    manifest: ImportProjectManifest,
}

#[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportManifestDefaults {
    lockfile: Option<String>,
    /// Module cache directory, relative to the manifest's directory.
    lib_cache: Option<String>,
    #[serde(default)]
    #[serde(rename = "policy")]
    _policy: Option<String>,
}

#[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRegistry {
    url: String,
}

#[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportDependency {
    version: String,
    registry: String,
}

#[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportProjectManifest {
    #[serde(default)]
    #[serde(rename = "project")]
    _project: toml::Table,
    #[serde(default)]
    defaults: ImportManifestDefaults,
    #[serde(default)]
    registries: HashMap<String, ImportRegistry>,
    #[serde(default)]
    dependencies: HashMap<String, ImportDependency>,
    #[serde(default)]
    #[serde(rename = "policies")]
    _policies: toml::Table,
}

#[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportLockfile {
    schema_version: u32,
    #[serde(default)]
    packages: HashMap<String, LockedPackage>,
}

#[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedPackage {
    version: String,
    base_url: String,
    #[serde(default)]
    modules: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ImportKey {
    File(PathBuf),
    #[cfg(feature = "loader-integrity-blake3")]
    VerifiedFile(PathBuf),
    Synthetic(&'static str),
    EmbeddedFile(&'static str),
}

#[derive(Debug)]
enum ResolvedImport {
    File {
        canonical: PathBuf,
        display_name: String,
    },
    #[cfg(feature = "loader-integrity-blake3")]
    VerifiedFile {
        canonical: PathBuf,
        display_name: String,
        source: String,
    },
    Synthetic {
        import_name: &'static str,
        display_name: String,
        source: &'static str,
        base_dir: PathBuf,
    },
    /// A source compiled into a sealed artifact (`EmbeddedSources`).
    Embedded {
        key: ImportKey,
        display_name: String,
        source: &'static str,
        base_dir: PathBuf,
        /// A locked module: like a remote module, it may not `use` files.
        module: bool,
    },
}

impl ResolvedImport {
    fn key(&self) -> ImportKey {
        match self {
            Self::File { canonical, .. } => ImportKey::File(canonical.clone()),
            #[cfg(feature = "loader-integrity-blake3")]
            Self::VerifiedFile { canonical, .. } => ImportKey::VerifiedFile(canonical.clone()),
            Self::Synthetic { import_name, .. } => ImportKey::Synthetic(import_name),
            Self::Embedded { key, .. } => key.clone(),
        }
    }

    /// Whether the source is checked against a module pin, including embedded sources.
    fn is_remote(&self) -> bool {
        match self {
            #[cfg(feature = "loader-integrity-blake3")]
            Self::VerifiedFile { .. } => true,
            Self::Embedded { module, .. } => *module,
            _ => false,
        }
    }

    fn display_name(&self) -> &str {
        match self {
            Self::File { display_name, .. } => display_name,
            #[cfg(feature = "loader-integrity-blake3")]
            Self::VerifiedFile { display_name, .. } => display_name,
            Self::Synthetic { display_name, .. } => display_name,
            Self::Embedded { display_name, .. } => display_name,
        }
    }

    fn load_source(&self) -> Result<(Cow<'_, str>, PathBuf), CompileError> {
        match self {
            Self::File { canonical, .. } => {
                let source = std::fs::read_to_string(canonical).map_err(|e| {
                    CompileError::codegen(format!("cannot read '{}': {e}", canonical.display()))
                })?;
                let base_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
                Ok((Cow::Owned(source), base_dir))
            }
            #[cfg(feature = "loader-integrity-blake3")]
            Self::VerifiedFile {
                canonical, source, ..
            } => {
                let base_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
                Ok((Cow::Borrowed(source.as_str()), base_dir))
            }
            Self::Synthetic {
                source, base_dir, ..
            }
            | Self::Embedded {
                source, base_dir, ..
            } => Ok((Cow::Borrowed(*source), base_dir.clone())),
        }
    }
}

#[derive(Debug)]
struct CachedImportedProgram {
    base_dir: PathBuf,
    decls: Vec<Decl>,
}

pub struct Compiler {
    functions: Vec<FunctionProto>,
    func_stack: Vec<FuncCtx>,
    constructor_tags: HashMap<String, u16>,
    constructor_arities: HashMap<String, u8>,
    /// Effect name -> tag for effect handler dispatch.
    effect_tags: HashMap<String, u16>,
    next_effect_tag: u16,
    /// Base directory for resolving relative local file includes via `use`.
    base_dir: PathBuf,
    /// Entry file for file-based compilation. External named-import loader state is derived lazily from this.
    #[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
    entry_file: Option<PathBuf>,
    /// Imports currently being loaded (for cycle detection).
    loading_imports: HashSet<ImportKey>,
    /// The extension module being inferred, if any: its library imports are
    /// checked against its extension's `requires`.
    importing_extension: Option<(ExtensionRequires, &'static str)>,
    /// Imports whose type inference is complete.
    inferred_imports: HashSet<ImportKey>,
    /// Imports whose codegen is complete.
    compiled_imports: HashSet<ImportKey>,
    /// Cached desugared programs for imported modules (used between passes).
    imported_programs: HashMap<ImportKey, CachedImportedProgram>,
    /// Named module lockfile loaded lazily from the entry program's project root.
    #[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
    import_lockfile: Option<ImportLockfile>,
    /// Where `import_lockfile` was read from.
    #[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
    import_lockfile_path: Option<PathBuf>,
    /// Cache directory for fetched remote modules.
    #[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
    module_cache_dir: Option<PathBuf>,
    /// Caller-supplied overrides for named-import resolution.
    #[cfg_attr(not(feature = "loader-integrity-blake3"), allow(dead_code))]
    loader_options: ModuleLoaderOptions,
    /// Global name -> proto_idx of the capture-free function it is currently
    /// bound to, for `CallDirect`. Every global rebinding clears the entry.
    global_protos: HashMap<String, u16>,
    /// Proto index the next `push_new_function` compiles into (see
    /// `reserve_proto`).
    pending_reserved_proto: Option<u16>,
    /// The next `push_new_function` starts a local recursive binding's
    /// function (see `FuncCtx::self_callable`).
    pending_self_callable: bool,
    /// Global name -> flat entry of a top-level function that takes one
    /// tuple (see `flat_tuple_width`). Valid only while `global_protos` still
    /// maps the name to the entry's wrapper.
    global_flat_protos: HashMap<String, FlatEntry>,
    /// Source name -> VM global name of the top-level binding in scope. A
    /// binding that shadows an earlier one, or a builtin, gets a fresh name
    /// (`x$1`, `x$2`, ...), so code compiled earlier keeps its binding.
    /// `$` cannot appear in a source identifier.
    global_bindings: HashMap<String, String>,
    /// Next identity suffix per source name; never reused after shadowing.
    global_versions: HashMap<String, usize>,
    /// Slots read by function/closure code that can run after a later rebind.
    pinned_global_slots: HashSet<u16>,
    /// VM global name -> slot, and the names in slot order.
    global_slots: HashMap<String, u16>,
    global_names: Vec<Arc<str>>,
    /// Shared type inference context (imports add to the same environment).
    infer_ctx: InferCtx,
    /// String interner for resolving symbols from the AST.
    interner: StringInterner,
}

impl Compiler {
    /// Compile a program from a file. Type inference is run first.
    /// `file_path` is used to resolve relative `use` imports.
    pub fn compile_file(
        program: Program,
        file_path: &Path,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        Self::compile_file_with_options(program, file_path, ModuleLoaderOptions::default())
    }

    /// Like [`Compiler::compile_file`], with overrides for how named imports
    /// listed in `hiko.lock.toml` are resolved. Every module is still
    /// verified against its lockfile BLAKE3 hash.
    pub fn compile_file_with_options(
        program: Program,
        file_path: &Path,
        loader_options: ModuleLoaderOptions,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        let base_dir = file_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let canonical =
            std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
        let mut loading = HashSet::new();
        loading.insert(ImportKey::File(canonical.clone()));
        Self::compile_with_ctx(program, base_dir, Some(canonical), loading, loader_options)
    }

    /// The lockfile that named imports in the project of `entry_file` use:
    /// the one its `hiko.toml` names.
    #[cfg(all(
        feature = "loader-named-imports",
        feature = "loader-http",
        feature = "loader-integrity-blake3"
    ))]
    pub fn project_lockfile_path(entry_file: &Path) -> Result<PathBuf, CompileError> {
        Ok(load_import_loader_config(entry_file, None)?.lockfile_path)
    }

    /// The source of every module in the lockfile of the project that
    /// `entry_file` belongs to, each read through the normal loader (local
    /// package root, cache, then network) and checked against its BLAKE3
    /// hash. `build-vm` embeds these in a sealed artifact. Sorted by import
    /// name.
    #[cfg(feature = "loader-integrity-blake3")]
    pub fn locked_module_sources(
        entry_file: &Path,
        loader_options: ModuleLoaderOptions,
    ) -> Result<Vec<LockedModuleSource>, CompileError> {
        let canonical = resolve_directory_of(entry_file).unwrap_or_else(|_| entry_file.into());
        let base_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut c = Compiler::new(
            base_dir,
            Some(canonical),
            HashSet::new(),
            loader_options,
            StringInterner::default(),
        );
        let (lockfile, _, _) = c.ensure_import_loader("the lockfile", None)?;
        let mut names = Vec::new();
        for (package_name, package) in &lockfile.packages {
            for (module, hash) in &package.modules {
                names.push((format!("{package_name}.{module}"), hash.clone()));
            }
        }
        names.sort();
        let mut sources = Vec::new();
        for (import_name, blake3) in names {
            let (package_name, leaf_module) = split_import_name(&import_name)?;
            let ResolvedImport::VerifiedFile { source, .. } =
                c.resolve_external_named_import(&import_name, package_name, leaf_module)?
            else {
                unreachable!("external named imports resolve to verified files");
            };
            sources.push(LockedModuleSource {
                import_name,
                digest: normalize_blake3(&blake3),
                blake3,
                source,
            });
        }
        Ok(sources)
    }

    /// Compile a sealed artifact's embedded entry program. Imports resolve
    /// only from `options.embedded`, and local `use` paths are relative to
    /// the entry script's directory.
    pub fn compile_embedded(
        program: Program,
        options: ModuleLoaderOptions,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        Self::compile_with_ctx(program, PathBuf::new(), None, HashSet::new(), options)
    }

    /// Compile a program without a file context (e.g., from a string).
    pub fn compile(
        program: Program,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        Self::compile_with_ctx(
            program,
            PathBuf::from("."),
            None,
            HashSet::new(),
            ModuleLoaderOptions::default(),
        )
    }

    /// Like [`Compiler::compile`], with loader options (and so extensions).
    pub fn compile_with_options(
        program: Program,
        loader_options: ModuleLoaderOptions,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        Self::compile_with_ctx(
            program,
            PathBuf::from("."),
            None,
            HashSet::new(),
            loader_options,
        )
    }

    fn new(
        base_dir: PathBuf,
        entry_file: Option<PathBuf>,
        loading_imports: HashSet<ImportKey>,
        loader_options: ModuleLoaderOptions,
        interner: StringInterner,
    ) -> Compiler {
        let mut c = Compiler {
            functions: Vec::new(),
            func_stack: vec![FuncCtx::new(None, 0)],
            constructor_tags: HashMap::new(),
            constructor_arities: HashMap::new(),
            effect_tags: HashMap::new(),
            next_effect_tag: 0,
            base_dir,
            entry_file,
            loading_imports,
            importing_extension: None,
            inferred_imports: HashSet::new(),
            compiled_imports: HashSet::new(),
            imported_programs: HashMap::new(),
            import_lockfile: None,
            import_lockfile_path: None,
            module_cache_dir: None,
            loader_options,
            global_protos: HashMap::new(),
            pending_reserved_proto: None,
            pending_self_callable: false,
            global_flat_protos: HashMap::new(),
            global_bindings: HashMap::new(),
            global_versions: HashMap::new(),
            pinned_global_slots: HashSet::new(),
            global_slots: HashMap::new(),
            global_names: Vec::new(),
            infer_ctx: InferCtx::new(),
            interner,
        };
        c.infer_ctx.interner = c.interner.clone();
        c
    }

    fn compile_with_ctx(
        program: Program,
        base_dir: PathBuf,
        entry_file: Option<PathBuf>,
        loading_imports: HashSet<ImportKey>,
        loader_options: ModuleLoaderOptions,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        // Desugar and constant-fold
        let program = hiko_syntax::desugar::desugar_program(program);
        let program = hiko_syntax::constfold::fold_program(program);
        let interner = program.interner;
        let desugared = program.decls;

        let mut c = Compiler::new(
            base_dir,
            entry_file,
            loading_imports,
            loader_options,
            interner,
        );
        let surface = &c.loader_options.extensions;
        for ty in &surface.types {
            c.infer_ctx
                .add_opaque_type(ty)
                .map_err(CompileError::Codegen)?;
        }
        for (name, sig) in &surface.builtins {
            c.infer_ctx
                .bind_extension_builtin(name, *sig)
                .map_err(CompileError::Codegen)?;
        }

        // Pass 1: type inference (all type errors reported before any codegen)
        for decl in &desugared {
            c.infer_decl_pass(decl)?;
        }

        // Sync interner back after inference (imports may have added symbols)
        c.interner = c.infer_ctx.interner.clone();

        // Pass 2: codegen
        for decl in &desugared {
            c.compile_decl(decl)?;
        }
        c.emit(Op::Halt);
        let main = c.func_stack.pop().unwrap();
        let warnings = c.infer_ctx.warnings;
        Ok((
            CompiledProgram {
                main: Arc::new(main.chunk),
                main_n_locals: checked_u16(main.max_locals, "local slots")?,
                functions: Arc::<[FunctionProto]>::from(c.functions),
                effects: Arc::<[crate::chunk::EffectMeta]>::from(
                    c.effect_tags
                        .into_iter()
                        .map(|(name, tag)| crate::chunk::EffectMeta { name, tag })
                        .collect::<Vec<_>>(),
                ),
                globals: Arc::from(c.global_names),
            },
            warnings,
        ))
    }

    // ── Utilities ────────────────────────────────────────────────────

    fn ctx(&self) -> &FuncCtx {
        self.func_stack.last().unwrap()
    }
    fn ctx_mut(&mut self) -> &mut FuncCtx {
        self.func_stack.last_mut().unwrap()
    }
    fn chunk(&mut self) -> &mut Chunk {
        &mut self.func_stack.last_mut().unwrap().chunk
    }
    fn emit(&mut self, op: Op) {
        self.chunk().emit_op(op);
    }
    fn emit_span(&mut self, span: hiko_syntax::span::Span) {
        self.chunk().record_span(span);
    }
    fn emit_u8(&mut self, val: u8) {
        self.chunk().emit_u8(val);
    }
    fn emit_u16(&mut self, val: u16) {
        self.chunk().emit_u16(val);
    }
    fn emit_constant(&mut self, c: Constant) -> Result<(), CompileError> {
        let idx = self.add_constant(c)?;
        self.emit(Op::Const);
        self.emit_u16(idx);
        Ok(())
    }
    fn emit_jump(&mut self, op: Op) -> usize {
        self.chunk().emit_jump(op)
    }
    fn patch_jump(&mut self, pos: usize) -> Result<(), CompileError> {
        self.chunk().patch_jump(pos).map_err(CompileError::codegen)
    }
    /// Add a constant to the current chunk, reusing an existing equal entry.
    fn add_constant(&mut self, c: Constant) -> Result<u16, CompileError> {
        let key = match &c {
            Constant::String(s) => return self.add_string_constant(s),
            Constant::Int(n) => ScalarConstant::Int(*n),
            Constant::Float(f) => ScalarConstant::Float(f.to_bits()),
            Constant::Word(w) => ScalarConstant::Word(*w),
            Constant::Char(c) => ScalarConstant::Char(*c),
        };
        if let Some(&idx) = self.ctx().scalar_constants.get(&key) {
            return Ok(idx);
        }
        let idx = self
            .chunk()
            .add_constant(c)
            .map_err(CompileError::codegen)?;
        self.ctx_mut().scalar_constants.insert(key, idx);
        Ok(idx)
    }

    fn add_string_constant(&mut self, s: &str) -> Result<u16, CompileError> {
        if let Some(&idx) = self.ctx().string_constants.get(s) {
            return Ok(idx);
        }
        let idx = self
            .chunk()
            .add_constant(Constant::String(s.to_string()))
            .map_err(CompileError::codegen)?;
        self.ctx_mut().string_constants.insert(s.to_string(), idx);
        Ok(idx)
    }

    fn push_new_function(&mut self, name: Option<String>) {
        let mut ctx = FuncCtx::new(name, 1);
        ctx.reserved_proto = self.pending_reserved_proto.take();
        ctx.self_callable = std::mem::take(&mut self.pending_self_callable);
        self.func_stack.push(ctx);
    }

    /// Reserve a proto index for a function whose body is not compiled yet,
    /// so calls to it can be compiled first. The next `push_new_function`
    /// compiles into it.
    fn reserve_proto(&mut self) -> Result<u16, CompileError> {
        let idx = checked_u16(self.functions.len(), "functions")?;
        self.functions.push(FunctionProto {
            name: None,
            arity: 0,
            n_captures: 0,
            n_locals: 0,
            chunk: Chunk::default(),
        });
        Ok(idx)
    }

    // ── Variable resolution ──────────────────────────────────────────

    fn resolve_local(&self, name: &str) -> Option<u16> {
        for (i, local) in self.ctx().locals.iter().enumerate().rev() {
            if local.name == name {
                local.used.set(true);
                return Some(i as u16);
            }
        }
        None
    }

    fn resolve_upvalue(&mut self, name: &str) -> Option<u16> {
        let depth = self.func_stack.len();
        self.resolve_upvalue_at(name, depth - 1)
    }

    fn resolve_upvalue_at(&mut self, name: &str, func_idx: usize) -> Option<u16> {
        if func_idx == 0 {
            return None;
        }
        for (i, local) in self.func_stack[func_idx - 1]
            .locals
            .iter()
            .enumerate()
            .rev()
        {
            if local.name == name {
                local.used.set(true);
                return Some(self.add_upvalue(func_idx, true, i as u16));
            }
        }
        if let Some(uv_idx) = self.resolve_upvalue_at(name, func_idx - 1) {
            return Some(self.add_upvalue(func_idx, false, uv_idx));
        }
        None
    }

    /// Whether `name` resolves to a handler clause's continuation variable,
    /// searching scopes in the same order as `resolve_local` and
    /// `resolve_upvalue`.
    fn is_continuation(&self, name: &str) -> bool {
        self.func_stack
            .iter()
            .rev()
            .find_map(|ctx| ctx.locals.iter().rev().find(|local| local.name == name))
            .is_some_and(|local| local.continuation)
    }

    fn add_continuation_local(&mut self, name: String) -> u16 {
        let slot = self.add_local(name);
        self.ctx_mut().locals[slot as usize].continuation = true;
        slot
    }

    fn add_upvalue(&mut self, func_idx: usize, is_local: bool, index: u16) -> u16 {
        let ctx = &mut self.func_stack[func_idx];
        for (i, uv) in ctx.upvalues.iter().enumerate() {
            if uv.is_local == is_local && uv.index == index {
                return i as u16;
            }
        }
        let idx = ctx.upvalues.len() as u16;
        ctx.upvalues.push(UpvalueDesc { is_local, index });
        idx
    }

    fn emit_get_local(&mut self, slot: u16) {
        match slot {
            0 => self.emit(Op::GetLocal0),
            1 => self.emit(Op::GetLocal1),
            2 => self.emit(Op::GetLocal2),
            3 => self.emit(Op::GetLocal3),
            _ => {
                self.emit(Op::GetLocal);
                self.emit_u16(slot);
            }
        }
    }

    fn emit_set_local(&mut self, slot: u16) {
        match slot {
            0 => self.emit(Op::SetLocal0),
            1 => self.emit(Op::SetLocal1),
            2 => self.emit(Op::SetLocal2),
            3 => self.emit(Op::SetLocal3),
            _ => {
                self.emit(Op::SetLocal);
                self.emit_u16(slot);
            }
        }
    }

    /// Push field `idx` of the tuple or data value in local `slot`: one
    /// `GetLocalField0`/`GetLocalField1` for the first two fields.
    fn emit_local_field(&mut self, slot: u16, idx: usize) -> Result<(), CompileError> {
        match idx {
            0 | 1 => {
                self.emit(if idx == 0 {
                    Op::GetLocalField0
                } else {
                    Op::GetLocalField1
                });
                self.emit_u16(slot);
                Ok(())
            }
            _ => {
                self.emit_get_local(slot);
                self.emit_get_field(idx)
            }
        }
    }

    fn emit_get_field(&mut self, idx: usize) -> Result<(), CompileError> {
        match idx {
            0 => self.emit(Op::GetField0),
            1 => self.emit(Op::GetField1),
            _ => {
                let idx = checked_u8(idx, "tuple fields")?;
                self.emit(Op::GetField);
                self.emit_u8(idx);
            }
        }
        Ok(())
    }

    fn emit_make_tuple(&mut self, len: usize) -> Result<(), CompileError> {
        let len = checked_u8(len, "tuple elements")?;
        self.emit(Op::MakeTuple);
        self.emit_u8(len);
        Ok(())
    }

    fn emit_get_var(&mut self, name: &str) -> Result<(), CompileError> {
        if let Some(slot) = self.resolve_local(name) {
            self.emit_get_local(slot);
        } else if let Some(idx) = self.resolve_upvalue(name) {
            self.emit(Op::GetUpvalue);
            self.emit_u16(idx);
        } else {
            let slot = self.global_slot(name)?;
            if self.func_stack.len() > 1 {
                self.pinned_global_slots.insert(slot);
            }
            self.emit(Op::GetGlobal);
            self.emit_u16(slot);
        }
        Ok(())
    }

    /// VM global name for the top-level binding of `name` in scope; a name
    /// with no top-level binding is a builtin.
    fn global_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.global_bindings.get(name).map_or(name, String::as_str)
    }

    /// Start a new top-level binding of `name`: later references resolve to
    /// it, earlier code keeps the binding it was compiled against.
    fn declare_global(&mut self, name: &str) {
        let extensions = &self.loader_options.extensions;
        let next = self
            .global_versions
            .entry(name.to_string())
            .or_insert_with(|| {
                usize::from(
                    hiko_builtin_meta::builtin_meta(name).is_some() || extensions.has_builtin(name),
                )
            });
        let version = *next;
        *next += 1;
        let global = if version > 0 {
            format!("{name}${version}")
        } else {
            name.to_string()
        };
        if let Some(previous) = self
            .global_bindings
            .insert(name.to_string(), global.clone())
            && let Some(&slot) = self.global_slots.get(&previous)
            && !self.pinned_global_slots.contains(&slot)
        {
            // Main-chunk reads have consumed their values before this store.
            // Only deferred reads in functions/closures need historical roots.
            self.global_slots.remove(&previous);
            self.global_names[usize::from(slot)] = Arc::from(global.as_str());
            self.global_slots.insert(global, slot);
        }
    }

    fn emit_set_global(&mut self, name: &str) -> Result<(), CompileError> {
        self.declare_global(name);
        self.emit_set_declared_global(name)
    }

    /// Store into the binding of `name` that `declare_global` started.
    fn emit_set_declared_global(&mut self, name: &str) -> Result<(), CompileError> {
        // The global may no longer hold the function recorded for direct calls.
        self.global_protos.remove(name);
        let slot = self.global_slot(name)?;
        self.emit(Op::SetGlobal);
        self.emit_u16(slot);
        Ok(())
    }

    /// Slot of the global that `name` refers to here, assigned on first use.
    fn global_slot(&mut self, name: &str) -> Result<u16, CompileError> {
        let global = self.global_name(name);
        if let Some(&slot) = self.global_slots.get(global) {
            return Ok(slot);
        }
        let slot = checked_u16(self.global_names.len(), "globals")?;
        let global = global.to_string();
        self.global_names.push(Arc::from(global.as_str()));
        self.global_slots.insert(global, slot);
        Ok(slot)
    }

    /// Proto index for a `CallDirect` to `name`: only when `name` resolves to
    /// a global that is currently bound to a capture-free function.
    fn direct_call_proto(&mut self, name: &str) -> Option<u16> {
        if self.resolve_local(name).is_some() || self.resolve_upvalue(name).is_some() {
            return None;
        }
        self.global_protos.get(name).copied()
    }

    /// Flat proto for a direct call of `name` with a written-out tuple of
    /// `width` components, while `name` is still bound to that function.
    fn direct_flat_proto(&mut self, name: &str, width: usize) -> Option<u16> {
        let entry = *self.global_flat_protos.get(name)?;
        (entry.width == width && self.direct_call_proto(name) == Some(entry.wrapper))
            .then_some(entry.flat)
    }

    fn forget_global_proto(&mut self, name: &str) {
        if self.ctx().scope_depth == 0 {
            self.global_protos.remove(name);
        }
    }

    /// After binding `name` to the closure of the function finished last,
    /// record it for direct calls when it is a capture-free global.
    fn register_global_proto(&mut self, name: &str) {
        if self.ctx().scope_depth != 0 {
            return;
        }
        let Some(proto) = self.functions.last() else {
            return;
        };
        if proto.n_captures == 0 {
            // finish_function already checked that the index fits in u16.
            let proto_idx = (self.functions.len() - 1) as u16;
            self.global_protos.insert(name.to_string(), proto_idx);
        }
    }

    /// Add a local in the current scope and return its slot. Slots above
    /// u16::MAX are rejected when the function is finished (max_locals).
    fn add_local(&mut self, name: String) -> u16 {
        let ctx = self.ctx_mut();
        let depth = ctx.scope_depth;
        ctx.locals.push(Local {
            name,
            depth,
            used: Cell::new(false),
            continuation: false,
        });
        ctx.max_locals = ctx.max_locals.max(ctx.locals.len());
        (ctx.locals.len() - 1) as u16
    }

    fn begin_scope(&mut self) {
        self.ctx_mut().scope_depth += 1;
    }

    fn end_scope_keep_result(&mut self) {
        let depth = self.ctx().scope_depth;
        let mut n = 0;
        while self.ctx().locals.last().is_some_and(|l| l.depth == depth) {
            self.ctx_mut().locals.pop();
            n += 1;
        }
        self.ctx_mut().scope_depth -= 1;
        if n > 0 {
            let base = self.ctx().locals.len();
            self.emit_set_local(base as u16);
            for _ in 0..n - 1 {
                self.emit(Op::Pop);
            }
        }
    }

    fn bind_name(&mut self, name: &str) -> Result<(), CompileError> {
        if self.ctx().scope_depth == 0 {
            self.emit_set_global(name)?;
        } else {
            self.add_local(name.to_string());
        }
        Ok(())
    }

    /// Emit GetLocal slot + GetField idx, add as temp local, return new slot.
    fn emit_field_extract(&mut self, slot: u16, idx: usize) -> Result<u16, CompileError> {
        self.emit_local_field(slot, idx)?;
        Ok(self.add_local("_tmp".to_string()))
    }

    /// Emit tag check: JumpIfLocalNotTag slot tag → fail.
    fn emit_tag_check(&mut self, slot: u16, tag: u16, fail_jumps: &mut Vec<(usize, usize)>) {
        let jmp = self.chunk().emit_local_tag_jump(slot, tag);
        fail_jumps.push((jmp, self.ctx().locals.len()));
    }

    /// Emit scalar comparison: GetLocal slot, push value, JumpIfNotEq → fail.
    fn emit_scalar_check(
        &mut self,
        slot: u16,
        value: Constant,
        fail_jumps: &mut Vec<(usize, usize)>,
    ) -> Result<(), CompileError> {
        self.emit_get_local(slot);
        self.emit_constant(value)?;
        let jmp = self.emit_jump(Op::JumpIfNotEq);
        fail_jumps.push((jmp, self.ctx().locals.len()));
        Ok(())
    }

    /// Compile `cond` and a jump taken when it is false. A comparison of two
    /// operands becomes one compare-and-branch instruction.
    fn compile_cond_jump(&mut self, cond: &Expr) -> Result<usize, CompileError> {
        let cond = strip_parens(cond);
        if let ExprKind::BinOp(op, lhs, rhs) = &cond.kind
            && !matches!(op, BinOp::Andalso | BinOp::Orelse | BinOp::Pipe)
        {
            let resolved = self
                .resolve_generic_binop(*op, cond.span)
                .unwrap_or_else(|| binop_to_op(*op));
            let jump = match resolved {
                Op::Eq => Some(Op::JumpIfNotEq),
                Op::Ne => Some(Op::JumpIfEq),
                Op::LtInt => Some(Op::JumpIfNotLtInt),
                Op::GtInt => Some(Op::JumpIfNotGtInt),
                Op::LeInt => Some(Op::JumpIfNotLeInt),
                Op::GeInt => Some(Op::JumpIfNotGeInt),
                _ => None,
            };
            if let Some(jump) = jump {
                self.compile_operands(&[lhs, rhs])?;
                return Ok(self.emit_jump(jump));
            }
        }
        self.compile_expr(cond)?;
        Ok(self.emit_jump(Op::JumpIfFalse))
    }

    // ── Declarations ─────────────────────────────────────────────────

    fn compile_decl(&mut self, decl: &Decl) -> Result<(), CompileError> {
        match &decl.kind {
            DeclKind::Val(pat, expr) => {
                self.compile_expr(expr)?;
                self.compile_binding_pattern(pat)
            }
            DeclKind::ValRec(sym, expr) => {
                let name = self.interner.resolve(*sym).to_string();
                if self.ctx().scope_depth > 0
                    && let ExprKind::Fn(pat, body) = &expr.kind
                {
                    return self.compile_local_rec_group(vec![name.clone()], |c, _| {
                        c.compile_lambda_proto(Some(name.clone()), pat, body)
                    });
                }
                self.forget_global_proto(&name);
                if self.ctx().scope_depth > 0 {
                    self.compile_expr(expr)?;
                    return self.bind_name(&name);
                }
                // Recursive: the body refers to the new binding.
                self.declare_global(&name);
                self.compile_expr(expr)?;
                self.emit_set_declared_global(&name)
            }
            DeclKind::Fun(bindings) if self.ctx().scope_depth > 0 => {
                let names = bindings
                    .iter()
                    .map(|b| self.interner.resolve(b.name).to_string())
                    .collect();
                self.compile_local_rec_group(names, |c, i| c.compile_fun_binding(&bindings[i]))
            }
            DeclKind::Fun(bindings) => {
                // `fun` is recursive: inside the bodies, these names refer to
                // the new bindings, never to an earlier global function.
                // Top-level functions capture nothing, so their protos are
                // reserved first and calls within the group are direct.
                let mut members = Vec::with_capacity(bindings.len());
                for binding in bindings {
                    let name = self.interner.resolve(binding.name).to_string();
                    self.declare_global(&name);
                    let proto_idx = self.reserve_proto()?;
                    self.global_protos.insert(name.clone(), proto_idx);
                    let flat = match flat_tuple_width(binding) {
                        Some(width) => {
                            let flat = self.reserve_proto()?;
                            let entry = FlatEntry {
                                wrapper: proto_idx,
                                flat,
                                width,
                            };
                            self.global_flat_protos.insert(name.clone(), entry);
                            Some(entry)
                        }
                        None => None,
                    };
                    members.push((name, proto_idx, flat));
                }
                for (binding, (name, proto_idx, flat)) in bindings.iter().zip(&members) {
                    let closure = match flat {
                        Some(entry) => {
                            self.pending_reserved_proto = Some(entry.flat);
                            let flat_fn = self.compile_flat_fun(name, entry.width, binding)?;
                            check_top_level_closure(name, &flat_fn, entry.flat)?;
                            self.pending_reserved_proto = Some(*proto_idx);
                            self.compile_flat_wrapper(name, *entry)?
                        }
                        None => {
                            self.pending_reserved_proto = Some(*proto_idx);
                            self.compile_fun_binding(binding)?
                        }
                    };
                    check_top_level_closure(name, &closure, *proto_idx)?;
                    self.emit_make_closure(&closure);
                    self.emit_set_declared_global(name)?;
                    // Setting the global forgets the direct-call target; the
                    // global now holds exactly this function.
                    self.global_protos.insert(name.clone(), *proto_idx);
                }
                Ok(())
            }
            DeclKind::Datatype(dt) => self.compile_datatype(dt),
            DeclKind::TypeAlias(_) => Ok(()),
            DeclKind::AbstractType(_) => Ok(()),
            DeclKind::ExportVal {
                public_name,
                internal_name,
                ..
            } => {
                self.compile_expr(&Expr {
                    kind: ExprKind::Var(*internal_name),
                    span: decl.span,
                })?;
                let internal = self.interner.resolve(*internal_name).to_string();
                let internal_proto = self.direct_call_proto(&internal);
                let name = self.interner.resolve(*public_name).to_string();
                self.bind_name(&name)?;
                if self.ctx().scope_depth == 0
                    && let Some(proto_idx) = internal_proto
                {
                    self.global_protos.insert(name, proto_idx);
                }
                Ok(())
            }
            DeclKind::Effect(sym, _) => {
                let name = self.interner.resolve(*sym).to_string();
                let tag = self.next_effect_tag;
                self.next_effect_tag += 1;
                self.effect_tags.insert(name, tag);
                Ok(())
            }
            DeclKind::Import(name) => self.compile_named_import(*name),
            DeclKind::ImportWithNames(name, names) => {
                self.compile_named_import_with_names(*name, names)
            }
            DeclKind::Use(path) => self.compile_use(path),
            DeclKind::Signature(_) => {
                unreachable!("signatures must be removed before codegen")
            }
            DeclKind::Structure { .. } => {
                unreachable!("structures must be flattened before codegen")
            }
            DeclKind::Local(locals, body) => {
                self.begin_scope();
                for d in locals {
                    self.compile_decl(d)?;
                }
                let private_depth = self.ctx().scope_depth;
                for d in body {
                    self.ctx_mut().scope_depth -= 1;
                    self.compile_decl(d)?;
                    self.ctx_mut().scope_depth += 1;
                }
                while self
                    .ctx()
                    .locals
                    .last()
                    .is_some_and(|l| l.depth == private_depth)
                {
                    self.ctx_mut().locals.pop();
                    self.emit(Op::Pop);
                }
                self.ctx_mut().scope_depth -= 1;
                Ok(())
            }
        }
    }

    // ── Local and named import handling ─────────────────────────────

    fn resolve_local_use(&self, path: &str) -> Result<ResolvedImport, CompileError> {
        if let Some((requires, importer)) = self.importing_extension {
            return Err(CompileError::codegen(format!(
                "module {importer} of extension '{}' declares use '{path}'; an extension module cannot use a file",
                requires.extension
            )));
        }
        if let Some(files) = self.loader_options.embedded.and_then(|e| e.files) {
            let key = embedded_file_path(&self.base_dir, path)?;
            let file = files.iter().find(|f| f.path == key).ok_or_else(|| {
                CompileError::codegen(format!(
                    "local use '{path}' ({key}) is not embedded in this artifact"
                ))
            })?;
            return Ok(ResolvedImport::Embedded {
                key: ImportKey::EmbeddedFile(file.path),
                display_name: path.to_string(),
                source: file.source,
                base_dir: Path::new(file.path)
                    .parent()
                    .unwrap_or(Path::new(""))
                    .to_path_buf(),
                module: false,
            });
        }
        let resolved = self.base_dir.join(path);
        let canonical = std::fs::canonicalize(&resolved).map_err(|e| {
            CompileError::codegen(format!(
                "cannot resolve local use '{}': {e}",
                resolved.display()
            ))
        })?;
        Ok(ResolvedImport::File {
            canonical,
            display_name: path.to_string(),
        })
    }

    fn resolve_named_import(&mut self, module_name: &str) -> Result<ResolvedImport, CompileError> {
        let (package_name, leaf_module) = split_import_name(module_name)?;
        if let Some((requires, importer)) = self.importing_extension
            && self.loader_options.extensions.module(module_name).is_none()
        {
            self.check_extension_import(&requires, importer, module_name)?;
        }
        if let Some((import_name, source)) = self.loader_options.extensions.module(module_name) {
            return Ok(ResolvedImport::Synthetic {
                import_name,
                display_name: module_name.to_string(),
                source,
                base_dir: self.base_dir.clone(),
            });
        }
        if is_internal_builtin_package(package_name) {
            return self.resolve_internal_builtin_import(module_name, leaf_module);
        }
        if let Some(embedded) = self.loader_options.embedded {
            return resolve_embedded_module(embedded, module_name, self.importing_extension);
        }
        self.resolve_external_named_import(module_name, package_name, leaf_module)
    }

    /// `import`, a library module that the extension module `importer`
    /// imports, is listed in `requires` and available: embedded in a sealed
    /// artifact, or pinned in the project's lockfile. The error names the
    /// extension rather than an import the user never wrote.
    fn check_extension_import(
        &mut self,
        requires: &ExtensionRequires,
        importer: &str,
        import: &str,
    ) -> Result<(), CompileError> {
        let who = extension_module(requires, importer);
        if !requires.imports.contains(&import) {
            return Err(CompileError::codegen(format!(
                "{who} imports {import}, which the extension does not list in its requires; add it to requires in the extension's Extension"
            )));
        }
        // Part of the compiler: not in a lockfile and not embedded.
        if is_internal_builtin_package(split_import_name(import)?.0) {
            return Ok(());
        }
        match self.loader_options.embedded {
            Some(embedded) if embedded.modules.iter().any(|m| m.import_name == import) => Ok(()),
            Some(_) => Err(CompileError::codegen(format!(
                "{who} needs {import}, which this artifact does not embed; build it with --lock and a lockfile that pins {import}"
            ))),
            None => self.check_locked_for_extension(&who, import),
        }
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn check_locked_for_extension(&mut self, who: &str, import: &str) -> Result<(), CompileError> {
        let (package, leaf) = split_import_name(import)?;
        let entry = self.entry_file.as_deref();
        if !entry.is_some_and(|entry| find_project_manifest_path(entry).is_some()) {
            let script = entry.map_or("this program".to_string(), |e| e.display().to_string());
            return Err(CompileError::codegen(format!(
                "{who} needs {import}, which comes from a project's lockfile, but {script} has no hiko.toml in or above its directory; create one that declares {package} {PACKAGE_ENTRY_AND_INSTALL}"
            )));
        }
        let (lockfile, lockfile_path, _) = self
            .ensure_import_loader(import, Some(package))
            .map_err(|e| {
                CompileError::codegen(format!("{who} needs {import}: {}", codegen_message(e)))
            })?;
        if lockfile
            .packages
            .get(package)
            .is_some_and(|p| p.modules.contains_key(leaf))
        {
            Ok(())
        } else {
            Err(CompileError::codegen(format!(
                "{who} needs {import}, which {} does not pin; {}",
                lockfile_path.display(),
                declare_package_fix(package)
            )))
        }
    }

    #[cfg(not(feature = "loader-integrity-blake3"))]
    fn check_locked_for_extension(&mut self, who: &str, import: &str) -> Result<(), CompileError> {
        Err(CompileError::codegen(format!(
            "{who} needs {import}, but this build cannot load library modules; enable cargo feature 'loader-integrity-blake3'"
        )))
    }

    /// Remote imports need the verified loader; `loader-integrity-blake3`
    /// enables `loader-http` and `loader-named-imports` too.
    #[cfg(not(feature = "loader-integrity-blake3"))]
    fn resolve_external_named_import(
        &mut self,
        module_name: &str,
        _package_name: &str,
        _leaf_module: &str,
    ) -> Result<ResolvedImport, CompileError> {
        Err(CompileError::codegen(format!(
            "named remote import '{module_name}' is not available in this build; enable cargo feature 'loader-integrity-blake3'"
        )))
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn resolve_external_named_import(
        &mut self,
        module_name: &str,
        package_name: &str,
        leaf_module: &str,
    ) -> Result<ResolvedImport, CompileError> {
        let local_package_root = self.loader_options.local_package_root.clone();
        let extension = self.importing_extension;
        let load_failed =
            |fix: &'static str| move |e| in_extension_module(extension, module_name, fix, e);
        let (lockfile, lockfile_path, cache_dir) = self.ensure_import_loader(module_name, None)?;
        let package = lockfile.packages.get(package_name).ok_or_else(|| {
            CompileError::codegen(format!(
                "package '{package_name}' for named import '{module_name}' not found in {}; {}",
                lockfile_path.display(),
                declare_package_fix(package_name)
            ))
        })?;
        let expected_hash = package.modules.get(leaf_module).ok_or_else(|| {
            CompileError::codegen(format!(
                "module '{leaf_module}' in package '{package_name}' for named import '{module_name}' not found in {}; {}",
                lockfile_path.display(),
                declare_package_fix(package_name)
            ))
        })?;
        let expected_hash = normalize_blake3(expected_hash);

        if let Some(root) = &local_package_root {
            let local_path = root
                .join(format!("{package_name}-v{}", package.version))
                .join("modules")
                .join(format!("{leaf_module}.hml"));
            if local_path.is_file() {
                let source = std::fs::read_to_string(&local_path)
                    .map_err(|e| {
                        CompileError::codegen(format!(
                            "cannot read local module '{}': {e}",
                            local_path.display()
                        ))
                    })
                    .map_err(load_failed(REPAIR_MODULE_FILE))?;
                verify_blake3(
                    source.as_bytes(),
                    &expected_hash,
                    &format!(
                        "local named import '{module_name}' ({})",
                        local_path.display()
                    ),
                )
                .map_err(load_failed(HASH_DIFFERS_FROM_PIN))?;
                let canonical = std::fs::canonicalize(&local_path).unwrap_or(local_path);
                return Ok(ResolvedImport::VerifiedFile {
                    canonical,
                    display_name: module_name.to_string(),
                    source,
                });
            }
        }

        let module_url = format!(
            "{}/modules/{leaf_module}.hml",
            package.base_url.trim_end_matches('/')
        );
        let cache_path = module_cache_path(cache_dir, &module_url, &expected_hash);

        if cache_path.exists() {
            let cached = std::fs::read_to_string(&cache_path)
                .map_err(|e| {
                    CompileError::codegen(format!(
                        "cannot read cached module '{}': {e}",
                        cache_path.display()
                    ))
                })
                .map_err(load_failed(REPAIR_MODULE_FILE))?;
            if verify_blake3(
                cached.as_bytes(),
                &expected_hash,
                &format!("cached named import '{module_name}'"),
            )
            .is_ok()
            {
                return Ok(ResolvedImport::VerifiedFile {
                    canonical: cache_path,
                    display_name: module_name.to_string(),
                    source: cached,
                });
            }
            let _ = std::fs::remove_file(&cache_path);
        }

        let source = fetch_remote_module(&module_url, module_name).map_err(load_failed(
            "check that the registry in hiko.toml is reachable",
        ))?;
        verify_blake3(
            source.as_bytes(),
            &expected_hash,
            &format!("named import '{module_name}'"),
        )
        .map_err(load_failed(HASH_DIFFERS_FROM_PIN))?;
        let cache_path = store_cached_module(cache_dir, &module_url, &expected_hash, &source)
            .map_err(load_failed(REPAIR_MODULE_CACHE))?;
        Ok(ResolvedImport::VerifiedFile {
            canonical: cache_path,
            display_name: module_name.to_string(),
            source,
        })
    }

    fn resolve_internal_builtin_import(
        &self,
        module_name: &str,
        leaf_module: &str,
    ) -> Result<ResolvedImport, CompileError> {
        let module = internal_builtin_module(leaf_module).ok_or_else(|| {
            CompileError::codegen(format!("unknown internal builtin module '{module_name}'"))
        })?;
        if !module.enabled {
            return Err(CompileError::codegen(format!(
                "internal builtin module '{}' is not available in this build of hiko; enable cargo feature '{}'",
                module.import_name, module.feature_name
            )));
        }
        Ok(ResolvedImport::Synthetic {
            import_name: module.import_name,
            display_name: module_name.to_string(),
            source: module.source,
            base_dir: self.base_dir.clone(),
        })
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn ensure_import_loader(
        &mut self,
        module_name: &str,
        needed_package: Option<&str>,
    ) -> Result<(&ImportLockfile, &PathBuf, &PathBuf), CompileError> {
        if self.import_lockfile.is_none() || self.module_cache_dir.is_none() {
            let entry_file = self.entry_file.as_ref().ok_or_else(|| {
                CompileError::codegen(format!(
                    "named import '{module_name}' requires file-based compilation context"
                ))
            })?;
            let loader_config = load_import_loader_config(entry_file, needed_package)?;
            let lockfile_path = &loader_config.lockfile_path;
            let lockfile_text = std::fs::read_to_string(lockfile_path).map_err(|e| {
                CompileError::codegen(format!(
                    "cannot read module lockfile '{}': {e}",
                    lockfile_path.display()
                ))
            })?;
            let lockfile: ImportLockfile = toml::from_str(&lockfile_text).map_err(|e| {
                CompileError::codegen(format!(
                    "invalid module lockfile '{}': {}",
                    lockfile_path.display(),
                    toml_error_text(&e, &lockfile_text)
                ))
            })?;
            validate_lockfile(&lockfile, lockfile_path)?;
            validate_lockfile_against_manifest(
                &lockfile,
                lockfile_path,
                &loader_config,
                needed_package,
            )?;
            self.import_lockfile = Some(lockfile);
            self.import_lockfile_path = Some(loader_config.lockfile_path.clone());
            // The embedder's choice, then the manifest's, then the default.
            self.module_cache_dir = Some(match &self.loader_options.cache_dir {
                Some(dir) => dir.clone(),
                None => project_module_cache_dir(
                    &loader_config.manifest_path,
                    loader_config.manifest.defaults.lib_cache.as_deref(),
                )?,
            });
        }
        let lockfile = self.import_lockfile.as_ref().unwrap();
        let lockfile_path = self.import_lockfile_path.as_ref().unwrap();
        let cache_dir = self.module_cache_dir.as_ref().unwrap();
        Ok((lockfile, lockfile_path, cache_dir))
    }

    /// Pass 1: type-check a single declaration (handles `use` recursively)
    fn infer_decl_pass(&mut self, decl: &Decl) -> Result<(), CompileError> {
        match &decl.kind {
            DeclKind::Import(name) => self.infer_named_import(*name),
            DeclKind::ImportWithNames(name, names) => {
                self.infer_named_import_with_names(*name, names)
            }
            DeclKind::Use(path) => self.infer_use(path),
            DeclKind::Signature(_) => {
                unreachable!("signatures must be removed before inference pass")
            }
            DeclKind::Structure { .. } => {
                unreachable!("structures must be flattened before inference pass")
            }
            DeclKind::AbstractType(_) | DeclKind::ExportVal { .. } => {
                self.infer_ctx.infer_decl(decl)?;
                Ok(())
            }
            DeclKind::Local(locals, body) => {
                // One declaration for overloaded operators, like `local` in
                // the type checker.
                self.infer_ctx.begin_declaration();
                let result = locals
                    .iter()
                    .chain(body)
                    .try_for_each(|d| self.infer_decl_pass(d));
                let ended = self.infer_ctx.end_declaration(result.is_ok());
                result?;
                Ok(ended?)
            }
            _ => {
                self.infer_ctx.infer_decl(decl)?;
                Ok(())
            }
        }
    }

    /// The extension whose module `key` is, with that module's name.
    fn extension_of(&self, key: &ImportKey) -> Option<(ExtensionRequires, &'static str)> {
        match key {
            ImportKey::Synthetic(name) => self
                .loader_options
                .extensions
                .requires_for(name)
                .map(|requires| (requires, *name)),
            _ => None,
        }
    }

    /// Pass 1 for imported source: load, parse, desugar, infer, and cache.
    fn infer_import_source(&mut self, import: ResolvedImport) -> Result<(), CompileError> {
        let key = import.key();
        let display_name = import.display_name().to_string();
        let is_remote = import.is_remote();
        if self.loading_imports.contains(&key) {
            return Err(CompileError::codegen(format!(
                "circular import detected while loading '{display_name}'"
            )));
        }

        let already_inferred = self.inferred_imports.contains(&key);
        if already_inferred && !is_remote {
            return Ok(());
        }

        let (source, imported_base_dir) = import.load_source()?;
        let tokens = Lexer::new(&source, 0).tokenize().map_err(|e| {
            CompileError::codegen(format!("lex error in '{display_name}': {}", e.message))
        })?;
        // Check every pinned source before parsing or reusing inference. A
        // token scan reaches nested declarations even when inference does not.
        if is_remote && let Some(path) = first_use_path(&tokens) {
            return Err(remote_use_refused(path));
        }
        if already_inferred {
            return Ok(());
        }

        // Use the same interner for import parsing so symbols are shared
        let import_interner = std::mem::take(&mut self.interner);
        let program = Parser::with_interner(tokens, import_interner)
            .parse_program()
            .map_err(|e| {
                CompileError::codegen(format!("parse error in '{display_name}': {}", e.message))
            })?;

        let program = hiko_syntax::desugar::desugar_program(program);
        let program = hiko_syntax::constfold::fold_program(program);
        self.interner = program.interner;
        self.infer_ctx.interner = self.interner.clone();
        let desugared = program.decls;

        self.loading_imports.insert(key.clone());
        let old_base = self.base_dir.clone();
        self.base_dir = imported_base_dir.clone();
        let old_extension = self.importing_extension;
        // None inside a library module: its own imports are not the
        // extension's.
        self.importing_extension = self.extension_of(&key);

        // The module's declarations are top-level ones of their own, even
        // when a `use` inside `local` brings it in.
        let importer = self.infer_ctx.suspend_declaration();
        let result = desugared.iter().try_for_each(|d| self.infer_decl_pass(d));
        self.infer_ctx.resume_declaration(importer);

        self.importing_extension = old_extension;
        self.base_dir = old_base;
        self.loading_imports.remove(&key);

        result?;
        self.inferred_imports.insert(key.clone());
        self.imported_programs.insert(
            key,
            CachedImportedProgram {
                base_dir: imported_base_dir,
                decls: desugared,
            },
        );
        Ok(())
    }

    fn infer_use(&mut self, path: &str) -> Result<(), CompileError> {
        let resolved = self.resolve_local_use(path)?;
        self.infer_import_source(resolved)
    }

    fn infer_named_import(&mut self, module_name: Symbol) -> Result<(), CompileError> {
        let module_name = self.interner.resolve(module_name).to_string();
        let resolved = self.resolve_named_import(&module_name)?;
        self.infer_import_source(resolved)
    }

    fn infer_named_import_with_names(
        &mut self,
        module_name: Symbol,
        names: &[Symbol],
    ) -> Result<(), CompileError> {
        let module_name_str = self.interner.resolve(module_name).to_string();

        // First, import and infer the module normally
        let resolved = self.resolve_named_import(&module_name_str)?;
        self.infer_import_source(resolved)?;

        // Now bind aliases for each exposed name. Exposed names are explicit
        // imports, so collisions are compile errors rather than silent shadowing.
        let module_short = module_name_str
            .rsplit('.')
            .next()
            .unwrap_or(&module_name_str);
        let mut seen_exposed = HashSet::new();
        for sym in names {
            let name = self.interner.resolve(*sym).to_string();
            if !seen_exposed.insert(name.clone()) {
                return Err(CompileError::codegen(format!(
                    "duplicate exposed import name '{}' in import '{}'",
                    name, module_name_str
                )));
            }
            if self.infer_ctx.has_constructor(&name) || self.infer_ctx.has_value(&name) {
                return Err(CompileError::codegen(format!(
                    "exposed import name '{}' from '{}' conflicts with an existing binding; use the qualified name instead",
                    name, module_name_str
                )));
            }
            let qualified = format!("{}.{}", module_short, name);

            // Try constructor first, then value
            if self.infer_ctx.has_constructor(&qualified) {
                self.infer_ctx
                    .bind_constructor_alias(&name, &qualified)
                    .map_err(CompileError::codegen)?;
                // Also register in the compiler's constructor tag/arity maps
                // so codegen can resolve the aliased name.
                // Use infer_ctx's tags since compile_datatype hasn't run yet.
                if let Some(tag) = self.infer_ctx.constructor_tag(&qualified) {
                    self.constructor_tags.insert(name.clone(), tag);
                }
                // Arity: 1 if the constructor has a payload, 0 otherwise
                if let Some(scheme) = self.infer_ctx.constructor_scheme(&qualified) {
                    let arity = matches!(&scheme.ty, Type::Arrow(..)) as u8;
                    self.constructor_arities.insert(name.clone(), arity);
                }
            } else if self.infer_ctx.has_value(&qualified) {
                self.infer_ctx
                    .bind_value_alias(&name, &qualified)
                    .map_err(CompileError::codegen)?;
            } else {
                return Err(CompileError::codegen(format!(
                    "import '{}' does not export '{}'",
                    module_name_str, name
                )));
            }
        }
        Ok(())
    }

    /// Pass 2 for imported source: compile from cached desugared AST.
    fn compile_import_source(&mut self, key: ImportKey) -> Result<(), CompileError> {
        if self.compiled_imports.contains(&key) {
            return Ok(());
        }

        let CachedImportedProgram { base_dir, decls } =
            self.imported_programs.remove(&key).ok_or_else(|| {
                CompileError::codegen(format!("import not found during compilation: {key:?}"))
            })?;

        let old_base = self.base_dir.clone();
        self.base_dir = base_dir;
        let old_extension = self.importing_extension;
        self.importing_extension = self.extension_of(&key);
        for decl in &decls {
            self.compile_decl(decl)?;
        }
        self.importing_extension = old_extension;
        self.base_dir = old_base;
        self.compiled_imports.insert(key);
        Ok(())
    }

    fn compile_use(&mut self, path: &str) -> Result<(), CompileError> {
        let resolved = self.resolve_local_use(path)?;
        self.compile_import_source(resolved.key())
    }

    fn compile_named_import(&mut self, module_name: Symbol) -> Result<(), CompileError> {
        let module_name = self.interner.resolve(module_name).to_string();
        let resolved = self.resolve_named_import(&module_name)?;
        self.compile_import_source(resolved.key())
    }

    fn compile_named_import_with_names(
        &mut self,
        module_name: Symbol,
        names: &[Symbol],
    ) -> Result<(), CompileError> {
        // First compile the module normally
        let module_name_str = self.interner.resolve(module_name).to_string();
        let resolved = self.resolve_named_import(&module_name_str)?;
        self.compile_import_source(resolved.key())?;

        // Now create global aliases for each exposed name
        let module_short = module_name_str
            .rsplit('.')
            .next()
            .unwrap_or(&module_name_str);
        for sym in names {
            let name = self.interner.resolve(*sym).to_string();
            let qualified = format!("{}.{}", module_short, name);
            // Emit a runtime global alias: Ok = Result.Ok, map = Result.map, etc.
            // Type inference already verified that the qualified export exists.
            self.emit_get_var(&qualified)?;
            self.emit_set_global(&name)?;
            // Preserve direct-call optimization for aliased functions.
            if let Some(&proto_idx) = self.global_protos.get(&qualified) {
                self.global_protos.insert(name, proto_idx);
            }
        }
        Ok(())
    }

    fn compile_binding_pattern(&mut self, pat: &Pat) -> Result<(), CompileError> {
        match &pat.kind {
            PatKind::Var(sym) => {
                let name = self.interner.resolve(*sym).to_string();
                self.bind_name(&name)?;
                Ok(())
            }
            PatKind::Wildcard => {
                self.emit(Op::Pop);
                Ok(())
            }
            PatKind::Tuple(pats) => {
                let tup_slot = self.add_local("_tup".to_string());
                for (i, p) in pats.iter().enumerate() {
                    self.emit_local_field(tup_slot, i)?;
                    self.compile_binding_pattern(p)?;
                }
                // At top level the fields went to globals, so the tuple is
                // back on top of the stack and nothing else will remove it.
                // In a local scope the fields are locals above it, and the
                // scope end removes them together.
                if self.ctx().scope_depth == 0 {
                    self.ctx_mut().locals.pop();
                    self.emit(Op::Pop);
                }
                Ok(())
            }
            PatKind::Paren(p) | PatKind::Ann(p, _) => self.compile_binding_pattern(p),
            _ => Err(CompileError::codegen(format!(
                "unsupported pattern in binding: {:?}",
                pat.kind
            ))),
        }
    }

    fn compile_datatype(&mut self, dt: &DatatypeDecl) -> Result<(), CompileError> {
        for (i, con) in dt.constructors.iter().enumerate() {
            let con_name = self.interner.resolve(con.name).to_string();
            let tag = checked_u16(i, "datatype constructors")?;
            let has_payload = con.payload.is_some();
            self.constructor_tags.insert(con_name.clone(), tag);
            self.constructor_arities
                .insert(con_name.clone(), u8::from(has_payload));

            if has_payload {
                self.push_new_function(Some(con_name.clone()));
                self.add_local("_arg".to_string());
                self.emit_get_local(0);
                self.emit(Op::MakeData);
                self.emit_u16(tag);
                self.emit_u8(1);
                self.emit(Op::Return);
                self.finish_function()?;
            } else {
                self.emit(Op::MakeData);
                self.emit_u16(tag);
                self.emit_u8(0);
            }
            self.bind_name(&con_name)?;
            if has_payload {
                self.register_global_proto(&con_name);
            }
        }
        Ok(())
    }

    // ── Functions ────────────────────────────────────────────────────

    /// Compile the function of one `fun` binding. The closure is not created
    /// yet; the caller emits it.
    fn compile_fun_binding(
        &mut self,
        binding: &FunBinding,
    ) -> Result<PendingClosure, CompileError> {
        let name = self.interner.resolve(binding.name).to_string();
        if binding.clauses.len() == 1 && binding.clauses[0].pats.iter().all(is_simple_pat) {
            let clause = &binding.clauses[0];
            return self.compile_curried_fn(Some(&name), &clause.pats, 0, &clause.body);
        }
        let arity = binding.clauses[0].pats.len();
        self.compile_clausal_fn_inner(Some(&name), arity, 0, &binding.clauses)
    }

    /// The flat entry of a function that takes one tuple of `width`
    /// components: one parameter slot per component, each clause matching
    /// its components against them.
    fn compile_flat_fun(
        &mut self,
        name: &str,
        width: usize,
        binding: &FunBinding,
    ) -> Result<PendingClosure, CompileError> {
        self.push_new_function(Some(name.to_string()));
        self.ctx_mut().arity = checked_u8(width, "tuple parameter components")?;
        for i in 0..width {
            self.add_local(format!("_arg{i}"));
        }
        self.begin_scope();
        let branches: Vec<_> = binding
            .clauses
            .iter()
            .map(|clause| {
                let components = tuple_components(&clause.pats[0])
                    .expect("flat_tuple_width checked every clause");
                let parts = (0u16..).zip(components.iter()).collect();
                (parts, &clause.body)
            })
            .collect();
        self.compile_multi_case_branches(&branches, true)?;
        self.end_scope_keep_result();
        self.emit(Op::Return);
        self.finish_function_proto()
    }

    /// The ordinary entry of a flattened function, for calls that pass a
    /// tuple value: take it apart and call the flat entry.
    fn compile_flat_wrapper(
        &mut self,
        name: &str,
        entry: FlatEntry,
    ) -> Result<PendingClosure, CompileError> {
        self.push_new_function(Some(name.to_string()));
        self.add_local("_arg".to_string());
        for i in 0..entry.width {
            self.emit_field_extract(0, i)?;
        }
        self.emit(Op::TailCallDirect);
        self.emit_u16(entry.flat);
        self.emit(Op::Return);
        self.finish_function_proto()
    }

    /// Compile a recursive group of local functions: `fun f ... and g ...`
    /// or `val rec f = fn ...` inside a `let`, `local` or function body.
    ///
    /// The names are bound to local slots before the bodies are compiled, so
    /// a body that refers to a member captures that slot. The closures do not
    /// exist when the slots are captured, so the slots are not read at run
    /// time: those captures become group references of one
    /// `MakeRecClosures` that creates all members and points them at each
    /// other. A group whose bodies never refer to a member uses plain
    /// `MakeClosure`, as before.
    fn compile_local_rec_group(
        &mut self,
        names: Vec<String>,
        mut compile_member: impl FnMut(&mut Self, usize) -> Result<PendingClosure, CompileError>,
    ) -> Result<(), CompileError> {
        let first_slot = self.ctx().locals.len();
        let n = names.len();
        for name in names {
            self.add_local(name);
        }
        let group = first_slot..first_slot + n;
        let mut members = Vec::with_capacity(n);
        for i in 0..n {
            self.pending_self_callable = true;
            members.push(compile_member(self, i)?);
            self.pending_self_callable = false;
        }

        let recursive = members.iter().any(|m| {
            m.upvalues
                .iter()
                .any(|uv| uv.is_local && group.contains(&(uv.index as usize)))
        });
        if !recursive {
            for member in &members {
                self.emit_make_closure(member);
            }
            return Ok(());
        }

        self.emit(Op::MakeRecClosures);
        self.emit_u8(checked_u8(n, "functions in a recursive group")?);
        for member in &members {
            self.emit_u16(member.proto_idx);
            self.emit_u8(member.n_captures);
            for uv in &member.upvalues {
                let index = uv.index as usize;
                if uv.is_local && group.contains(&index) {
                    self.emit_u8(REC_CAPTURE_GROUP);
                    self.emit_u16((index - first_slot) as u16);
                } else {
                    self.emit_u8(if uv.is_local {
                        REC_CAPTURE_LOCAL
                    } else {
                        REC_CAPTURE_UPVALUE
                    });
                    self.emit_u16(uv.index);
                }
            }
        }
        Ok(())
    }

    fn compile_clausal_fn_inner(
        &mut self,
        name: Option<&str>,
        arity: usize,
        arg_idx: usize,
        clauses: &[FunClause],
    ) -> Result<PendingClosure, CompileError> {
        let fn_name = if arg_idx == 0 {
            name.map(|s| s.to_string())
        } else {
            None
        };
        self.push_new_function(fn_name);
        self.add_local(format!("_arg{arg_idx}"));

        if arg_idx + 1 < arity {
            let inner = self.compile_clausal_fn_inner(None, arity, arg_idx + 1, clauses)?;
            self.emit_make_closure(&inner);
        } else {
            self.begin_scope();
            if arity == 1 {
                self.emit_get_var("_arg0")?;
                let scrut_slot = self.add_local("_scrut".to_string());
                let branches: Vec<_> = clauses.iter().map(|c| (&c.pats[0], &c.body)).collect();
                self.compile_case_branches_inner(scrut_slot, &branches, true)?;
            } else {
                for i in 0..arity {
                    self.emit_get_var(&format!("_arg{i}"))?;
                }
                self.emit_make_tuple(arity)?;
                let scrut_slot = self.add_local("_scrut".to_string());
                let tuple_pats: Vec<Pat> = clauses
                    .iter()
                    .map(|c| Pat {
                        kind: PatKind::Tuple(c.pats.clone()),
                        span: c.span,
                    })
                    .collect();
                let branches: Vec<_> = tuple_pats
                    .iter()
                    .zip(clauses.iter().map(|c| &c.body))
                    .collect();
                self.compile_case_branches_inner(scrut_slot, &branches, true)?;
            }
            self.end_scope_keep_result();
        }
        self.emit(Op::Return);
        self.finish_function_proto()
    }

    fn compile_curried_fn(
        &mut self,
        name: Option<&str>,
        pats: &[Pat],
        idx: usize,
        body: &Expr,
    ) -> Result<PendingClosure, CompileError> {
        let fn_name = if idx == 0 {
            name.map(|s| s.to_string())
        } else {
            None
        };
        self.push_new_function(fn_name);
        self.add_local(simple_pat_name(&pats[idx], &self.interner));
        if idx + 1 < pats.len() {
            let inner = self.compile_curried_fn(None, pats, idx + 1, body)?;
            self.emit_make_closure(&inner);
        } else {
            self.compile_expr_tail(body)?;
        }
        self.emit(Op::Return);
        self.finish_function_proto()
    }

    fn compile_lambda(&mut self, pat: &Pat, body: &Expr) -> Result<(), CompileError> {
        let closure = self.compile_lambda_proto(None, pat, body)?;
        self.emit_make_closure(&closure);
        Ok(())
    }

    fn compile_lambda_proto(
        &mut self,
        name: Option<String>,
        pat: &Pat,
        body: &Expr,
    ) -> Result<PendingClosure, CompileError> {
        self.push_new_function(name);
        if is_simple_pat(pat) {
            self.add_local(simple_pat_name(pat, &self.interner));
            self.compile_expr_tail(body)?;
        } else {
            self.add_local("_arg".to_string());
            self.begin_scope();
            self.emit_get_local(0);
            let scrut_slot = self.add_local("_scrut".to_string());
            self.compile_case_branches_inner(scrut_slot, &[(pat, body)], true)?;
            self.end_scope_keep_result();
        }
        self.emit(Op::Return);
        self.finish_function_proto()
    }

    fn finish_function(&mut self) -> Result<(), CompileError> {
        let closure = self.finish_function_proto()?;
        self.emit_make_closure(&closure);
        Ok(())
    }

    /// Pop the current function context and store its prototype. Its
    /// closure is created in the enclosing function by `emit_make_closure`
    /// or as part of a `MakeRecClosures` group.
    fn finish_function_proto(&mut self) -> Result<PendingClosure, CompileError> {
        let func_ctx = self.func_stack.pop().unwrap();
        let n_captures = checked_u8(func_ctx.upvalues.len(), "closure captures")?;
        let proto = FunctionProto {
            name: func_ctx.name,
            arity: func_ctx.arity,
            n_captures,
            n_locals: checked_u16(func_ctx.max_locals, "local slots")?,
            chunk: func_ctx.chunk,
        };
        let proto_idx = match func_ctx.reserved_proto {
            Some(idx) => {
                self.functions[idx as usize] = proto;
                idx
            }
            None => {
                let idx = checked_u16(self.functions.len(), "functions")?;
                self.functions.push(proto);
                idx
            }
        };
        Ok(PendingClosure {
            proto_idx,
            n_captures,
            upvalues: func_ctx.upvalues,
        })
    }

    fn emit_make_closure(&mut self, closure: &PendingClosure) {
        self.emit(Op::MakeClosure);
        self.emit_u16(closure.proto_idx);
        self.emit_u8(closure.n_captures);
        for uv in &closure.upvalues {
            self.emit_u8(u8::from(uv.is_local));
            self.emit_u16(uv.index);
        }
    }

    // ── Case / Pattern Matching ──────────────────────────────────────
    //
    // Two-pass approach:
    //   Pass 1 (test): check all conditions, emit JumpIfFalse to shared fail
    //   Pass 2 (bind): if all tests pass, extract and bind variables
    // On failure, NO locals have been pushed, so NO cleanup is needed.

    fn compile_case_branches_inner(
        &mut self,
        scrut_slot: u16,
        branches: &[(&Pat, &Expr)],
        tail: bool,
    ) -> Result<(), CompileError> {
        let branches: Vec<_> = branches
            .iter()
            .map(|&(pat, body)| (vec![(scrut_slot, pat)], body))
            .collect();
        self.compile_multi_case_branches(&branches, tail)
    }

    /// Match each branch's patterns against their slots, all of them, and
    /// run the first branch that matches. With one pattern per branch this
    /// is a `case`; a flattened tuple parameter (see `flat_tuple_width`)
    /// matches one component per parameter slot.
    fn compile_multi_case_branches(
        &mut self,
        branches: &[(Vec<(u16, &Pat)>, &Expr)],
        tail: bool,
    ) -> Result<(), CompileError> {
        let mut end_jumps = Vec::new();

        for (parts, body) in branches {
            let locals_before = self.ctx().locals.len();
            let depth_before = self.ctx().scope_depth;
            self.begin_scope();

            let mut fail_jumps: Vec<(usize, usize)> = Vec::new(); // (jump_pos, locals_at_fail)
            for &(slot, pat) in parts {
                self.compile_pattern_test(slot, pat, &mut fail_jumps)?;
            }
            for &(slot, pat) in parts {
                self.compile_pattern_bind(slot, pat)?;
            }

            self.compile_expr_inner(body, tail)?;
            self.end_scope_keep_result();
            end_jumps.push(self.emit_jump(Op::Jump));

            // Fail path: each fail point has a different number of
            // temporaries on the stack. Emit a separate cleanup
            // trampoline for each, jumping to a shared landing pad. Fail
            // points without temporaries jump straight to the landing pad;
            // they must not fall into a later trampoline's pops.
            let mut landing_jumps = Vec::new();
            for &(fj, locals_at_fail) in &fail_jumps {
                let n_pops = locals_at_fail - locals_before;
                if n_pops == 0 {
                    landing_jumps.push(fj);
                    continue;
                }
                self.patch_jump(fj)?;
                for _ in 0..n_pops {
                    self.emit(Op::Pop);
                }
                landing_jumps.push(self.emit_jump(Op::Jump));
            }
            for j in landing_jumps {
                self.patch_jump(j)?;
            }

            // Restore compiler state for next branch
            let ctx = self.ctx_mut();
            ctx.locals.truncate(locals_before);
            ctx.scope_depth = depth_before;
        }

        // Non-exhaustive match: emit runtime panic
        let msg_idx = self.add_string_constant("non-exhaustive match")?;
        self.emit(Op::Panic);
        self.emit_u16(msg_idx);

        for j in end_jumps {
            self.patch_jump(j)?;
        }
        Ok(())
    }

    /// Pass 1: emit tests only. No locals are pushed.
    /// Each failed test emits JumpIfFalse into fail_jumps.
    fn compile_pattern_test(
        &mut self,
        slot: u16,
        pat: &Pat,
        fail_jumps: &mut Vec<(usize, usize)>,
    ) -> Result<(), CompileError> {
        match &pat.kind {
            PatKind::Wildcard | PatKind::Var(_) | PatKind::Unit => {} // always match

            PatKind::IntLit(n) => {
                self.emit_scalar_check(slot, Constant::Int(*n), fail_jumps)?;
            }
            PatKind::FloatLit(f) => {
                self.emit_scalar_check(slot, Constant::Float(*f), fail_jumps)?;
            }
            PatKind::WordLit(w) => {
                self.emit_scalar_check(slot, Constant::Word(*w), fail_jumps)?;
            }
            PatKind::BoolLit(b) => {
                // The scrutinee is already a bool: branch on it directly
                // (negated for `false`) instead of a generic Eq.
                self.emit_get_local(slot);
                if !*b {
                    self.emit(Op::Not);
                }
                let jmp = self.emit_jump(Op::JumpIfFalse);
                fail_jumps.push((jmp, self.ctx().locals.len()));
            }
            PatKind::StringLit(s) => {
                self.emit_scalar_check(slot, Constant::String(s.clone()), fail_jumps)?;
            }
            PatKind::CharLit(c) => {
                self.emit_scalar_check(slot, Constant::Char(*c), fail_jumps)?;
            }

            PatKind::Constructor(sym, payload) => {
                let name = self.interner.resolve(*sym).to_string();
                let tag = *self
                    .constructor_tags
                    .get(&name)
                    .ok_or_else(|| CompileError::codegen(format!("unknown constructor: {name}")))?;
                self.emit_tag_check(slot, tag, fail_jumps);
                if let Some(sub_pat) = payload {
                    // Need to test sub-pattern against field 0
                    // Push temp to get a slot, then pop after testing
                    self.emit_field_extract_test(slot, 0, sub_pat, fail_jumps)?;
                }
            }

            PatKind::Tuple(pats) => {
                for (i, p) in pats.iter().enumerate() {
                    self.emit_field_extract_test(slot, i, p, fail_jumps)?;
                }
            }

            PatKind::Cons(hd, tl) => {
                self.emit_tag_check(slot, 1, fail_jumps); // Cons = tag 1
                self.emit_field_extract_test(slot, 0, hd, fail_jumps)?;
                self.emit_field_extract_test(slot, 1, tl, fail_jumps)?;
            }

            PatKind::List(pats) => {
                assert!(
                    pats.is_empty(),
                    "non-empty list pattern should be desugared to Cons"
                );
                self.emit_tag_check(slot, 0, fail_jumps); // Nil = tag 0
            }

            PatKind::As(_, sub_pat) => {
                self.compile_pattern_test(slot, sub_pat, fail_jumps)?;
            }
            PatKind::Paren(p) | PatKind::Ann(p, _) => {
                self.compile_pattern_test(slot, p, fail_jumps)?;
            }
        }
        Ok(())
    }

    /// Helper: extract a field into a temp local, test sub-pattern, pop temp.
    fn emit_field_extract_test(
        &mut self,
        slot: u16,
        field_idx: usize,
        sub_pat: &Pat,
        fail_jumps: &mut Vec<(usize, usize)>,
    ) -> Result<(), CompileError> {
        if is_trivial_pat(sub_pat) {
            return Ok(()); // wildcard/var: nothing to test
        }
        // Push field value as temp local for sub-pattern testing
        let field_slot = self.emit_field_extract(slot, field_idx)?;
        self.compile_pattern_test(field_slot, sub_pat, fail_jumps)?;
        // Pop the temp local (we'll re-extract during bind phase)
        self.ctx_mut().locals.pop();
        self.emit(Op::Pop);
        Ok(())
    }

    /// Pass 2: bind pattern variables. All tests have passed.
    fn compile_pattern_bind(&mut self, slot: u16, pat: &Pat) -> Result<(), CompileError> {
        match &pat.kind {
            PatKind::Wildcard | PatKind::Unit => {}
            PatKind::Var(sym) => {
                let name = self.interner.resolve(*sym).to_string();
                self.emit_get_local(slot);
                self.add_local(name);
            }
            PatKind::IntLit(_)
            | PatKind::FloatLit(_)
            | PatKind::WordLit(_)
            | PatKind::BoolLit(_)
            | PatKind::StringLit(_)
            | PatKind::CharLit(_) => {} // no bindings

            PatKind::Constructor(_, None) => {}
            PatKind::Constructor(_, Some(sub_pat)) => {
                let field_slot = self.emit_field_extract(slot, 0)?;
                self.compile_pattern_bind(field_slot, sub_pat)?;
            }

            PatKind::Tuple(pats) => {
                for (i, p) in pats.iter().enumerate() {
                    let field_slot = self.emit_field_extract(slot, i)?;
                    self.compile_pattern_bind(field_slot, p)?;
                }
            }

            PatKind::Cons(hd, tl) => {
                let hd_slot = self.emit_field_extract(slot, 0)?;
                self.compile_pattern_bind(hd_slot, hd)?;
                let tl_slot = self.emit_field_extract(slot, 1)?;
                self.compile_pattern_bind(tl_slot, tl)?;
            }

            PatKind::List(_) => {} // empty list, nothing to bind

            PatKind::As(sym, sub_pat) => {
                let name = self.interner.resolve(*sym).to_string();
                self.emit_get_local(slot);
                self.add_local(name);
                self.compile_pattern_bind(slot, sub_pat)?;
            }
            PatKind::Paren(p) | PatKind::Ann(p, _) => self.compile_pattern_bind(slot, p)?,
        }
        Ok(())
    }

    // ── Generic binary op resolution ────────────────────────────────

    fn resolve_generic_binop(&self, op: BinOp, span: hiko_syntax::span::Span) -> Option<Op> {
        use hiko_types::ty::Type;
        let ty = self.infer_ctx.expr_types.get(&span)?;
        let is_int = matches!(ty, Type::Con(n) if n == "Int");
        let is_float = matches!(ty, Type::Con(n) if n == "Float");
        let is_word = matches!(ty, Type::Con(n) if n == "Word");
        match op {
            BinOp::Add if is_int => Some(Op::AddInt),
            BinOp::Add if is_float => Some(Op::AddFloat),
            BinOp::Add if is_word => Some(Op::AddWord),
            BinOp::Sub if is_int => Some(Op::SubInt),
            BinOp::Sub if is_float => Some(Op::SubFloat),
            BinOp::Sub if is_word => Some(Op::SubWord),
            BinOp::Mul if is_int => Some(Op::MulInt),
            BinOp::Mul if is_float => Some(Op::MulFloat),
            BinOp::Mul if is_word => Some(Op::MulWord),
            BinOp::Div if is_int => Some(Op::DivInt),
            BinOp::Div if is_float => Some(Op::DivFloat),
            BinOp::Div if is_word => Some(Op::DivWord),
            BinOp::Mod if is_int => Some(Op::ModInt),
            BinOp::Mod if is_word => Some(Op::ModWord),
            BinOp::Lt if is_int => Some(Op::LtInt),
            BinOp::Lt if is_float => Some(Op::LtFloat),
            BinOp::Lt if is_word => Some(Op::LtWord),
            BinOp::Gt if is_int => Some(Op::GtInt),
            BinOp::Gt if is_float => Some(Op::GtFloat),
            BinOp::Gt if is_word => Some(Op::GtWord),
            BinOp::Le if is_int => Some(Op::LeInt),
            BinOp::Le if is_float => Some(Op::LeFloat),
            BinOp::Le if is_word => Some(Op::LeWord),
            BinOp::Ge if is_int => Some(Op::GeInt),
            BinOp::Ge if is_float => Some(Op::GeFloat),
            BinOp::Ge if is_word => Some(Op::GeWord),
            _ => None,
        }
    }

    // ── Expressions ──────────────────────────────────────────────────

    /// Compiles a handle expression whose end is also the end of the current
    /// function, so the return clause is compiled in tail position.
    fn compile_handle(
        &mut self,
        body: &Expr,
        return_var: Symbol,
        return_body: &Expr,
        handlers: &[EffectHandler],
    ) -> Result<(), CompileError> {
        // A provider clause (every path ends in `resume k e`) becomes a
        // closure of (payload, k). `Perform` calls it at the perform site and
        // its result is the result of `perform`. The closures stay on the
        // stack as unnamed locals below the handler, which keeps them alive
        // while it is installed.
        self.begin_scope();
        let mut provider_slots = Vec::with_capacity(handlers.len());
        for handler in handlers {
            if ends_in_resume(&handler.body, handler.cont_var) {
                self.compile_provider_clause(handler)?;
                provider_slots.push(Some(self.add_local(String::new())));
            } else {
                provider_slots.push(None);
            }
        }

        // Emit InstallHandler: u16 n_clauses, then per clause:
        //   u16 effect_tag, u8 flags, u16 closure slot or i16 offset
        self.emit(Op::InstallHandler);
        let n_clauses = checked_u16(handlers.len(), "handler clauses")?;
        self.emit_u16(n_clauses);
        let mut clause_offset_positions = Vec::new();
        for (handler, provider_slot) in handlers.iter().zip(&provider_slots) {
            let eff_name = self.interner.resolve(handler.effect_name).to_string();
            let tag = *self
                .effect_tags
                .get(&eff_name)
                .ok_or_else(|| CompileError::codegen(format!("unknown effect: {eff_name}")))?;
            self.emit_u16(tag);
            match provider_slot {
                Some(slot) => {
                    self.emit_u8(HANDLER_CLAUSE_PROVIDER);
                    self.emit_u16(*slot);
                }
                None => {
                    self.emit_u8(0);
                    clause_offset_positions.push(self.chunk().code.len());
                    self.emit_u16(0); // placeholder offset
                }
            }
        }

        // Jump over clause code
        let skip_jump = self.emit_jump(Op::Jump);

        // An abort clause runs in this frame after `Perform` unwinds to the
        // handler. Its continuation slot holds Unit and cannot be read.
        let mut clause_end_jumps = Vec::new();
        let abort_clauses = handlers
            .iter()
            .zip(&provider_slots)
            .filter(|(_, slot)| slot.is_none());
        for ((handler, _), offset_pos) in abort_clauses.zip(clause_offset_positions) {
            self.patch_jump(offset_pos)?;
            self.begin_scope();
            let payload_name = self.interner.resolve(handler.payload_var).to_string();
            let cont_name = self.interner.resolve(handler.cont_var).to_string();
            self.add_local(payload_name);
            self.add_continuation_local(cont_name);
            self.compile_expr_inner(&handler.body, false)?;
            self.end_scope_keep_result();
            clause_end_jumps.push(self.emit_jump(Op::Jump));
        }

        // Patch skip_jump to land here (after clause code)
        self.patch_jump(skip_jump)?;

        // Compile the body (runs under the handler)
        self.compile_expr_inner(body, false)?;

        // Body returned normally; remove handler
        self.emit(Op::RemoveHandler);

        // Compile the return clause
        self.begin_scope();
        let return_var_name = self.interner.resolve(return_var).to_string();
        self.add_local(return_var_name);
        self.compile_expr_tail(return_body)?;
        self.end_scope_keep_result();

        // Patch all clause end jumps to here
        for j in clause_end_jumps {
            self.patch_jump(j)?;
        }
        self.end_scope_keep_result();
        Ok(())
    }

    /// Compile a provider clause as a closure taking the payload and a Unit
    /// continuation slot, and leave the closure on the stack.
    fn compile_provider_clause(&mut self, handler: &EffectHandler) -> Result<(), CompileError> {
        self.func_stack.push(FuncCtx::new(None, 2));
        let payload_name = self.interner.resolve(handler.payload_var).to_string();
        let cont_name = self.interner.resolve(handler.cont_var).to_string();
        self.add_local(payload_name);
        let cont_slot = self.add_continuation_local(cont_name);
        self.ctx_mut().provider_cont_slot = Some(cont_slot);
        self.compile_expr_tail(&handler.body)?;
        self.emit(Op::Return);
        self.finish_function()
    }

    fn compile_expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
        self.compile_expr_inner(expr, false)
    }

    /// Compile operands left to right, leaving each value on the stack.
    ///
    /// Locals live on the same stack as operands, so while a later operand
    /// is compiled the earlier values occupy slots. Reserve those slots as
    /// unnamed locals; otherwise a `case` scrutinee, `let` binding or pattern
    /// temporary inside a later operand gets a slot index that ignores them.
    /// Whether `func` names the function being compiled, from its own body:
    /// a local recursive binding of one parameter, not shadowed by a local.
    fn is_self_call(&self, func: &Expr) -> bool {
        let ExprKind::Var(sym) = &func.kind else {
            return false;
        };
        let ctx = self.ctx();
        if !ctx.self_callable || ctx.arity != 1 {
            return false;
        }
        let name = self.interner.resolve(*sym);
        ctx.name.as_deref() == Some(name) && !ctx.locals.iter().any(|local| local.name == name)
    }

    /// The flat proto and tuple components of `func arg`, when `func` names
    /// a function with a flat entry and `arg` is a tuple expression.
    fn flat_call_target<'e>(&mut self, func: &Expr, arg: &'e Expr) -> Option<(u16, &'e [Expr])> {
        let ExprKind::Var(sym) = &func.kind else {
            return None;
        };
        let mut arg = arg;
        while let ExprKind::Paren(inner) = &arg.kind {
            arg = inner;
        }
        let ExprKind::Tuple(items) = &arg.kind else {
            return None;
        };
        let name = self.interner.resolve(*sym).to_string();
        let flat = self.direct_flat_proto(&name, items.len())?;
        Some((flat, items))
    }

    fn compile_operands(&mut self, operands: &[&Expr]) -> Result<(), CompileError> {
        let locals_before = self.ctx().locals.len();
        for (i, operand) in operands.iter().enumerate() {
            self.compile_expr(operand)?;
            if i + 1 < operands.len() {
                // An empty name never resolves as a variable.
                self.add_local(String::new());
            }
        }
        self.ctx_mut().locals.truncate(locals_before);
        Ok(())
    }

    fn compile_expr_tail(&mut self, expr: &Expr) -> Result<(), CompileError> {
        self.compile_expr_inner(expr, true)
    }

    fn compile_expr_inner(&mut self, expr: &Expr, tail: bool) -> Result<(), CompileError> {
        self.emit_span(expr.span);
        match &expr.kind {
            ExprKind::IntLit(n) => self.emit_constant(Constant::Int(*n))?,
            ExprKind::FloatLit(f) => self.emit_constant(Constant::Float(*f))?,
            ExprKind::WordLit(w) => self.emit_constant(Constant::Word(*w))?,
            ExprKind::StringLit(s) => self.emit_constant(Constant::String(s.clone()))?,
            ExprKind::CharLit(c) => self.emit_constant(Constant::Char(*c))?,
            ExprKind::BoolLit(true) => self.emit(Op::True),
            ExprKind::BoolLit(false) => self.emit(Op::False),
            ExprKind::Unit => self.emit(Op::Unit),
            ExprKind::Var(sym) | ExprKind::Constructor(sym) => {
                let name = self.interner.resolve(*sym).to_string();
                if self.is_continuation(&name) {
                    return Err(continuation_error(
                        expr.span,
                        format!(
                            "the continuation `{name}` is not a value; use it only as \
                             `resume {name} e` at the end of a handler clause"
                        ),
                    ));
                }
                self.emit_get_var(&name)?;
            }

            ExprKind::Tuple(elems) => {
                let elems_refs: Vec<&Expr> = elems.iter().collect();
                self.compile_operands(&elems_refs)?;
                self.emit_make_tuple(elems.len())?;
            }
            ExprKind::List(elems) => {
                // Same code as the nested `e1 :: (e2 :: ... :: [])`: the
                // elements left to right, then Nil, then one MakeCons each.
                let locals_before = self.ctx().locals.len();
                for elem in elems {
                    self.compile_expr(elem)?;
                    // An empty name never resolves as a variable.
                    self.add_local(String::new());
                }
                self.ctx_mut().locals.truncate(locals_before);
                self.emit(Op::MakeData);
                self.emit_u16(0); // Nil
                self.emit_u8(0);
                for _ in elems {
                    self.emit(Op::MakeCons);
                }
            }
            ExprKind::Cons(head, tail) => {
                self.compile_operands(&[head, tail])?;
                self.emit(Op::MakeCons);
            }

            ExprKind::BinOp(BinOp::Andalso | BinOp::Orelse, _, _) => {
                unreachable!("desugared to if-then-else")
            }
            ExprKind::BinOp(op, lhs, rhs) => {
                let resolved_op = self
                    .resolve_generic_binop(*op, expr.span)
                    .unwrap_or_else(|| binop_to_op(*op));
                self.compile_operands(&[lhs, rhs])?;
                self.emit(resolved_op);
            }

            ExprKind::UnaryNeg(e) => {
                self.compile_expr(e)?;
                let is_float = self
                    .infer_ctx
                    .expr_types
                    .get(&expr.span)
                    .is_some_and(|t| matches!(t, hiko_types::ty::Type::Con(n) if n == "Float"));
                self.emit(if is_float { Op::NegFloat } else { Op::Neg });
            }
            ExprKind::Not(_) => {
                unreachable!("desugared to if-then-else")
            }

            ExprKind::App(func, arg) if self.is_self_call(func) => {
                // A local recursive function calling itself: same function,
                // same captures, so no closure is looked up.
                self.compile_expr(arg)?;
                self.emit(if tail { Op::TailCallSelf } else { Op::CallSelf });
            }
            ExprKind::App(func, arg)
                if let Some((flat, items)) = self.flat_call_target(func, arg) =>
            {
                // `f (e1, ..., eN)` of a function with a flat entry: pass
                // the components as arguments instead of building a tuple.
                self.compile_operands(&items.iter().collect::<Vec<_>>())?;
                self.emit(if tail {
                    Op::TailCallDirect
                } else {
                    Op::CallDirect
                });
                self.emit_u16(flat);
            }
            ExprKind::App(func, arg) => {
                // Check for direct call: App(Var(name), arg) where name
                // is a known global function with no captures
                let direct_proto = if let ExprKind::Var(sym) = &func.kind {
                    let name = self.interner.resolve(*sym).to_string();
                    self.direct_call_proto(&name)
                } else {
                    None
                };
                if let Some(proto_idx) = direct_proto {
                    self.compile_expr(arg)?;
                    if tail {
                        self.emit(Op::TailCallDirect);
                    } else {
                        self.emit(Op::CallDirect);
                    }
                    self.emit_u16(proto_idx);
                } else {
                    self.compile_operands(&[func, arg])?;
                    if tail {
                        self.emit(Op::TailCall);
                    } else {
                        self.emit(Op::Call);
                    }
                    self.emit_u8(1);
                }
            }

            ExprKind::Fn(pat, body) => self.compile_lambda(pat, body)?,

            ExprKind::If(cond, then_br, else_br) => {
                let else_jump = self.compile_cond_jump(cond)?;
                self.compile_expr_inner(then_br, tail)?;
                let end_jump = self.emit_jump(Op::Jump);
                self.patch_jump(else_jump)?;
                self.compile_expr_inner(else_br, tail)?;
                self.patch_jump(end_jump)?;
            }

            ExprKind::Let(decls, body) => {
                self.begin_scope();
                for d in decls {
                    self.compile_decl(d)?;
                }
                self.compile_expr_inner(body, tail)?;
                self.end_scope_keep_result();
            }

            ExprKind::Case(scrutinee, branches) => {
                self.begin_scope();
                self.compile_expr(scrutinee)?;
                let scrut_slot = self.add_local("_scrut".to_string());
                let branch_refs: Vec<_> = branches.iter().map(|(p, e)| (p, e)).collect();
                self.compile_case_branches_inner(scrut_slot, &branch_refs, tail)?;
                self.end_scope_keep_result();
            }

            ExprKind::Ann(e, _) | ExprKind::Paren(e) => self.compile_expr_inner(e, tail)?,

            ExprKind::Perform(sym, arg) => {
                let name = self.interner.resolve(*sym).to_string();
                let tag = *self
                    .effect_tags
                    .get(&name)
                    .ok_or_else(|| CompileError::codegen(format!("unknown effect: {name}")))?;
                self.compile_expr_inner(arg, false)?;
                self.emit(Op::Perform);
                self.emit_u16(tag);
            }

            ExprKind::Handle {
                body,
                return_var,
                return_body,
                handlers,
            } => self.compile_handle(body, *return_var, return_body, handlers)?,

            ExprKind::Resume(cont, arg) => {
                // In a provider clause, `resume k e` in tail position is the
                // clause's result: `Perform` returns it to the perform site.
                let resumes_this_clause = tail
                    && match &strip_parens(cont).kind {
                        ExprKind::Var(sym) => {
                            let name = self.interner.resolve(*sym).to_string();
                            self.resolve_local(&name)
                                .is_some_and(|slot| self.ctx().provider_cont_slot == Some(slot))
                        }
                        _ => false,
                    };
                if !resumes_this_clause {
                    return Err(continuation_error(
                        expr.span,
                        "`resume k e` is supported only at the end of a handler clause, \
                         with every path of the clause ending in `resume k e`",
                    ));
                }
                self.compile_expr_inner(arg, false)?;
            }
        }
        Ok(())
    }
}

/// Convert a count or index to a `u8` bytecode operand, failing instead of
/// silently truncating.
fn checked_u8(value: usize, what: &str) -> Result<u8, CompileError> {
    u8::try_from(value).map_err(|_| {
        CompileError::codegen(format!(
            "too many {what}: {value} exceeds the bytecode limit of {}",
            u8::MAX
        ))
    })
}

/// Convert a count or index to a `u16` bytecode operand, failing instead of
/// silently truncating.
fn checked_u16(value: usize, what: &str) -> Result<u16, CompileError> {
    u16::try_from(value).map_err(|_| {
        CompileError::codegen(format!(
            "too many {what}: {value} exceeds the bytecode limit of {}",
            u16::MAX
        ))
    })
}

/// A misuse of a handler clause's continuation, reported at its source
/// location.
fn continuation_error(span: Span, message: impl Into<String>) -> CompileError {
    CompileError::Type(TypeError {
        message: message.into(),
        span,
    })
}

fn strip_parens(expr: &Expr) -> &Expr {
    match &expr.kind {
        ExprKind::Paren(e) | ExprKind::Ann(e, _) => strip_parens(e),
        _ => expr,
    }
}

/// Whether every path through a clause body ends in `resume k e`, which
/// makes the clause a provider. Shadowing is checked when `resume` is
/// compiled.
fn ends_in_resume(expr: &Expr, cont: Symbol) -> bool {
    match &strip_parens(expr).kind {
        ExprKind::Resume(k, _) => matches!(strip_parens(k).kind, ExprKind::Var(sym) if sym == cont),
        ExprKind::Let(_, body) => ends_in_resume(body, cont),
        ExprKind::If(_, then_branch, else_branch) => {
            ends_in_resume(then_branch, cont) && ends_in_resume(else_branch, cont)
        }
        ExprKind::Case(_, branches) => {
            !branches.is_empty() && branches.iter().all(|(_, body)| ends_in_resume(body, cont))
        }
        _ => false,
    }
}

fn simple_pat_name(pat: &Pat, interner: &StringInterner) -> String {
    match &pat.kind {
        PatKind::Var(sym) => interner.resolve(*sym).to_string(),
        PatKind::Wildcard => "_".to_string(),
        PatKind::Paren(p) | PatKind::Ann(p, _) => simple_pat_name(p, interner),
        _ => "_".to_string(),
    }
}

/// The components of a tuple pattern, under any parentheses and type
/// annotations.
fn tuple_components(pat: &Pat) -> Option<&[Pat]> {
    match &pat.kind {
        PatKind::Tuple(items) => Some(items),
        PatKind::Paren(p) | PatKind::Ann(p, _) => tuple_components(p),
        _ => None,
    }
}

/// N when every clause of `binding` takes one tuple pattern of N >= 2
/// components, so the function can take the components as N arguments.
fn flat_tuple_width(binding: &FunBinding) -> Option<usize> {
    let mut width = None;
    for clause in &binding.clauses {
        let [pat] = clause.pats.as_slice() else {
            return None;
        };
        let n = tuple_components(pat)?.len();
        if n < 2 || n > usize::from(u8::MAX) || width.is_some_and(|w| w != n) {
            return None;
        }
        width = Some(n);
    }
    width
}

/// A top-level function compiles into its reserved proto and captures
/// nothing, which the direct calls emitted inside its group rely on.
fn check_top_level_closure(
    name: &str,
    closure: &PendingClosure,
    reserved: u16,
) -> Result<(), CompileError> {
    if closure.proto_idx != reserved || closure.n_captures != 0 {
        return Err(CompileError::codegen(format!(
            "top-level function `{name}` was compiled with captures"
        )));
    }
    Ok(())
}

fn is_simple_pat(pat: &Pat) -> bool {
    match &pat.kind {
        PatKind::Var(_) | PatKind::Wildcard => true,
        PatKind::Paren(p) | PatKind::Ann(p, _) => is_simple_pat(p),
        _ => false,
    }
}

fn is_trivial_pat(pat: &Pat) -> bool {
    match &pat.kind {
        PatKind::Var(_) | PatKind::Wildcard | PatKind::Unit => true,
        PatKind::Paren(p) | PatKind::Ann(p, _) => is_trivial_pat(p),
        _ => false,
    }
}

fn binop_to_op(op: BinOp) -> Op {
    match op {
        BinOp::Pipe => unreachable!("pipeline is desugared to application"),
        // Int-specific
        BinOp::AddInt => Op::AddInt,
        BinOp::SubInt => Op::SubInt,
        BinOp::MulInt => Op::MulInt,
        BinOp::DivInt => Op::DivInt,
        BinOp::ModInt => Op::ModInt,
        BinOp::LtInt => Op::LtInt,
        BinOp::GtInt => Op::GtInt,
        BinOp::LeInt => Op::LeInt,
        BinOp::GeInt => Op::GeInt,
        // Word-specific
        BinOp::AddWord => Op::AddWord,
        BinOp::SubWord => Op::SubWord,
        BinOp::MulWord => Op::MulWord,
        BinOp::DivWord => Op::DivWord,
        BinOp::ModWord => Op::ModWord,
        BinOp::LtWord => Op::LtWord,
        BinOp::GtWord => Op::GtWord,
        BinOp::LeWord => Op::LeWord,
        BinOp::GeWord => Op::GeWord,
        // String
        BinOp::ConcatStr => Op::ConcatString,
        // Equality
        BinOp::Eq => Op::Eq,
        BinOp::Ne => Op::Ne,
        // Short-circuit
        BinOp::Andalso | BinOp::Orelse => unreachable!("short-circuit ops handled separately"),
        // Generic ops should be resolved via expr_types in resolve_generic_binop
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::Mod
        | BinOp::Lt
        | BinOp::Gt
        | BinOp::Le
        | BinOp::Ge => {
            unreachable!("generic ops should be resolved via expr_types")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "loader-integrity-blake3")]
    use std::io::{Read, Write};
    #[cfg(feature = "loader-integrity-blake3")]
    use std::net::TcpListener;
    #[cfg(feature = "loader-integrity-blake3")]
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn parse_program(source: &str) -> Program {
        let tokens = Lexer::new(source, 0).tokenize().expect("lex");
        Parser::new(tokens).parse_program().expect("parse")
    }

    #[test]
    fn lexical_global_bindings_allow_70000_unread_rebindings() {
        let source = "val x = 1\n".repeat(70_000);
        let (program, _) =
            Compiler::compile(parse_program(&source)).expect("compile 70k rebindings");
        assert_eq!(
            program.globals.len(),
            1,
            "unread bindings reuse their storage"
        );
    }

    /// Isolate binding resolution from lexing, inference, code emission, and
    /// source-name generation. Run explicitly; timing is not a CI gate.
    #[test]
    #[ignore = "manual binding-resolution growth measurement"]
    fn lexical_global_bindings_growth() {
        for size in [4_000, 8_000, 16_000] {
            let names: Vec<_> = (0..size).map(|i| format!("v{i}")).collect();
            let mut samples = Vec::new();
            for _ in 0..11 {
                let mut compiler = Compiler::new(
                    PathBuf::new(),
                    None,
                    HashSet::new(),
                    ModuleLoaderOptions::default(),
                    StringInterner::default(),
                );
                let start = std::time::Instant::now();
                for _ in 0..2 {
                    for name in &names {
                        compiler.declare_global(name);
                        std::hint::black_box(compiler.global_slot(name).unwrap());
                    }
                }
                samples.push(start.elapsed().as_nanos());
                assert_eq!(compiler.global_names.len(), size);
            }
            samples.sort_unstable();
            eprintln!(
                "binding_resolution names={size} median_ns={}",
                samples[samples.len() / 2]
            );
        }
    }

    #[cfg(feature = "loader-embedded")]
    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    #[cfg(feature = "loader-embedded")]
    /// Embedded sources with one module `Pkg.Greet` (its hash computed
    /// unless `hash` is given) and the given local files.
    fn embedded(
        module_source: &str,
        hash: Option<&str>,
        files: Option<Vec<(&str, &str)>>,
    ) -> ModuleLoaderOptions {
        let blake3 = match hash {
            Some(hash) => leak(hash.to_string()),
            None => leak(format!("blake3:{}", blake3_hex(module_source.as_bytes()))),
        };
        let modules = Box::leak(Box::new([EmbeddedModule {
            import_name: "Pkg.Greet",
            blake3,
            source: leak(module_source.to_string()),
        }]));
        let files = files.map(|files| {
            let files: Vec<EmbeddedFile> = files
                .into_iter()
                .map(|(path, source)| EmbeddedFile {
                    path: leak(path.to_string()),
                    source: leak(source.to_string()),
                })
                .collect();
            &*Box::leak(files.into_boxed_slice())
        });
        ModuleLoaderOptions {
            embedded: Some(Box::leak(Box::new(EmbeddedSources { modules, files }))),
            ..ModuleLoaderOptions::default()
        }
    }

    #[cfg(feature = "loader-embedded")]
    const GREET: &str = "structure Greet = struct val hello = 42 end";

    #[cfg(feature = "loader-embedded")]
    #[test]
    fn embedded_named_import_resolves_without_a_lockfile() {
        let program = parse_program("import Pkg.Greet\nval x = Greet.hello");
        Compiler::compile_embedded(program, embedded(GREET, None, None))
            .unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[cfg(feature = "loader-embedded")]
    #[test]
    fn embedded_module_with_a_wrong_hash_is_refused() {
        let program = parse_program("import Pkg.Greet\nval x = Greet.hello");
        let wrong = format!("blake3:{}", "0".repeat(64));
        let err = Compiler::compile_embedded(program, embedded(GREET, Some(&wrong), None))
            .expect_err("a tampered module must not compile");
        assert!(
            format!("{err:?}").contains("failed BLAKE3 verification"),
            "{err:?}"
        );
    }

    #[cfg(feature = "loader-embedded")]
    #[test]
    fn named_import_not_embedded_is_refused() {
        let program = parse_program("import Pkg.Other\nval x = 1");
        let err = Compiler::compile_embedded(program, embedded(GREET, None, None))
            .expect_err("only embedded modules resolve");
        assert!(
            format!("{err:?}").contains("not embedded in this artifact"),
            "{err:?}"
        );
    }

    #[cfg(feature = "loader-embedded")]
    #[test]
    fn embedded_local_use_resolves_relative_to_the_using_file() {
        let program = parse_program("use \"./lib/a.hml\"\nval y = a + b");
        let files = vec![
            ("lib/a.hml", "use \"./b.hml\"\nval a = 1"),
            ("lib/b.hml", "val b = 2"),
        ];
        Compiler::compile_embedded(program, embedded(GREET, None, Some(files)))
            .unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[cfg(feature = "loader-embedded")]
    #[test]
    fn embedded_local_use_never_reads_the_filesystem() {
        // Cargo.toml exists next to this crate, but it is not embedded.
        let program = parse_program("use \"./Cargo.toml\"\nval y = 1");
        let err = Compiler::compile_embedded(program, embedded(GREET, None, Some(vec![])))
            .expect_err("only embedded files resolve");
        assert!(
            format!("{err:?}").contains("not embedded in this artifact"),
            "{err:?}"
        );
    }

    #[cfg(feature = "loader-embedded")]
    #[test]
    fn embedded_module_cannot_use_local_files() {
        let module = "use \"./x.hml\"\nstructure Greet = struct val hello = 1 end";
        let program = parse_program("import Pkg.Greet\nval x = Greet.hello");
        let files = vec![("x.hml", "val z = 0")];
        let err = Compiler::compile_embedded(program, embedded(module, None, Some(files)))
            .expect_err("a locked module may not use files");
        assert!(
            format!("{err:?}").contains("not allowed in a remote module"),
            "{err:?}"
        );
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn locked_module_sources_reads_every_locked_module_verified() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let options = ModuleLoaderOptions {
            cache_dir: Some(unique_temp_dir("locked-sources")),
            local_package_root: Some(repo.join("libraries")),
            ..Default::default()
        };
        let sources = Compiler::locked_module_sources(&repo.join("hiko.toml"), options)
            .unwrap_or_else(|e| panic!("{e:?}"));
        let option = sources
            .iter()
            .find(|m| m.import_name == "Std.Option")
            .expect("Std.Option is locked");
        let on_disk =
            std::fs::read_to_string(repo.join("libraries/Std-v0.1.0/modules/Option.hml")).unwrap();
        assert_eq!(option.source, on_disk);
        assert_eq!(
            normalize_blake3(&option.blake3),
            blake3_hex(on_disk.as_bytes())
        );
        let names: Vec<_> = sources.iter().map(|m| m.import_name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn a_used_module_decides_its_own_operators_even_inside_local() {
        // `dbl`'s `+` is decided by nothing in m.hml, so the module is
        // rejected wherever it is used; the importer's `dbl 21` must not
        // decide it, or the cached module's type would depend on where it
        // was first used.
        let dir = unique_temp_dir("use-in-local");
        write_file(&dir.join("m.hml"), "fun dbl x = x + x\n");
        write_file(
            &dir.join("main.hml"),
            "local use \"./m.hml\" in val y = dbl 21 end\n",
        );
        let err = compile_path(&dir.join("main.hml")).expect_err("m.hml leaves + undecided");
        assert!(
            format!("{err:?}").contains("cannot tell whether"),
            "{err:?}"
        );
        write_file(&dir.join("m.hml"), "fun dbl (x : int) = x + x\n");
        compile_path(&dir.join("main.hml")).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn local_use_files_finds_uses_inside_local_and_structures() {
        let dir = unique_temp_dir("local-use-nested");
        let source = "local use \"./a.hml\" in val x = a end\n\
                      structure S = struct use \"./b.hml\" val y = b end";
        write_file(&dir.join("main.hml"), source);
        write_file(&dir.join("a.hml"), "val a = 1");
        write_file(&dir.join("b.hml"), "val b = 2");
        let files =
            local_use_files(&dir.join("main.hml"), source).unwrap_or_else(|e| panic!("{e:?}"));
        let keys: Vec<_> = files.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["a.hml", "b.hml"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn local_use_files_follows_uses_relative_to_each_file() {
        let dir = unique_temp_dir("local-use-files");
        write_file(&dir.join("main.hml"), "use \"./lib/a.hml\"\nval x = a");
        write_file(
            &dir.join("lib/a.hml"),
            "use \"./b.hml\"\nuse \"../c.hml\"\nval a = b + c",
        );
        write_file(&dir.join("lib/b.hml"), "val b = 1");
        write_file(&dir.join("c.hml"), "use \"./lib/b.hml\"\nval c = 2");
        let main = dir.join("main.hml");
        let source = std::fs::read_to_string(&main).unwrap();
        let files = local_use_files(&main, &source).unwrap_or_else(|e| panic!("{e:?}"));
        let keys: Vec<_> = files.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["c.hml", "lib/a.hml", "lib/b.hml"]);
        assert_eq!(files[2].1, "val b = 1");
    }

    #[cfg(unix)]
    #[test]
    fn local_use_files_refuses_a_path_through_a_symbolic_link() {
        let dir = unique_temp_dir("local-use-symlink");
        write_file(&dir.join("real/b.hml"), "val b = 1");
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        write_file(&dir.join("main.hml"), "");
        let source = "use \"./link/b.hml\"\nval x = b";
        let err = local_use_files(&dir.join("main.hml"), source).expect_err("link/ is a symlink");
        assert!(format!("{err:?}").contains("symbolic link"), "{err:?}");
        // Into the link and back out: the key is "b.hml", but `hiko run`
        // reads the file next to the link's target.
        // hop -> real/sub, so hop/.. is real/, which holds a different c.hml.
        write_file(&dir.join("real/sub/x.hml"), "");
        write_file(&dir.join("real/c.hml"), "val c = 2");
        std::os::unix::fs::symlink(dir.join("real/sub"), dir.join("hop")).unwrap();
        write_file(&dir.join("c.hml"), "val c = 3");
        let source = "use \"./hop/../c.hml\"\nval x = c";
        let err =
            local_use_files(&dir.join("main.hml"), source).expect_err("hop/.. leaves the link");
        assert!(format!("{err:?}").contains("symbolic link"), "{err:?}");
    }

    #[test]
    fn embedded_file_paths_are_normalized_and_relative() {
        let path = |base: &str, p: &str| embedded_file_path(Path::new(base), p).ok();
        assert_eq!(path("", "a.hml").as_deref(), Some("a.hml"));
        assert_eq!(path("", "./lib/../a.hml").as_deref(), Some("a.hml"));
        assert_eq!(path("lib", "b.hml").as_deref(), Some("lib/b.hml"));
        assert_eq!(path("lib", "../../up.hml").as_deref(), Some("../up.hml"));
        assert_eq!(path("", "/etc/passwd.hml"), None);
    }

    #[test]
    fn toml_error_text_gives_the_position_and_no_source_line() {
        let source = "a = 1\né = 2\nexport TOKEN=value\n";
        let error = toml::from_str::<toml::Table>(source).unwrap_err();
        assert_eq!(
            toml_error_text(&error, source),
            "line 2, column 1: invalid key"
        );
        let source = "a = 1\nexport TOKEN=value\n";
        let error = toml::from_str::<toml::Table>(source).unwrap_err();
        assert_eq!(
            toml_error_text(&error, source),
            "line 2, column 8: expected `.`, `=`"
        );
    }

    #[test]
    fn a_lockfile_that_leaves_the_manifest_directory_is_rejected() {
        let dir = unique_temp_dir("lockfile-bound");
        let project = dir.join("project");
        write_file(&project.join("hiko.toml"), "");
        let manifest = project.join("hiko.toml");
        let outside = dir.join("outside.toml");
        write_file(&outside, "");
        let inputs = [
            "../x",
            "new/../../planted",
            "new/../inside.toml",
            outside.to_str().unwrap(),
        ];
        for lockfile in inputs {
            assert_eq!(
                bounded_lockfile_path(&manifest, Some(lockfile)).unwrap_err(),
                format!(
                    "project manifest '{}' sets lockfile to '{lockfile}', which is outside its directory; use a path inside it",
                    manifest.display()
                )
            );
        }
    }

    #[test]
    fn a_lockfile_inside_the_manifest_directory_passes_the_bound() {
        let dir = unique_temp_dir("lockfile-inside");
        write_file(&dir.join("hiko.toml"), "");
        std::fs::create_dir(dir.join("sub")).unwrap();
        let manifest = dir.join("hiko.toml");
        let absolute = dir.join("new.toml");
        let inputs = [
            (None, dir.join("hiko.lock.toml")),
            (Some("new/deeper/x.toml"), dir.join("new/deeper/x.toml")),
            (Some("sub/../x.toml"), dir.join("sub/../x.toml")),
            (Some(absolute.to_str().unwrap()), absolute.clone()),
        ];
        for (lockfile, expected) in inputs {
            assert_eq!(
                bounded_lockfile_path(&manifest, lockfile).unwrap(),
                expected
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_final_symlink_is_bounded_by_where_it_leads() {
        use std::os::unix::fs::symlink;

        let dir = unique_temp_dir("lockfile-symlink");
        let project = dir.join("project");
        write_file(&project.join("hiko.toml"), "");
        write_file(&project.join("real.toml"), "");
        write_file(&dir.join("outside.toml"), "");
        let manifest = project.join("hiko.toml");
        symlink("../outside.toml", project.join("hiko.lock.toml")).unwrap();
        symlink("../missing.toml", project.join("dangling.toml")).unwrap();
        symlink("real.toml", project.join("inside.toml")).unwrap();

        assert!(bounded_lockfile_path(&manifest, None).is_err());
        let dangling = bounded_lockfile_path(&manifest, Some("dangling.toml")).unwrap_err();
        assert!(dangling.contains("cannot be resolved"), "got: {dangling}");
        assert!(dangling.contains("fix or remove it"), "got: {dangling}");
        assert!(!dangling.contains("outside"), "got: {dangling}");
        assert!(bounded_lockfile_path(&manifest, Some("inside.toml")).is_ok());
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "hiko-compile-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parents");
        }
        std::fs::write(path, contents).expect("write file");
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn write_project_manifest(project_dir: &Path) {
        write_file(
            &project_dir.join("hiko.toml"),
            "[defaults]\nlockfile = \"hiko.lock.toml\"\n",
        );
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn write_project_manifest_with_dependency(
        project_dir: &Path,
        package: &str,
        version: &str,
        registry_url: &str,
    ) {
        write_file(
            &project_dir.join("hiko.toml"),
            &format!(
                "[defaults]\nlockfile = \"hiko.lock.toml\"\n\n[registries.local]\nurl = \"{registry_url}\"\n\n[dependencies]\n{package} = {{ version = \"{version}\", registry = \"local\" }}\n"
            ),
        );
    }

    /// Per-test module cache next to the entry file, so tests never touch
    /// the user's `~/.hiko/lib-cache`.
    fn test_cache_dir(entry_path: &Path) -> PathBuf {
        entry_path
            .parent()
            .expect("entry file has a parent directory")
            .join(".test-lib-cache")
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn cached_module_path(entry_path: &Path, url: &str, hash: &str) -> PathBuf {
        test_cache_dir(entry_path).join(format!("{}-{}.hml", blake3_hex(url.as_bytes()), hash))
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn spawn_response_server(
        expected_path: &'static str,
        body: String,
        request_count: usize,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind server");
        let addr = listener.local_addr().expect("server addr");
        let handle = thread::spawn(move || {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut request = [0_u8; 4096];
                let count = stream.read(&mut request).expect("read request");
                let request = String::from_utf8_lossy(&request[..count]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (status, response_body) = if path == expected_path {
                    ("200 OK", body.clone())
                } else {
                    ("404 Not Found", "not found".to_string())
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
                stream.flush().expect("flush response");
            }
        });
        (format!("http://{addr}{expected_path}"), handle)
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn spawn_single_response_server(
        expected_path: &'static str,
        body: String,
    ) -> (String, thread::JoinHandle<()>) {
        spawn_response_server(expected_path, body, 1)
    }

    fn compile_path(
        path: &Path,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        let source = std::fs::read_to_string(path).expect("read source");
        let options = ModuleLoaderOptions {
            cache_dir: Some(test_cache_dir(path)),
            local_package_root: None,
            ..Default::default()
        };
        Compiler::compile_file_with_options(parse_program(&source), path, options)
    }

    #[test]
    fn compile_fuses_local_and_field_reads() {
        let program = parse_program("val (a, b) = (1, 2)\n");
        let (compiled, _) = Compiler::compile(program).expect("compile");
        let ops = compiled.main.ops();
        assert!(ops.contains(&Op::GetLocalField0), "{ops:?}");
        assert!(ops.contains(&Op::GetLocalField1), "{ops:?}");
        assert!(!ops.contains(&Op::GetField0), "{ops:?}");
    }

    #[test]
    fn compile_emits_make_cons_for_cons_expression() {
        let program = parse_program("val xs = 1 :: []\n");
        let (compiled, _) = Compiler::compile(program).expect("compile");
        assert!(compiled.main.ops().contains(&Op::MakeCons));
    }

    /// First instruction of the function named `name`.
    fn first_op_of(compiled: &CompiledProgram, name: &str) -> Op {
        let proto = compiled
            .functions
            .iter()
            .find(|f| f.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no function named {name}"));
        Op::try_from(proto.chunk.code[0]).expect("valid opcode")
    }

    #[test]
    fn local_fun_closure_kind_depends_on_recursion() {
        let (compiled, _) = Compiler::compile(parse_program(
            "fun rec_one n = let fun go 0 = 0 | go k = go (k - 1) in go n end\n\
             fun rec_two n = let fun ev 0 = true | ev k = od (k - 1)\n\
                                 and od 0 = false | od k = ev (k - 1) in ev n end\n\
             fun plain n = let fun inc k = k + n in inc 1 end\n\
             fun later n = let fun a k = k + 1\n\
                               fun b k = a (a k) in b n end\n\
             fun nested n = let fun go k = if k = 0 then 0 else (fn () => go (k - 1)) () in go n end\n",
        ))
        .expect("compile");
        // `go` only calls itself, with `TailCallSelf`, so it does not need
        // to capture itself.
        assert_eq!(first_op_of(&compiled, "rec_one"), Op::MakeClosure);
        let go = compiled
            .functions
            .iter()
            .find(|f| f.name.as_deref() == Some("go") && f.chunk.ops().contains(&Op::TailCallSelf));
        assert!(go.is_some(), "rec_one's go calls itself with TailCallSelf");
        // Mutual recursion, and a self-reference from a nested `fn`, still
        // capture the group.
        assert_eq!(first_op_of(&compiled, "rec_two"), Op::MakeRecClosures);
        assert_eq!(first_op_of(&compiled, "nested"), Op::MakeRecClosures);
        // No member refers to its own group: plain closures, as before.
        assert_eq!(first_op_of(&compiled, "plain"), Op::MakeClosure);
        assert_eq!(first_op_of(&compiled, "later"), Op::MakeClosure);
    }

    /// Long chains and deep nesting must end in a parse error or a compiled
    /// program, never a stack overflow, on a thread with the 8 MiB stack a
    /// main thread gets (`hiko check` runs there).
    #[test]
    fn deep_and_long_sources_do_not_overflow_the_stack() {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(|| {
                let parse_error = |src: &str| {
                    let tokens = Lexer::new(src, 0).tokenize().expect("lex");
                    Parser::new(tokens).parse_program().unwrap_err().message
                };
                let n = 5_000;
                for src in [
                    format!("val x = 1{}", " + 1".repeat(n)),
                    format!("val x = {}[]", "1 :: ".repeat(n)),
                    format!("fun f x = x\nval x = 1{}", " |> f".repeat(n)),
                    format!("val x = \"a\"{}", " ^ \"a\"".repeat(n)),
                    format!("val x = true{}", " andalso true".repeat(n)),
                    format!("fun f x = f\nval x = f{}", " 1".repeat(n)),
                    format!("fun f ({}[]) = 1", "x :: ".repeat(n)),
                    format!("fun f [{}_] = 1", "_, ".repeat(n)),
                    format!("val f : int{} = 1", " -> int".repeat(n)),
                    format!("val xs : int{} = []", " list".repeat(n)),
                ] {
                    assert_eq!(parse_error(&src), "nesting limit exceeded");
                }

                // List literals stay flat, so a long one is fine.
                let src = format!("val xs = [{}1]", "1, ".repeat(50_000));
                Compiler::compile(parse_program(&src)).expect("compile long list literal");

                // Just under the limit, every pass still fits the stack.
                let m = 250;
                for src in [
                    format!("val x = 1{}", " + 1".repeat(m)),
                    format!("val x = {}[]", "1 :: ".repeat(m)),
                    format!("fun f x = x\nval x = 1{}", " |> f".repeat(m)),
                    format!("val x = \"a\"{}", " ^ \"a\"".repeat(m)),
                    format!("val x = true{}", " andalso true".repeat(m)),
                    format!("val x = {}1{}", "(".repeat(m), ")".repeat(m)),
                    format!("val x = {}1", "fn x => ".repeat(m)),
                    format!("val x = {}1", "if true then 1 else ".repeat(m)),
                    format!("val x = {}1", "case 1 of _ => ".repeat(m)),
                    format!(
                        "val x = {}1{}",
                        "let val y = 1 in ".repeat(m),
                        " end".repeat(m)
                    ),
                    format!("val x = {}1{}", "[".repeat(m), "]".repeat(m)),
                    format!("val xs : int{} = []", " list".repeat(m)),
                    format!("val f : (int{}) -> int = fn _ => 1", " -> int".repeat(m)),
                ] {
                    Compiler::compile(parse_program(&src))
                        .unwrap_or_else(|e| panic!("{src:.40}...: {e:?}"));
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[cfg(feature = "builtin-stdio")]
    #[test]
    fn list_literal_compiles_like_the_cons_chain_it_stands_for() {
        let code = |src: &str| {
            let (compiled, _) = Compiler::compile(parse_program(src)).expect("compile");
            compiled.main.code.clone()
        };
        assert_eq!(
            code("val xs = [print \"a\", print \"b\", ()]\n"),
            code("val xs = print \"a\" :: print \"b\" :: () :: []\n")
        );
    }

    #[test]
    fn compile_direct_calls_only_unshadowed_global_functions() {
        let program = parse_program(
            "fun g x = x + 1\nval a = g 1\nfun h g = g 10\nval g = fn x => x * 2\nval b = g 2\n",
        );
        let (compiled, _) = Compiler::compile(program).expect("compile");
        let h = compiled
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("h"))
            .expect("function proto");
        // `a = g 1` calls the global function directly ...
        assert_eq!(
            compiled
                .main
                .ops()
                .iter()
                .filter(|&&op| op == Op::CallDirect)
                .count(),
            1,
            "expected exactly one CallDirect in main (for `g 1`, not `g 2`)"
        );
        // ... but the parameter `g` inside `h` shadows it.
        assert!(!h.chunk.ops().contains(&Op::CallDirect));
    }

    fn proto<'a>(compiled: &'a CompiledProgram, name: &str) -> &'a FunctionProto {
        compiled
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no function {name}"))
    }

    /// A top-level function's calls to itself, and between members of its
    /// `fun ... and ...` group, are direct.
    #[test]
    fn compile_recursive_top_level_calls_directly() {
        let program = parse_program(
            "fun fact n = if n = 0 then 1 else n * fact (n - 1)\n\
             fun loop (n, acc) = if n = 0 then acc else loop (n - 1, acc + 1)\n\
             fun even n = if n = 0 then true else odd (n - 1)\n\
             and odd n = if n = 0 then false else even (n - 1)\n",
        );
        let (compiled, _) = Compiler::compile(program).expect("compile");
        assert!(
            proto(&compiled, "fact")
                .chunk
                .ops()
                .contains(&Op::CallDirect)
        );
        assert!(
            proto(&compiled, "loop")
                .chunk
                .ops()
                .contains(&Op::TailCallDirect)
        );
        for name in ["even", "odd"] {
            let ops = proto(&compiled, name).chunk.ops();
            assert!(ops.contains(&Op::TailCallDirect), "{name}");
            assert!(!ops.contains(&Op::GetGlobal), "{name}");
        }
    }

    /// A function that takes one tuple gets a flat entry with a parameter
    /// per component; written-out tuple arguments call it without building
    /// a tuple, and the ordinary entry takes the tuple apart.
    #[test]
    fn compile_tuple_parameter_functions_flat() {
        let program = parse_program(
            "fun ack (m, n) = if m = 0 then n + 1 else if n = 0 then ack (m - 1, 1) else ack (m - 1, ack (m, n - 1))\n\
             val r = ack (2, 3)\n\
             val p = (2, 3)\n\
             val s = ack p\n",
        );
        let (compiled, _) = Compiler::compile(program).expect("compile");
        let protos: Vec<_> = compiled
            .functions
            .iter()
            .filter(|proto| proto.name.as_deref() == Some("ack"))
            .collect();
        let flat = protos.iter().find(|p| p.arity == 2).expect("flat entry");
        let wrapper = protos.iter().find(|p| p.arity == 1).expect("wrapper");
        let flat_ops = flat.chunk.ops();
        assert!(!flat_ops.contains(&Op::MakeTuple));
        assert!(flat_ops.contains(&Op::CallDirect) && flat_ops.contains(&Op::TailCallDirect));
        assert!(wrapper.chunk.ops().contains(&Op::TailCallDirect));
        // `ack (2, 3)` is flat and `ack p` goes through the wrapper, so the
        // only tuple main builds is `p`.
        let main = compiled.main.ops();
        let count = |op| main.iter().filter(|&&o| o == op).count();
        assert_eq!(count(Op::MakeTuple), 1);
        assert_eq!(count(Op::CallDirect), 2);
    }

    /// A body that rebinds the name stops the direct calls: the inner `fact`
    /// is a parameter.
    #[test]
    fn compile_recursive_call_respects_shadowing() {
        let program = parse_program("fun fact fact = fact 1\n");
        let (compiled, _) = Compiler::compile(program).expect("compile");
        let ops = proto(&compiled, "fact").chunk.ops();
        assert!(!ops.contains(&Op::CallDirect) && !ops.contains(&Op::TailCallDirect));
    }

    #[test]
    fn compile_deduplicates_constants_per_chunk() {
        let program = parse_program("val x = 1\nval y = x + x + 1\nval s = \"x\"\n");
        let (compiled, _) = Compiler::compile(program).expect("compile");
        // Int 1 and the string "x". Global names are in the global table,
        // not the constant pool.
        assert_eq!(compiled.main.constants.len(), 2);
        let globals: Vec<&str> = compiled.globals.iter().map(|g| &**g).collect();
        assert_eq!(globals, ["x", "y", "s"]);
    }

    #[test]
    fn compile_bool_pattern_branches_without_generic_eq() {
        let program = parse_program("fun f b = case b of true => 1 | false => 0\n");
        let (compiled, _) = Compiler::compile(program).expect("compile");
        let proto = compiled
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("f"))
            .expect("function proto");
        assert!(!proto.chunk.ops().contains(&Op::Eq));
        assert!(proto.chunk.ops().contains(&Op::Not));
    }

    #[test]
    fn compile_constructor_patterns_test_tags_with_one_jump() {
        let program = parse_program(
            "datatype t = A | B of int\n\
             fun f x = case x of A => 0 | B n => n\n\
             fun len xs = case xs of [] => 0 | _ :: rest => 1 + len rest\n",
        );
        let (compiled, _) = Compiler::compile(program).expect("compile");
        for name in ["f", "len"] {
            let proto = compiled
                .functions
                .iter()
                .find(|proto| proto.name.as_deref() == Some(name))
                .expect("function proto");
            let ops = proto.chunk.ops();
            // The tag test reads the scrutinee's local directly.
            assert!(ops.contains(&Op::JumpIfLocalNotTag), "{name}: {ops:?}");
            assert!(!ops.contains(&Op::JumpIfNotTag), "{name}: {ops:?}");
            assert!(!ops.contains(&Op::Eq), "{name}");
        }
    }

    #[test]
    fn compile_conditions_use_compare_and_branch() {
        let program = parse_program(
            "fun lt n = if n < 2 then 1 else 2\n\
             fun ge n = if n >= 2 then 1 else 2\n\
             fun eq n = if n = 0 then 1 else 2\n\
             fun ne n = if n <> 0 then 1 else 2\n\
             fun lit n = case n of 0 => 1 | _ => 2\n\
             fun flt (x : float) = if x < 1.0 then 1 else 2\n",
        );
        let (compiled, _) = Compiler::compile(program).expect("compile");
        let ops = |name: &str| {
            compiled
                .functions
                .iter()
                .find(|proto| proto.name.as_deref() == Some(name))
                .expect("function proto")
                .chunk
                .ops()
        };
        for (name, fused, unfused) in [
            ("lt", Op::JumpIfNotLtInt, Op::LtInt),
            ("ge", Op::JumpIfNotGeInt, Op::GeInt),
            ("eq", Op::JumpIfNotEq, Op::Eq),
            ("ne", Op::JumpIfEq, Op::Ne),
            ("lit", Op::JumpIfNotEq, Op::Eq),
        ] {
            let ops = ops(name);
            assert!(ops.contains(&fused), "{name}: {ops:?}");
            assert!(!ops.contains(&unfused), "{name}: {ops:?}");
            assert!(!ops.contains(&Op::JumpIfFalse), "{name}: {ops:?}");
        }
        // Float comparisons keep the compare and the jump.
        let ops = ops("flt");
        assert!(ops.contains(&Op::LtFloat), "{ops:?}");
        assert!(ops.contains(&Op::JumpIfFalse), "{ops:?}");
    }

    #[test]
    fn compile_records_max_locals_after_scope_cleanup() {
        let program = parse_program("fun f x = let val y = x in y end\nval answer = f 1\n");
        let (compiled, _) = Compiler::compile(program).expect("compile");
        let proto = compiled
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("f"))
            .expect("function proto");
        assert_eq!(proto.n_locals, 2);
    }

    #[cfg(feature = "builtin-path")]
    #[test]
    fn compile_accepts_internal_builtin_import_without_file_context() {
        let result = Compiler::compile(parse_program(
            "import __Builtin.Path\nval joined = BuiltinPath.join_raw (\"alpha\", \"beta\")\n",
        ));
        assert!(
            result.is_ok(),
            "expected in-memory compile to accept internal builtin import"
        );
    }

    #[test]
    fn compile_exports_opaque_ascription_values() {
        let source = "signature BOX = sig
  type t
  val make : int -> t
  val get : t -> int
end

structure Box :> BOX = struct
  type t = int
  fun make x = x
  fun get x = x
end

val result = Box.get (Box.make 41)
";
        let result = Compiler::compile(parse_program(source));
        assert!(
            result.is_ok(),
            "expected opaque module exports to typecheck, got {result:?}"
        );
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_fetches_over_http_and_reuses_cache() {
        let project_dir = unique_temp_dir("named-import-ok");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");
        let module_source = "structure List = struct\n  val forty_two = 42\nend\n".to_string();
        let module_hash = blake3_hex(module_source.as_bytes());
        let (list_url, server) = spawn_single_response_server("/modules/List.hml", module_source);
        let base_url = list_url.trim_end_matches("/modules/List.hml").to_string();
        let cache_path = cached_module_path(
            &entry_path,
            &format!("{base_url}/modules/List.hml"),
            &module_hash,
        );

        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Std]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Std.modules]\nList = \"blake3:{module_hash}\"\n"
            ),
        );
        write_file(
            &entry_path,
            "import Std.List\nval answer = List.forty_two\n",
        );

        let _ = std::fs::remove_file(&cache_path);

        let first = compile_path(&entry_path);
        server.join().expect("join server");
        assert!(first.is_ok(), "first compile should fetch remote module");

        let second = compile_path(&entry_path);
        assert!(second.is_ok(), "second compile should reuse cached module");
        assert!(
            cache_path.is_file(),
            "expected fetched module in the test cache"
        );

        let _ = std::fs::remove_file(&cache_path);
        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn manifest_lib_cache_holds_fetched_modules() {
        let project_dir = unique_temp_dir("manifest-lib-cache");
        write_file(
            &project_dir.join("hiko.toml"),
            "[defaults]\nlockfile = \"hiko.lock.toml\"\nlib_cache = \"modules-cache\"\n",
        );
        let entry_path = project_dir.join("main.hml");
        let module_source = "structure List = struct\n  val forty_two = 42\nend\n".to_string();
        let module_hash = blake3_hex(module_source.as_bytes());
        let (list_url, server) = spawn_single_response_server("/modules/List.hml", module_source);
        let base_url = list_url.trim_end_matches("/modules/List.hml").to_string();
        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Std]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Std.modules]\nList = \"blake3:{module_hash}\"\n"
            ),
        );
        write_file(
            &entry_path,
            "import Std.List\nval answer = List.forty_two\n",
        );
        let compile = || {
            let source = std::fs::read_to_string(&entry_path).expect("read source");
            // No embedder override: the manifest's lib_cache applies.
            Compiler::compile_file_with_options(
                parse_program(&source),
                &entry_path,
                ModuleLoaderOptions::default(),
            )
        };

        let first = compile();
        server.join().expect("join server");
        assert!(first.is_ok(), "first compile should fetch: {first:?}");
        let cached = project_dir.join("modules-cache").join(format!(
            "{}-{module_hash}.hml",
            blake3_hex(format!("{base_url}/modules/List.hml").as_bytes())
        ));
        assert!(
            cached.is_file(),
            "expected the module in the manifest's lib_cache"
        );

        // The server has gone, so this compile can only use the cache.
        let second = compile();
        assert!(
            second.is_ok(),
            "second compile should reuse the cache: {second:?}"
        );

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn loader_cache_dir_overrides_manifest_lib_cache() {
        let project_dir = unique_temp_dir("lib-cache-override");
        write_file(
            &project_dir.join("hiko.toml"),
            "[defaults]\nlockfile = \"hiko.lock.toml\"\nlib_cache = \"modules-cache\"\n",
        );
        let entry_path = project_dir.join("main.hml");
        let module_source = "structure List = struct\n  val forty_two = 42\nend\n";
        let module_hash = blake3_hex(module_source.as_bytes());
        // Nothing listens here: the module must come from the cache.
        let base_url = "http://127.0.0.1:9/Std-v0.1.0";
        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Std]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Std.modules]\nList = \"blake3:{module_hash}\"\n"
            ),
        );
        write_file(
            &entry_path,
            "import Std.List\nval answer = List.forty_two\n",
        );
        write_file(
            &cached_module_path(
                &entry_path,
                &format!("{base_url}/modules/List.hml"),
                &module_hash,
            ),
            module_source,
        );

        let result = compile_path(&entry_path);
        assert!(
            result.is_ok(),
            "the override cache has the module: {result:?}"
        );
        assert!(
            !project_dir.join("modules-cache").exists(),
            "the manifest's lib_cache must not be used"
        );

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn selective_named_import_exposes_values_and_constructors() {
        let project_dir = unique_temp_dir("selective-named-import");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");
        let module_source = "structure Result = struct\n  datatype ('a, 'e) result = Ok of 'a | Err of 'e\n  fun map f r = case r of Ok x => Ok (f x) | Err e => Err e\nend\n".to_string();
        let module_hash = blake3_hex(module_source.as_bytes());
        let (result_url, server) =
            spawn_single_response_server("/modules/Result.hml", module_source);
        let base_url = result_url
            .trim_end_matches("/modules/Result.hml")
            .to_string();
        let cache_path = cached_module_path(
            &entry_path,
            &format!("{base_url}/modules/Result.hml"),
            &module_hash,
        );

        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Std]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Std.modules]\nResult = \"blake3:{module_hash}\"\n"
            ),
        );
        write_file(
            &entry_path,
            "import Std.Result (Ok, Err, map)\nval x : (int, string) Result.result = Ok 41\nval y = map (fn n => n + 1) x\nval z = case y of Ok n => n | Err _ => 0\n",
        );

        let _ = std::fs::remove_file(&cache_path);
        let result = compile_path(&entry_path);
        server.join().expect("join server");
        assert!(
            result.is_ok(),
            "selective import should typecheck and compile, got {result:?}"
        );

        let _ = std::fs::remove_file(&cache_path);
        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn selective_named_import_rejects_collisions() {
        let project_dir = unique_temp_dir("selective-named-import-collision");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");
        let module_source =
            "structure Result = struct\n  datatype ('a, 'e) result = Ok of 'a | Err of 'e\nend\n"
                .to_string();
        let module_hash = blake3_hex(module_source.as_bytes());
        let (result_url, server) =
            spawn_single_response_server("/modules/Result.hml", module_source);
        let base_url = result_url
            .trim_end_matches("/modules/Result.hml")
            .to_string();
        let cache_path = cached_module_path(
            &entry_path,
            &format!("{base_url}/modules/Result.hml"),
            &module_hash,
        );

        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Std]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Std.modules]\nResult = \"blake3:{module_hash}\"\n"
            ),
        );
        write_file(
            &entry_path,
            "import Std.Result (Ok)\nimport Std.Result (Ok)\nval x = Ok 1\n",
        );

        let _ = std::fs::remove_file(&cache_path);
        let result = compile_path(&entry_path);
        server.join().expect("join server");
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(
                    message.contains("conflicts with an existing binding"),
                    "got: {message}"
                );
            }
            other => panic!("expected exposed import collision, got {other:?}"),
        }

        let _ = std::fs::remove_file(&cache_path);
        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_uses_project_lockfile_above_entry_file() {
        let project_dir = unique_temp_dir("named-import-project-lockfile");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("examples/main.hml");

        write_file(
            &project_dir.join("hiko.lock.toml"),
            r#"schema_version = 1

[packages.Std]
version = "0.1.0"
base_url = "http://127.0.0.1:8000/Std-v0.1.0"

[packages.Std.modules]
Prelude = "blake3:deadbeef"
"#,
        );
        write_file(&entry_path, "import Std.List\nval answer = 1\n");

        let result = compile_path(&entry_path);
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("module 'List'"));
                assert!(message.contains("not found"));
            }
            other => panic!("expected project lockfile to be used, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_rejects_manifest_lockfile_version_mismatch() {
        let project_dir = unique_temp_dir("named-import-manifest-version-mismatch");
        write_project_manifest_with_dependency(
            &project_dir,
            "Test",
            "0.2.0",
            "http://127.0.0.1:8000",
        );
        let entry_path = project_dir.join("main.hml");

        write_file(
            &project_dir.join("hiko.lock.toml"),
            r#"schema_version = 1

[packages.Test]
version = "0.1.0"
base_url = "http://127.0.0.1:8000/Test-v0.1.0"

[packages.Test.modules]
Answer = "blake3:deadbeef"
"#,
        );
        write_file(&entry_path, "import Test.Answer\nval answer = 1\n");

        let result = compile_path(&entry_path);
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("version mismatch"));
                assert!(message.contains("requires 0.2.0"));
                assert!(message.contains("resolves 0.1.0"));
            }
            other => panic!("expected manifest/lockfile mismatch, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_rejects_non_local_http_base_url() {
        let project_dir = unique_temp_dir("named-import-insecure-http");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");
        let module_source = "val answer = 42\n";
        let module_hash = blake3_hex(module_source.as_bytes());

        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Test]\nversion = \"0.1.0\"\nbase_url = \"http://example.com/Test-v0.1.0\"\n\n[packages.Test.modules]\nAnswer = \"blake3:{module_hash}\"\n"
            ),
        );
        write_file(&entry_path, "import Test.Answer\nval result = answer\n");

        let result = compile_path(&entry_path);
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("insecure base_url"));
                assert!(message.contains("https://"));
            }
            other => panic!("expected insecure base_url failure, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn remote_base_url_allows_only_https_or_local_http() {
        let check = |base_url: &str| {
            validate_remote_base_url("Std", base_url, Path::new("hiko.lock.toml")).is_ok()
        };
        assert!(check("https://modules.example.com/Std-v0.1.0"));
        assert!(check("http://127.0.0.1:8000/Std-v0.1.0"));
        assert!(check("http://localhost/Std-v0.1.0"));
        assert!(check("http://[::1]:8000/Std-v0.1.0"));
        assert!(!check("http://modules.example.com/Std-v0.1.0"));
        assert!(!check("http://127.0.0.1@modules.example.com/Std-v0.1.0"));
        assert!(!check("http://localhost.example.com/Std-v0.1.0"));
        // Text after the authority must not be mistaken for the host.
        assert!(!check("http://modules.example.com?@127.0.0.1/Std-v0.1.0"));
        assert!(!check("http://modules.example.com#@localhost/Std-v0.1.0"));
        assert!(!check("http://modules.example.com\\@127.0.0.1/Std-v0.1.0"));
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_rejects_oversized_remote_module() {
        let project_dir = unique_temp_dir("named-import-too-large");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");
        let module_source = format!("val answer = \"{}\"\n", "x".repeat(1024 * 1024));
        let module_hash = blake3_hex(module_source.as_bytes());
        let (answer_url, server) =
            spawn_single_response_server("/modules/Answer.hml", module_source);
        let base_url = answer_url
            .trim_end_matches("/modules/Answer.hml")
            .to_string();
        let cache_path = cached_module_path(
            &entry_path,
            &format!("{base_url}/modules/Answer.hml"),
            &module_hash,
        );

        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Test]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Test.modules]\nAnswer = \"blake3:{module_hash}\"\n"
            ),
        );
        write_file(
            &entry_path,
            "import Test.Answer\nval result = Answer.answer\n",
        );
        let _ = std::fs::remove_file(&cache_path);

        let result = compile_path(&entry_path);
        server.join().expect("join server");
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("cannot fetch named import"));
                assert!(message.contains("larger than request limit"));
            }
            other => panic!("expected oversized module failure, got {other:?}"),
        }
        assert!(!cache_path.exists());

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_rejects_local_use_inside_remote_module() {
        let project_dir = unique_temp_dir("named-import-remote-use");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");
        let module_source = "use \"../secret.hml\"\nval remote_answer = 42\n".to_string();
        let module_hash = blake3_hex(module_source.as_bytes());
        let (answer_url, server) =
            spawn_single_response_server("/modules/Answer.hml", module_source);
        let base_url = answer_url
            .trim_end_matches("/modules/Answer.hml")
            .to_string();
        let cache_path = cached_module_path(
            &entry_path,
            &format!("{base_url}/modules/Answer.hml"),
            &module_hash,
        );

        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Test]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Test.modules]\nAnswer = \"blake3:{module_hash}\"\n"
            ),
        );
        write_file(
            &entry_path,
            "import Test.Answer\nval result = remote_answer\n",
        );
        let _ = std::fs::remove_file(&cache_path);

        let result = compile_path(&entry_path);
        server.join().expect("join server");
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(
                    message.contains("not allowed in a remote module"),
                    "unexpected error: {message}"
                );
            }
            other => panic!("expected local use in remote module to fail, got {other:?}"),
        }
        let _ = std::fs::remove_file(&cache_path);
        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn write_pinned_module_project(name: &str, module_source: &str) -> (PathBuf, PathBuf, PathBuf) {
        let project_dir = unique_temp_dir(name);
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");
        let base_url = "http://127.0.0.1:9/Test-v0.1.0";
        let module_hash = blake3_hex(module_source.as_bytes());
        let cache_path = cached_module_path(
            &entry_path,
            &format!("{base_url}/modules/Answer.hml"),
            &module_hash,
        );
        write_file(&cache_path, module_source);
        write_file(&project_dir.join("helper.hml"), "val helper = 41\n");
        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Test]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Test.modules]\nAnswer = \"blake3:{module_hash}\"\n"
            ),
        );
        (project_dir, entry_path, cache_path)
    }

    /// Compiles `script` against a cached, pinned module `module_source`,
    /// with a `helper.hml` in the project, `../helper.hml` from the cache.
    #[cfg(feature = "loader-integrity-blake3")]
    fn compile_with_pinned_module(
        name: &str,
        module_source: &str,
        script: &str,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        let (project_dir, entry_path, _) = write_pinned_module_project(name, module_source);
        write_file(&entry_path, script);
        let result = compile_path(&entry_path);
        std::fs::remove_dir_all(&project_dir).ok();
        result
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn assert_nested_use_refused(
        result: Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError>,
    ) {
        match result {
            Err(CompileError::Codegen(message)) => assert!(
                message.contains("local use '../helper.hml' is not allowed in a remote module"),
                "unexpected error: {message}"
            ),
            Err(other) => panic!("expected the nested use to be refused, got {other:?}"),
            Ok(_) => panic!("expected the nested use to be refused, but it compiled"),
        }
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_use_in_let_refused_when_script_used_file() {
        assert_nested_use_refused(compile_with_pinned_module(
            "pinned-let-use-script-used",
            "val n = let use \"../helper.hml\" in helper end\n",
            "use \"./helper.hml\"\nimport Test.Answer\nval m = n\n",
        ));
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_use_in_let_refused_when_script_did_not_use_file() {
        assert_nested_use_refused(compile_with_pinned_module(
            "pinned-let-use-alone",
            "val n = let use \"../helper.hml\" in helper end\n",
            "import Test.Answer\nval m = n\n",
        ));
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_use_in_let_in_fun_refused() {
        for (name, script) in [
            (
                "pinned-fun-use-script-used",
                "use \"./helper.hml\"\nimport Test.Answer\nval m = f ()\n",
            ),
            ("pinned-fun-use-alone", "import Test.Answer\nval m = f ()\n"),
            (
                "pinned-fun-use-not-called",
                "use \"./helper.hml\"\nimport Test.Answer\nval m = 1\n",
            ),
        ] {
            assert_nested_use_refused(compile_with_pinned_module(
                name,
                "fun f () = let use \"../helper.hml\" in helper end\n",
                script,
            ));
        }
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_use_in_local_or_structure_refused() {
        for module in [
            "local use \"../helper.hml\" in val n = helper end",
            "structure Answer = struct use \"../helper.hml\" val n = helper end",
        ] {
            assert_nested_use_refused(compile_with_pinned_module(
                "pinned-nested-use",
                module,
                "use \"./helper.hml\"\nimport Test.Answer",
            ));
        }
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_use_refusal_precedes_parse_and_type_errors() {
        for module in [
            "val n = let use \"../helper.hml\" in helper end\nval broken =",
            "val broken = )\nuse \"../helper.hml\"",
            "val broken = unknown_name\nuse \"../helper.hml\"",
        ] {
            for script in [
                "import Test.Answer",
                "use \"./helper.hml\"\nimport Test.Answer",
            ] {
                assert_nested_use_refused(compile_with_pinned_module(
                    "pinned-use-error-precedence",
                    module,
                    script,
                ));
            }
        }
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_cannot_reuse_locally_loaded_cache_file() {
        let (project_dir, entry_path, cache_path) = write_pinned_module_project(
            "pinned-local-cache-use",
            "val n = let use \"../helper.hml\" in helper end",
        );
        // macOS temp paths can contain a symlink (/var -> /private/var).
        // Use the canonical path so a path mismatch cannot mask the bypass.
        let canonical = std::fs::canonicalize(cache_path).unwrap();
        let canonical_project = std::fs::canonicalize(&project_dir).unwrap();
        let entry_path = canonical_project.join(entry_path.file_name().unwrap());
        let relative_cache = canonical.strip_prefix(&canonical_project).unwrap();
        write_file(
            &entry_path,
            &format!(
                "use \"./helper.hml\"\nuse \"./{}\"\nimport Test.Answer\nval m = n",
                relative_cache.display()
            ),
        );
        for helper in [41, 999] {
            write_file(
                &project_dir.join("helper.hml"),
                &format!("val helper = {helper}"),
            );
            assert_nested_use_refused(compile_path(&entry_path));
        }
        std::fs::remove_dir_all(project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_transitive_use_refused() {
        let (project_dir, entry_path, _) = write_pinned_module_project(
            "pinned-transitive-use",
            "import Test.Other\nval answer = n",
        );
        let source = "val n = let use \"../helper.hml\" in helper end";
        let hash = blake3_hex(source.as_bytes());
        let lock_path = project_dir.join("hiko.lock.toml");
        let lock = std::fs::read_to_string(&lock_path).unwrap();
        write_file(&lock_path, &format!("{lock}Other = \"blake3:{hash}\"\n"));
        write_file(
            &cached_module_path(
                &entry_path,
                "http://127.0.0.1:9/Test-v0.1.0/modules/Other.hml",
                &hash,
            ),
            source,
        );
        write_file(&entry_path, "use \"./helper.hml\"\nimport Test.Answer");
        assert_nested_use_refused(compile_path(&entry_path));
        std::fs::remove_dir_all(project_dir).ok();
    }

    #[cfg(feature = "loader-embedded")]
    #[test]
    fn pinned_module_embedded_nested_use_precedes_parse_error() {
        let module = "val n = let use \"../helper.hml\" in helper end\nval broken =";
        let script = parse_program("use \"./helper.hml\"\nimport Pkg.Greet");
        let options = embedded(module, None, Some(vec![("helper.hml", "val helper = 41")]));
        let err = Compiler::compile_embedded(script, options).unwrap_err();
        assert!(
            format!("{err:?}")
                .contains("local use '../helper.hml' is not allowed in a remote module"),
            "{err:?}"
        );
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_without_use_compiles_and_type_errors_stand() {
        assert!(
            compile_with_pinned_module(
                "pinned-plain",
                "(* use \"../helper.hml\" *)\nval text = \"use \\\"../helper.hml\\\"\"\nval n = let val x = 1 in x end\n",
                "import Test.Answer\nval m = n\n",
            )
            .is_ok()
        );
        let result = compile_with_pinned_module(
            "pinned-unbound",
            "val n = unknown_name\n",
            "import Test.Answer\nval m = n\n",
        );
        assert!(
            matches!(&result, Err(e) if format!("{e:?}").contains("unbound variable")),
            "an unrelated error stands"
        );
        let result =
            compile_with_pinned_module("pinned-parse-error", "val n =", "import Test.Answer");
        assert!(
            matches!(&result, Err(e) if format!("{e:?}").contains("parse error")),
            "an unrelated parse error stands"
        );
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_rejects_bad_blake3() {
        let project_dir = unique_temp_dir("named-import-bad-hash");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");
        let module_source = "val remote_answer = 42\n".to_string();
        let (answer_url, server) =
            spawn_single_response_server("/modules/Answer.hml", module_source);
        let base_url = answer_url
            .trim_end_matches("/modules/Answer.hml")
            .to_string();
        let cache_path = cached_module_path(
            &entry_path,
            &format!("{base_url}/modules/Answer.hml"),
            "deadbeef",
        );

        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Test]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Test.modules]\nAnswer = \"blake3:deadbeef\"\n"
            ),
        );
        write_file(
            &entry_path,
            "import Test.Answer\nval result = remote_answer\n",
        );
        let _ = std::fs::remove_file(&cache_path);

        let result = compile_path(&entry_path);
        server.join().expect("join server");
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("failed BLAKE3 verification"));
            }
            other => panic!("expected BLAKE3 verification failure, got {other:?}"),
        }
        let _ = std::fs::remove_file(&cache_path);
        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn write_local_package_project(module_source: &str, locked_hash: &str) -> (PathBuf, PathBuf) {
        let project_dir = unique_temp_dir("named-import-local-root");
        write_project_manifest(&project_dir);
        write_file(
            &project_dir.join("libraries/Test-v0.1.0/modules/Answer.hml"),
            module_source,
        );
        // Nothing listens on the discard port; a fetch attempt would fail.
        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Test]\nversion = \"0.1.0\"\nbase_url = \"http://127.0.0.1:9/Test-v0.1.0\"\n\n[packages.Test.modules]\nAnswer = \"blake3:{locked_hash}\"\n"
            ),
        );
        let entry_path = project_dir.join("main.hml");
        write_file(
            &entry_path,
            "import Test.Answer\nval result = Answer.value\n",
        );
        (project_dir, entry_path)
    }

    #[cfg(feature = "loader-integrity-blake3")]
    fn compile_with_local_root(
        project_dir: &Path,
        entry_path: &Path,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        let source = std::fs::read_to_string(entry_path).expect("read source");
        let options = ModuleLoaderOptions {
            cache_dir: Some(project_dir.join(".test-lib-cache")),
            local_package_root: Some(project_dir.join("libraries")),
            ..Default::default()
        };
        Compiler::compile_file_with_options(parse_program(&source), entry_path, options)
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_from_local_package_root_cannot_use_files() {
        let source = "val n = let use \"../helper.hml\" in helper end";
        let (project_dir, entry_path) =
            write_local_package_project(source, &blake3_hex(source.as_bytes()));
        write_file(&entry_path, "import Test.Answer");
        assert_nested_use_refused(compile_with_local_root(&project_dir, &entry_path));
        std::fs::remove_dir_all(project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_reads_verified_module_from_local_package_root() {
        let module_source = "structure Answer = struct\n  val value = 42\nend\n";
        let (project_dir, entry_path) =
            write_local_package_project(module_source, &blake3_hex(module_source.as_bytes()));

        let result = compile_with_local_root(&project_dir, &entry_path);
        assert!(
            result.is_ok(),
            "expected local module to load, got {result:?}"
        );
        assert!(
            !project_dir.join(".test-lib-cache").exists(),
            "local package modules must not be copied into the cache"
        );

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_rejects_local_package_module_with_bad_blake3() {
        let module_source = "structure Answer = struct\n  val value = 42\nend\n";
        let (project_dir, entry_path) = write_local_package_project(module_source, "deadbeef");

        match compile_with_local_root(&project_dir, &entry_path) {
            Err(CompileError::Codegen(message)) => {
                assert!(
                    message.contains("failed BLAKE3 verification"),
                    "got: {message}"
                );
            }
            other => panic!("expected BLAKE3 verification failure, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn pinned_module_does_not_reuse_unverified_inference_or_codegen() {
        let project_dir = unique_temp_dir("pinned-local-inference");
        let path = project_dir.join("module.hml");
        write_file(&path, "val unchecked = 13");
        let canonical = std::fs::canonicalize(&path).unwrap();
        let local = ResolvedImport::File {
            canonical: canonical.clone(),
            display_name: "module.hml".into(),
        };
        let local_key = local.key();
        let mut compiler = Compiler::new(
            project_dir.clone(),
            None,
            HashSet::new(),
            ModuleLoaderOptions::default(),
            StringInterner::default(),
        );
        compiler.infer_import_source(local).unwrap();
        compiler.compile_import_source(local_key).unwrap();

        // A verified snapshot at the same canonical path must contribute its
        // own declarations, regardless of previously inferred/compiled bytes.
        let verified = ResolvedImport::VerifiedFile {
            canonical,
            display_name: "Test.Answer".into(),
            source: "val answer = 42".into(),
        };
        let verified_key = verified.key();
        compiler.infer_import_source(verified).unwrap();
        assert!(compiler.infer_ctx.has_value("answer"));
        compiler.compile_import_source(verified_key).unwrap();
        assert!(compiler.global_slots.contains_key("answer"));
        std::fs::remove_dir_all(project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn verified_file_import_uses_verified_source_without_rereading_disk() {
        let project_dir = unique_temp_dir("verified-import-source");
        let cache_path = project_dir.join("module.hml");
        write_file(&cache_path, "val answer = 0\n");
        let import = ResolvedImport::VerifiedFile {
            canonical: cache_path.clone(),
            display_name: "Test.Module".into(),
            source: "val answer = 42\n".into(),
        };
        write_file(&cache_path, "val answer = 13\n");

        let (source, base_dir) = import.load_source().expect("load verified source");
        assert_eq!(source.as_ref(), "val answer = 42\n");
        assert_eq!(base_dir, project_dir);

        std::fs::remove_dir_all(base_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_revalidates_cached_file_on_later_load() {
        let project_dir = unique_temp_dir("named-import-revalidate");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");
        let module_source = "structure Cache = struct\n  val value = 7\nend\n".to_string();
        let module_hash = blake3_hex(module_source.as_bytes());
        let (cache_url, server) =
            spawn_response_server("/modules/Cache.hml", module_source.clone(), 2);
        let base_url = cache_url.trim_end_matches("/modules/Cache.hml").to_string();
        let cache_path = cached_module_path(
            &entry_path,
            &format!("{base_url}/modules/Cache.hml"),
            &module_hash,
        );

        write_file(
            &project_dir.join("hiko.lock.toml"),
            &format!(
                "schema_version = 1\n\n[packages.Test]\nversion = \"0.1.0\"\nbase_url = \"{base_url}\"\n\n[packages.Test.modules]\nCache = \"blake3:{module_hash}\"\n"
            ),
        );
        write_file(&entry_path, "import Test.Cache\nval answer = Cache.value\n");
        let _ = std::fs::remove_file(&cache_path);

        assert!(
            compile_path(&entry_path).is_ok(),
            "initial compile should fetch"
        );
        assert_eq!(
            std::fs::read_to_string(&cache_path).expect("cached source"),
            module_source
        );

        std::fs::write(&cache_path, "structure Cache = struct val value = 0 end\n")
            .expect("poison cache");

        assert!(
            compile_path(&entry_path).is_ok(),
            "second compile should refetch after cache verification fails"
        );
        server.join().expect("join server");
        assert_eq!(
            std::fs::read_to_string(&cache_path).expect("repaired cache"),
            module_source
        );

        let _ = std::fs::remove_file(&cache_path);
        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_rejects_missing_package_in_lockfile() {
        let project_dir = unique_temp_dir("named-import-missing-package");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");

        write_file(
            &project_dir.join("hiko.lock.toml"),
            r#"schema_version = 1

[packages.Other]
version = "0.1.0"
base_url = "http://127.0.0.1:8000/Other-v0.1.0"

[packages.Other.modules]
List = "blake3:deadbeef"
"#,
        );
        write_file(&entry_path, "import Std.List\nval answer = 1\n");

        let result = compile_path(&entry_path);
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("package 'Std'"));
                assert!(message.contains("not found"));
            }
            other => panic!("expected missing package failure, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_rejects_missing_module_in_lockfile() {
        let project_dir = unique_temp_dir("named-import-missing-module");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");

        write_file(
            &project_dir.join("hiko.lock.toml"),
            r#"schema_version = 1

[packages.Std]
version = "0.1.0"
base_url = "http://127.0.0.1:8000/Std-v0.1.0"

[packages.Std.modules]
Prelude = "blake3:deadbeef"
"#,
        );
        write_file(&entry_path, "import Std.List\nval answer = 1\n");

        let result = compile_path(&entry_path);
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("module 'List'"));
                assert!(message.contains("package 'Std'"));
                assert!(message.contains("not found"));
            }
            other => panic!("expected missing module failure, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "builtin-path")]
    #[test]
    fn named_import_accepts_internal_builtin_package_without_lockfile() {
        let project_dir = unique_temp_dir("internal-builtin-import");
        let entry_path = project_dir.join("main.hml");

        write_file(
            &entry_path,
            "import __Builtin.Path\nval joined = BuiltinPath.join_raw (\"alpha\", \"beta\")\n",
        );

        let result = compile_path(&entry_path);
        assert!(
            result.is_ok(),
            "expected internal builtin import to compile"
        );

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "builtin-path")]
    #[test]
    fn malformed_lockfile_does_not_affect_local_use_compile() {
        let project_dir = unique_temp_dir("local-use-ignores-bad-lockfile");
        let entry_path = project_dir.join("main.hml");
        let helper_path = project_dir.join("helper.hml");

        write_file(
            &project_dir.join("hiko.lock.toml"),
            "this = [ is not valid toml",
        );
        write_file(&helper_path, "val local_answer = 7\n");
        write_file(
            &entry_path,
            "use \"./helper.hml\"\nval answer = local_answer\n",
        );

        let result = compile_path(&entry_path);
        assert!(
            result.is_ok(),
            "expected malformed lockfile to not affect local-only compilation"
        );

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[test]
    fn named_import_rejects_unknown_internal_builtin_module() {
        let project_dir = unique_temp_dir("unknown-internal-builtin");
        let entry_path = project_dir.join("main.hml");

        write_file(&entry_path, "import __Builtin.Nope\nval answer = 1\n");

        let result = compile_path(&entry_path);
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("unknown internal builtin module"));
                assert!(message.contains("__Builtin.Nope"));
            }
            other => panic!("expected unknown internal builtin failure, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    // `import Std.Json` reaches `__Builtin.Json`, so a default build, which
    // leaves out JSON, fails at compile time with this message.
    #[cfg(not(feature = "builtin-json"))]
    #[test]
    fn named_import_reports_missing_json_feature() {
        let project_dir = unique_temp_dir("disabled-json-builtin");
        let entry_path = project_dir.join("main.hml");

        write_file(&entry_path, "import __Builtin.Json\nval answer = 1\n");

        match compile_path(&entry_path) {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("__Builtin.Json"), "{message}");
                assert!(message.contains("not available in this build"), "{message}");
                assert!(message.contains("builtin-json"), "{message}");
            }
            other => panic!("expected disabled JSON builtin failure, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(not(feature = "builtin-hash"))]
    #[test]
    fn named_import_reports_missing_feature_for_disabled_internal_builtin() {
        let project_dir = unique_temp_dir("disabled-internal-builtin");
        let entry_path = project_dir.join("main.hml");

        write_file(
            &entry_path,
            "import __Builtin.Hash\nval digest = BuiltinHash.blake3_raw \"abc\"\n",
        );

        let result = compile_path(&entry_path);
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("__Builtin.Hash"));
                assert!(message.contains("not available in this build"));
                assert!(message.contains("builtin-hash"));
            }
            other => panic!("expected disabled internal builtin failure, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[cfg(feature = "loader-integrity-blake3")]
    #[test]
    fn named_import_rejects_reserved_package_name_in_lockfile() {
        let project_dir = unique_temp_dir("named-import-reserved-package");
        write_project_manifest(&project_dir);
        let entry_path = project_dir.join("main.hml");

        write_file(
            &project_dir.join("hiko.lock.toml"),
            r#"schema_version = 1

[packages.__Builtin]
version = "0.1.0"
base_url = "http://127.0.0.1:8000/__Builtin-v0.1.0"

[packages.__Builtin.modules]
Filesystem = "blake3:deadbeef"
"#,
        );
        write_file(&entry_path, "import Std.List\nval answer = 1\n");

        let result = compile_path(&entry_path);
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("reserved package name"));
                assert!(message.contains("__Builtin"));
            }
            other => panic!("expected reserved package validation failure, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[test]
    fn use_rejects_nonexistent_file() {
        let project_dir = unique_temp_dir("use-bad-path");
        let entry_path = project_dir.join("main.hml");

        write_file(
            &entry_path,
            "use \"./no_such_module.hml\"\nval answer = 1\n",
        );

        let result = compile_path(&entry_path);
        match result {
            Err(CompileError::Codegen(message)) => {
                assert!(
                    message.contains("cannot resolve local use"),
                    "expected 'cannot resolve local use' in error, got: {message}"
                );
            }
            other => panic!("expected compile error for missing use path, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[test]
    fn short_jump_beyond_i16_offset_compiles() {
        // A short forward jump whose absolute position exceeds i16::MAX must
        // still be encodable: only the relative offset has to fit in i16.
        let mut source = String::from("val b = true\n");
        for i in 0..6000 {
            source.push_str(&format!("val v{i} = {i}\n"));
        }
        source.push_str("val y = if b then 1 else 2\n");
        let (compiled, _) = Compiler::compile(parse_program(&source)).expect("compile");
        assert!(compiled.main.code.len() > i16::MAX as usize);
    }

    #[test]
    fn tuple_with_more_than_255_elements_is_rejected() {
        let elems = (0..256).map(|i| i.to_string()).collect::<Vec<_>>();
        let source = format!("val t = ({})\n", elems.join(", "));
        match Compiler::compile(parse_program(&source)) {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("tuple"), "unexpected error: {message}");
            }
            other => panic!("expected codegen error for 256-element tuple, got {other:?}"),
        }
    }

    /// Flags of the clauses of the first `InstallHandler` in a function
    /// chunk. A function whose body is a `handle` starts with one
    /// `MakeClosure` per provider clause, then `InstallHandler`.
    fn handler_clause_flags(source: &str) -> Vec<u8> {
        let (program, _) = Compiler::compile(parse_program(source)).expect("compile");
        let install_at = |code: &[u8]| {
            let mut ip = 0;
            while code.get(ip) == Some(&(Op::MakeClosure as u8)) {
                let n_captures = code[ip + 3] as usize;
                ip += 4 + 3 * n_captures;
            }
            (code.get(ip) == Some(&(Op::InstallHandler as u8))).then_some(ip)
        };
        let (code, start) = program
            .functions
            .iter()
            .find_map(|f| install_at(&f.chunk.code).map(|ip| (&f.chunk.code, ip)))
            .expect("a function chunk with InstallHandler after its provider closures");
        let n_clauses = u16::from_le_bytes([code[start + 1], code[start + 2]]) as usize;
        (0..n_clauses)
            .map(|i| code[start + 3 + i * crate::op::HANDLER_CLAUSE_BYTES + 2])
            .collect()
    }

    #[test]
    fn handler_clause_is_a_provider_when_every_path_resumes() {
        let effects = "effect A of int\neffect B of int\neffect C of int\neffect D of int\n";
        let flags = handler_clause_flags(&format!(
            "{effects}fun r u = handle perform A 1\n\
             with return x => x\n\
                | A n _ => n\n\
                | B n k => n\n\
                | C n k => resume k n\n\
                | D n k => if n > 0 then resume k n else (case n of 0 => resume k 1 | _ => resume k 2)"
        ));
        assert_eq!(
            flags,
            vec![0, 0, HANDLER_CLAUSE_PROVIDER, HANDLER_CLAUSE_PROVIDER]
        );
    }

    fn compile_error(source: &str) -> String {
        match Compiler::compile(parse_program(source)) {
            Err(CompileError::Type(TypeError { message, .. })) => message,
            other => panic!("expected a continuation error, got {other:?}"),
        }
    }

    #[test]
    fn continuation_is_not_a_value() {
        let prefix = "effect A of int\nval r = handle perform A 1 with return x => x\n";
        for clause in [
            // stored
            "| A n k => let val saved = k in resume k n end",
            // passed to a function
            "| A n k => let val _ = (fn f => 0) k in resume k n end",
            // captured by a closure
            "| A n k => (fn u => resume k u) n",
            // resumed twice
            "| A n k => resume k (resume k n)",
            // not in tail position
            "| A n k => 1 + resume k n",
            // only some paths resume
            "| A n k => if n > 0 then resume k n else 0",
            // inside a nested handle
            "| A n k => handle resume k n with return x => x",
        ] {
            let message = compile_error(&format!("{prefix}{clause}"));
            assert!(
                message.contains("continuation") || message.contains("resume k e"),
                "{clause}: unexpected error: {message}"
            );
        }
    }

    #[test]
    fn handler_clause_with_shadowed_continuation_does_not_resume() {
        // The inner `k` is a new binding, so the clause never refers to the
        // continuation.
        let flags = handler_clause_flags(
            "effect A of int\n\
             fun r u = handle perform A 1\n\
             with return x => x\n\
                | A n k => let val k = n + 1 in k end",
        );
        assert_eq!(flags, vec![0]);
    }

    #[test]
    fn closure_with_more_than_255_captures_is_rejected() {
        let mut source = String::from("val f = let\n");
        for i in 0..256 {
            source.push_str(&format!("  val c{i} = {i}\n"));
        }
        // Group the captures into small tuples to keep the expression shallow.
        let groups = (0..16)
            .map(|g| {
                let names = (0..16)
                    .map(|i| format!("c{}", g * 16 + i))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({names})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        source.push_str(&format!("in fn x => (x, {groups}) end\n"));
        match Compiler::compile(parse_program(&source)) {
            Err(CompileError::Codegen(message)) => {
                assert!(message.contains("capture"), "unexpected error: {message}");
            }
            other => panic!("expected codegen error for 256 captures, got {other:?}"),
        }
    }
}
