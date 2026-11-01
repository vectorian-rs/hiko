use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "loader-http")]
use std::time::Duration;

use hiko_builtin_meta::{internal_builtin_module, is_internal_builtin_package};
#[cfg(feature = "loader-integrity-blake3")]
use hiko_common::blake3_hex;
#[cfg(feature = "loader-http")]
use hiko_common::http_get_text_limited;
use hiko_syntax::ast::*;
use hiko_syntax::intern::{StringInterner, Symbol};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::span::Span;
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
    /// Module cache directory. `None` uses `$HOME/.hiko/lib-cache`.
    pub cache_dir: Option<PathBuf>,
    /// Local package directory checked before the cache and the network.
    /// A module `Pkg.Mod` locked at version `V` is read from
    /// `<root>/Pkg-vV/modules/Mod.hml` if that file exists (the layout of the
    /// repository's `libraries/` directory). A hash mismatch is an error.
    pub local_package_root: Option<PathBuf>,
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

#[cfg(feature = "loader-named-imports")]
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

#[cfg(feature = "loader-named-imports")]
fn load_import_loader_config(entry_file: &Path) -> Result<ImportLoaderConfig, CompileError> {
    let manifest_path = find_project_manifest_path(entry_file).ok_or_else(|| {
        CompileError::codegen(format!(
            "named imports require a project hiko.toml above '{}'",
            entry_file.display()
        ))
    })?;
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|e| {
        CompileError::codegen(format!(
            "cannot read project manifest '{}': {e}",
            manifest_path.display()
        ))
    })?;
    let manifest: ImportProjectManifest = toml::from_str(&manifest_text).map_err(|e| {
        CompileError::codegen(format!(
            "invalid project manifest '{}': {e}",
            manifest_path.display()
        ))
    })?;
    let lockfile = manifest
        .defaults
        .lockfile
        .as_deref()
        .unwrap_or("hiko.lock.toml");
    let lockfile_path = manifest_dir.join(lockfile);
    if !lockfile_path.is_file() {
        return Err(CompileError::codegen(format!(
            "project manifest '{}' references missing module lockfile '{}'",
            manifest_path.display(),
            lockfile_path.display()
        )));
    }
    Ok(ImportLoaderConfig {
        manifest_path,
        lockfile_path,
        manifest,
    })
}

#[cfg(feature = "loader-named-imports")]
fn default_module_cache_dir() -> Result<PathBuf, CompileError> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home).join(".hiko").join("lib-cache"));
    }
    Err(CompileError::codegen(
        "cannot determine module cache directory; HOME is not set",
    ))
}

#[cfg(feature = "loader-http")]
const REMOTE_MODULE_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(feature = "loader-http")]
const REMOTE_MODULE_MAX_BYTES: u64 = 1024 * 1024;

#[cfg(feature = "loader-http")]
fn fetch_remote_module(url: &str, module_name: &str) -> Result<String, CompileError> {
    http_get_text_limited(url, REMOTE_MODULE_TIMEOUT, REMOTE_MODULE_MAX_BYTES).map_err(|e| {
        CompileError::codegen(format!(
            "cannot fetch named import '{module_name}' from '{url}': {e}"
        ))
    })
}

#[cfg(feature = "loader-integrity-blake3")]
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

#[cfg(all(
    feature = "loader-named-imports",
    feature = "loader-http",
    feature = "loader-integrity-blake3"
))]
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

#[cfg(feature = "loader-named-imports")]
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

#[cfg(feature = "loader-named-imports")]
fn validate_lockfile_against_manifest(
    lockfile: &ImportLockfile,
    lockfile_path: &Path,
    config: &ImportLoaderConfig,
) -> Result<(), CompileError> {
    let dependencies = &config.manifest.dependencies;
    if dependencies.is_empty() {
        return Ok(());
    }

    for package_name in lockfile.packages.keys() {
        if !dependencies.contains_key(package_name) {
            return Err(CompileError::codegen(format!(
                "package '{package_name}' in '{}' is not declared in project manifest '{}'",
                lockfile_path.display(),
                config.manifest_path.display()
            )));
        }
    }

    for (package_name, dependency) in dependencies {
        let package = lockfile.packages.get(package_name).ok_or_else(|| {
            CompileError::codegen(format!(
                "dependency '{package_name}' declared in project manifest '{}' is missing from module lockfile '{}'",
                config.manifest_path.display(),
                lockfile_path.display()
            ))
        })?;
        if package.version != dependency.version {
            return Err(CompileError::codegen(format!(
                "dependency '{package_name}' version mismatch: project manifest '{}' requires {}, but module lockfile '{}' resolves {}",
                config.manifest_path.display(),
                dependency.version,
                lockfile_path.display(),
                package.version
            )));
        }
        let registry = config
            .manifest
            .registries
            .get(&dependency.registry)
            .ok_or_else(|| {
                CompileError::codegen(format!(
                    "dependency '{package_name}' references unknown registry '{}' in project manifest '{}'",
                    dependency.registry,
                    config.manifest_path.display()
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
                "dependency '{package_name}' source mismatch: project manifest '{}' expects '{}', but module lockfile '{}' resolves '{}'",
                config.manifest_path.display(),
                expected_base_url,
                lockfile_path.display(),
                package.base_url
            )));
        }
    }

    Ok(())
}

#[cfg(feature = "loader-named-imports")]
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

#[cfg(feature = "loader-integrity-blake3")]
fn normalize_blake3(hash: &str) -> String {
    hash.trim()
        .strip_prefix("blake3:")
        .unwrap_or(hash.trim())
        .to_ascii_lowercase()
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

#[cfg_attr(not(feature = "loader-named-imports"), allow(dead_code))]
#[derive(Debug, Clone)]
struct ImportLoaderConfig {
    manifest_path: PathBuf,
    lockfile_path: PathBuf,
    manifest: ImportProjectManifest,
}

#[cfg_attr(not(feature = "loader-named-imports"), allow(dead_code))]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportManifestDefaults {
    lockfile: Option<String>,
    #[serde(default)]
    #[serde(rename = "policy")]
    _policy: Option<String>,
}

#[cfg_attr(not(feature = "loader-named-imports"), allow(dead_code))]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRegistry {
    url: String,
}

#[cfg_attr(not(feature = "loader-named-imports"), allow(dead_code))]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportDependency {
    version: String,
    registry: String,
}

#[cfg_attr(not(feature = "loader-named-imports"), allow(dead_code))]
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

#[cfg_attr(not(feature = "loader-named-imports"), allow(dead_code))]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportLockfile {
    schema_version: u32,
    #[serde(default)]
    packages: HashMap<String, LockedPackage>,
}

#[cfg_attr(
    not(all(feature = "loader-named-imports", feature = "loader-http")),
    allow(dead_code)
)]
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
    Synthetic(&'static str),
}

#[derive(Debug)]
enum ResolvedImport {
    File {
        canonical: PathBuf,
        display_name: String,
    },
    #[cfg(all(
        feature = "loader-named-imports",
        feature = "loader-http",
        feature = "loader-integrity-blake3"
    ))]
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
}

impl ResolvedImport {
    fn key(&self) -> ImportKey {
        match self {
            Self::File { canonical, .. } => ImportKey::File(canonical.clone()),
            #[cfg(all(
                feature = "loader-named-imports",
                feature = "loader-http",
                feature = "loader-integrity-blake3"
            ))]
            Self::VerifiedFile { canonical, .. } => ImportKey::File(canonical.clone()),
            Self::Synthetic { import_name, .. } => ImportKey::Synthetic(import_name),
        }
    }

    /// Whether the source was fetched from a remote package.
    fn is_remote(&self) -> bool {
        match self {
            #[cfg(all(
                feature = "loader-named-imports",
                feature = "loader-http",
                feature = "loader-integrity-blake3"
            ))]
            Self::VerifiedFile { .. } => true,
            _ => false,
        }
    }

    fn display_name(&self) -> &str {
        match self {
            Self::File { display_name, .. } => display_name,
            #[cfg(all(
                feature = "loader-named-imports",
                feature = "loader-http",
                feature = "loader-integrity-blake3"
            ))]
            Self::VerifiedFile { display_name, .. } => display_name,
            Self::Synthetic { display_name, .. } => display_name,
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
            #[cfg(all(
                feature = "loader-named-imports",
                feature = "loader-http",
                feature = "loader-integrity-blake3"
            ))]
            Self::VerifiedFile {
                canonical, source, ..
            } => {
                let base_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
                Ok((Cow::Borrowed(source.as_str()), base_dir))
            }
            Self::Synthetic {
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
    #[cfg_attr(
        not(all(
            feature = "loader-named-imports",
            feature = "loader-http",
            feature = "loader-integrity-blake3"
        )),
        allow(dead_code)
    )]
    entry_file: Option<PathBuf>,
    /// Imports currently being loaded (for cycle detection).
    loading_imports: HashSet<ImportKey>,
    /// True while inferring a remote module or anything it imports. Local
    /// `use` is rejected there: a hash-pinned module must not pull in
    /// unverified files relative to the shared module cache.
    in_remote_import: bool,
    /// Imports whose type inference is complete.
    inferred_imports: HashSet<ImportKey>,
    /// Imports whose codegen is complete.
    compiled_imports: HashSet<ImportKey>,
    /// Cached desugared programs for imported modules (used between passes).
    imported_programs: HashMap<ImportKey, CachedImportedProgram>,
    /// Named module lockfile loaded lazily from the entry program's project root.
    #[cfg_attr(
        not(all(
            feature = "loader-named-imports",
            feature = "loader-http",
            feature = "loader-integrity-blake3"
        )),
        allow(dead_code)
    )]
    import_lockfile: Option<ImportLockfile>,
    /// Cache directory for fetched remote modules.
    #[cfg_attr(
        not(all(
            feature = "loader-named-imports",
            feature = "loader-http",
            feature = "loader-integrity-blake3"
        )),
        allow(dead_code)
    )]
    module_cache_dir: Option<PathBuf>,
    /// Caller-supplied overrides for named-import resolution.
    #[cfg_attr(
        not(all(
            feature = "loader-named-imports",
            feature = "loader-http",
            feature = "loader-integrity-blake3"
        )),
        allow(dead_code)
    )]
    loader_options: ModuleLoaderOptions,
    /// Global name -> proto_idx of the capture-free function it is currently
    /// bound to, for `CallDirect`. Every global rebinding clears the entry.
    global_protos: HashMap<String, u16>,
    /// Proto index the next `push_new_function` compiles into (see
    /// `reserve_proto`).
    pending_reserved_proto: Option<u16>,
    /// Global name -> flat entry of a top-level function that takes one
    /// tuple (see `flat_tuple_width`). Valid only while `global_protos` still
    /// maps the name to the entry's wrapper.
    global_flat_protos: HashMap<String, FlatEntry>,
    /// Source name -> VM global name of the top-level binding in scope. A
    /// binding that shadows an earlier one, or a builtin, gets a fresh name
    /// (`x$1`, `x$2`, ...), so code compiled earlier keeps its binding.
    /// `$` cannot appear in a source identifier.
    global_bindings: HashMap<String, String>,
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
            in_remote_import: false,
            inferred_imports: HashSet::new(),
            compiled_imports: HashSet::new(),
            imported_programs: HashMap::new(),
            import_lockfile: None,
            module_cache_dir: None,
            loader_options,
            global_protos: HashMap::new(),
            pending_reserved_proto: None,
            global_flat_protos: HashMap::new(),
            global_bindings: HashMap::new(),
            global_slots: HashMap::new(),
            global_names: Vec::new(),
            infer_ctx: InferCtx::new(),
            interner,
        };
        c.infer_ctx.interner = c.interner.clone();

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
        let taken = self.global_bindings.contains_key(name)
            || hiko_builtin_meta::builtin_meta(name).is_some();
        let global = if taken {
            let mut n = 1;
            loop {
                let candidate = format!("{name}${n}");
                if !self.global_bindings.values().any(|g| *g == candidate) {
                    break candidate;
                }
                n += 1;
            }
        } else {
            name.to_string()
        };
        self.global_bindings.insert(name.to_string(), global);
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
        self.emit_get_local(slot);
        self.emit_get_field(idx)?;
        Ok(self.add_local("_tmp".to_string()))
    }

    /// Emit tag check: GetLocal slot, JumpIfNotTag tag → fail.
    fn emit_tag_check(&mut self, slot: u16, tag: u16, fail_jumps: &mut Vec<(usize, usize)>) {
        self.emit_get_local(slot);
        let jmp = self.chunk().emit_tag_jump(tag);
        fail_jumps.push((jmp, self.ctx().locals.len()));
    }

    /// Emit scalar comparison: GetLocal slot, push value, eq_op, JumpIfFalse → fail.
    fn emit_scalar_check(
        &mut self,
        slot: u16,
        value: Constant,
        eq_op: Op,
        fail_jumps: &mut Vec<(usize, usize)>,
    ) -> Result<(), CompileError> {
        self.emit_get_local(slot);
        self.emit_constant(value)?;
        self.emit(eq_op);
        let jmp = self.emit_jump(Op::JumpIfFalse);
        fail_jumps.push((jmp, self.ctx().locals.len()));
        Ok(())
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
        if self.in_remote_import {
            return Err(CompileError::codegen(format!(
                "local use '{path}' is not allowed in a remote module; use a named import instead"
            )));
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
        if is_internal_builtin_package(package_name) {
            return self.resolve_internal_builtin_import(module_name, leaf_module);
        }
        self.resolve_external_named_import(module_name, package_name, leaf_module)
    }

    #[cfg(not(feature = "loader-named-imports"))]
    fn resolve_external_named_import(
        &mut self,
        module_name: &str,
        _package_name: &str,
        _leaf_module: &str,
    ) -> Result<ResolvedImport, CompileError> {
        Err(CompileError::codegen(format!(
            "named remote import '{module_name}' is not available in this build; enable cargo feature 'loader-named-imports'"
        )))
    }

    #[cfg(all(feature = "loader-named-imports", not(feature = "loader-http")))]
    fn resolve_external_named_import(
        &mut self,
        module_name: &str,
        _package_name: &str,
        _leaf_module: &str,
    ) -> Result<ResolvedImport, CompileError> {
        Err(CompileError::codegen(format!(
            "named remote import '{module_name}' is not available in this build; enable cargo feature 'loader-http'"
        )))
    }

    #[cfg(all(
        feature = "loader-named-imports",
        feature = "loader-http",
        not(feature = "loader-integrity-blake3")
    ))]
    fn resolve_external_named_import(
        &mut self,
        module_name: &str,
        _package_name: &str,
        _leaf_module: &str,
    ) -> Result<ResolvedImport, CompileError> {
        Err(CompileError::codegen(format!(
            "named remote import '{module_name}' requires BLAKE3 verification support; enable cargo feature 'loader-integrity-blake3'"
        )))
    }

    #[cfg(all(
        feature = "loader-named-imports",
        feature = "loader-http",
        feature = "loader-integrity-blake3"
    ))]
    fn resolve_external_named_import(
        &mut self,
        module_name: &str,
        package_name: &str,
        leaf_module: &str,
    ) -> Result<ResolvedImport, CompileError> {
        let local_package_root = self.loader_options.local_package_root.clone();
        let (lockfile, cache_dir) = self.ensure_import_loader(module_name)?;
        let package = lockfile.packages.get(package_name).ok_or_else(|| {
            CompileError::codegen(format!(
                "package '{package_name}' for named import '{module_name}' not found in hiko.lock.toml"
            ))
        })?;
        let expected_hash = package.modules.get(leaf_module).ok_or_else(|| {
            CompileError::codegen(format!(
                "module '{leaf_module}' in package '{package_name}' for named import '{module_name}' not found in hiko.lock.toml"
            ))
        })?;
        let expected_hash = normalize_blake3(expected_hash);

        if let Some(root) = &local_package_root {
            let local_path = root
                .join(format!("{package_name}-v{}", package.version))
                .join("modules")
                .join(format!("{leaf_module}.hml"));
            if local_path.is_file() {
                let source = std::fs::read_to_string(&local_path).map_err(|e| {
                    CompileError::codegen(format!(
                        "cannot read local module '{}': {e}",
                        local_path.display()
                    ))
                })?;
                verify_blake3(
                    source.as_bytes(),
                    &expected_hash,
                    &format!(
                        "local named import '{module_name}' ({})",
                        local_path.display()
                    ),
                )?;
                let canonical = std::fs::canonicalize(&local_path).unwrap_or(local_path);
                return Ok(ResolvedImport::VerifiedFile {
                    canonical,
                    display_name: module_name.to_string(),
                    source,
                });
            }
        }

        std::fs::create_dir_all(cache_dir).map_err(|e| {
            CompileError::codegen(format!(
                "cannot create module cache '{}': {e}",
                cache_dir.display()
            ))
        })?;
        let module_url = format!(
            "{}/modules/{leaf_module}.hml",
            package.base_url.trim_end_matches('/')
        );
        let url_hash = blake3_hex(module_url.as_bytes());
        let cache_path = cache_dir.join(format!("{url_hash}-{expected_hash}.hml"));

        if cache_path.exists() {
            let cached = std::fs::read_to_string(&cache_path).map_err(|e| {
                CompileError::codegen(format!(
                    "cannot read cached module '{}': {e}",
                    cache_path.display()
                ))
            })?;
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

        let source = fetch_remote_module(&module_url, module_name)?;
        verify_blake3(
            source.as_bytes(),
            &expected_hash,
            &format!("named import '{module_name}'"),
        )?;
        write_verified_cache_file(&cache_path, source.as_bytes())?;
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

    #[cfg(all(
        feature = "loader-named-imports",
        feature = "loader-http",
        feature = "loader-integrity-blake3"
    ))]
    fn ensure_import_loader(
        &mut self,
        module_name: &str,
    ) -> Result<(&ImportLockfile, &PathBuf), CompileError> {
        if self.import_lockfile.is_none() || self.module_cache_dir.is_none() {
            let entry_file = self.entry_file.as_ref().ok_or_else(|| {
                CompileError::codegen(format!(
                    "named import '{module_name}' requires file-based compilation context"
                ))
            })?;
            let loader_config = load_import_loader_config(entry_file)?;
            let lockfile_path = &loader_config.lockfile_path;
            let lockfile_text = std::fs::read_to_string(lockfile_path).map_err(|e| {
                CompileError::codegen(format!(
                    "cannot read module lockfile '{}': {e}",
                    lockfile_path.display()
                ))
            })?;
            let lockfile: ImportLockfile = toml::from_str(&lockfile_text).map_err(|e| {
                CompileError::codegen(format!(
                    "invalid module lockfile '{}': {e}",
                    lockfile_path.display()
                ))
            })?;
            validate_lockfile(&lockfile, lockfile_path)?;
            validate_lockfile_against_manifest(&lockfile, lockfile_path, &loader_config)?;
            self.import_lockfile = Some(lockfile);
            self.module_cache_dir = match &self.loader_options.cache_dir {
                Some(dir) => Some(dir.clone()),
                None => Some(default_module_cache_dir()?),
            };
        }
        let lockfile = self.import_lockfile.as_ref().unwrap();
        let cache_dir = self.module_cache_dir.as_ref().unwrap();
        Ok((lockfile, cache_dir))
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
                for d in locals {
                    self.infer_decl_pass(d)?;
                }
                for d in body {
                    self.infer_decl_pass(d)?;
                }
                Ok(())
            }
            _ => {
                self.infer_ctx.infer_decl(decl)?;
                Ok(())
            }
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

        if self.inferred_imports.contains(&key) {
            return Ok(());
        }

        let (source, imported_base_dir) = import.load_source()?;
        let tokens = Lexer::new(&source, 0).tokenize().map_err(|e| {
            CompileError::codegen(format!("lex error in '{display_name}': {}", e.message))
        })?;
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
        let old_in_remote = self.in_remote_import;
        self.in_remote_import |= is_remote;

        let result = desugared.iter().try_for_each(|d| self.infer_decl_pass(d));

        self.in_remote_import = old_in_remote;
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
                    let arity = matches!(&scheme.ty, Type::Arrow(_, _)) as u8;
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
        for decl in &decls {
            self.compile_decl(decl)?;
        }
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
                    self.emit_get_local(tup_slot);
                    self.emit_get_field(i)?;
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
            members.push(compile_member(self, i)?);
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
                self.emit_scalar_check(slot, Constant::Int(*n), Op::Eq, fail_jumps)?;
            }
            PatKind::FloatLit(f) => {
                self.emit_scalar_check(slot, Constant::Float(*f), Op::Eq, fail_jumps)?;
            }
            PatKind::WordLit(w) => {
                self.emit_scalar_check(slot, Constant::Word(*w), Op::Eq, fail_jumps)?;
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
                self.emit_scalar_check(slot, Constant::String(s.clone()), Op::Eq, fail_jumps)?;
            }
            PatKind::CharLit(c) => {
                self.emit_scalar_check(slot, Constant::Char(*c), Op::Eq, fail_jumps)?;
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
                self.compile_expr(cond)?;
                let else_jump = self.emit_jump(Op::JumpIfFalse);
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn parse_program(source: &str) -> Program {
        let tokens = Lexer::new(source, 0).tokenize().expect("lex");
        Parser::new(tokens).parse_program().expect("parse")
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

    fn write_project_manifest(project_dir: &Path) {
        write_file(
            &project_dir.join("hiko.toml"),
            "[defaults]\nlockfile = \"hiko.lock.toml\"\n",
        );
    }

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

    fn cached_module_path(entry_path: &Path, url: &str, hash: &str) -> PathBuf {
        test_cache_dir(entry_path).join(format!("{}-{}.hml", blake3_hex(url.as_bytes()), hash))
    }

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
        };
        Compiler::compile_file_with_options(parse_program(&source), path, options)
    }

    #[test]
    fn compile_emits_quick_local_and_field_opcodes() {
        let program = parse_program("val (a, b) = (1, 2)\n");
        let (compiled, _) = Compiler::compile(program).expect("compile");
        assert!(compiled.main.ops().contains(&Op::GetLocal0));
        assert!(compiled.main.ops().contains(&Op::GetField0));
        assert!(compiled.main.ops().contains(&Op::GetField1));
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
                               fun b k = a (a k) in b n end\n",
        ))
        .expect("compile");
        assert_eq!(first_op_of(&compiled, "rec_one"), Op::MakeRecClosures);
        assert_eq!(first_op_of(&compiled, "rec_two"), Op::MakeRecClosures);
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
            assert!(proto.chunk.ops().contains(&Op::JumpIfNotTag), "{name}");
            assert!(!proto.chunk.ops().contains(&Op::Eq), "{name}");
        }
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

    #[cfg(feature = "loader-named-imports")]
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

    fn compile_with_local_root(
        project_dir: &Path,
        entry_path: &Path,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        let source = std::fs::read_to_string(entry_path).expect("read source");
        let options = ModuleLoaderOptions {
            cache_dir: Some(project_dir.join(".test-lib-cache")),
            local_package_root: Some(project_dir.join("libraries")),
        };
        Compiler::compile_file_with_options(parse_program(&source), entry_path, options)
    }

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
