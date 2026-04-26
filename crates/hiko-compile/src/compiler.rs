use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hiko_builtin_meta::{internal_builtin_module, is_internal_builtin_package};
#[cfg(feature = "loader-integrity-blake3")]
use hiko_common::blake3_hex;
#[cfg(feature = "loader-http")]
use hiko_common::http_get_text;
use hiko_syntax::ast::*;
use hiko_syntax::intern::{StringInterner, Symbol};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_types::infer::{InferCtx, TypeError};
use serde::Deserialize;

use crate::chunk::{Chunk, CompiledProgram, Constant, FunctionProto};
use crate::op::Op;

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
fn find_lockfile_path(entry_file: &Path) -> Option<PathBuf> {
    let mut dir = entry_file.parent();
    while let Some(current) = dir {
        let candidate = current.join("hiko.lock.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = current.parent();
    }
    None
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
fn fetch_remote_module(url: &str, module_name: &str) -> Result<String, CompileError> {
    http_get_text(url).map_err(|e| {
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
        if package_name.trim().is_empty() {
            return Err(CompileError::codegen(format!(
                "package entry in '{}' has an empty name",
                path.display()
            )));
        }
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
}

struct UpvalueDesc {
    is_local: bool,
    index: u16,
}

struct FuncCtx {
    chunk: Chunk,
    locals: Vec<Local>,
    upvalues: Vec<UpvalueDesc>,
    scope_depth: u32,
    arity: u8,
    name: Option<String>,
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
            Self::Synthetic { import_name, .. } => ImportKey::Synthetic(import_name),
        }
    }

    fn display_name(&self) -> &str {
        match self {
            Self::File { display_name, .. } | Self::Synthetic { display_name, .. } => display_name,
        }
    }

    fn load_source(&self) -> Result<(Cow<'static, str>, PathBuf), CompileError> {
        match self {
            Self::File { canonical, .. } => {
                let source = std::fs::read_to_string(canonical).map_err(|e| {
                    CompileError::codegen(format!("cannot read '{}': {e}", canonical.display()))
                })?;
                let base_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
                Ok((Cow::Owned(source), base_dir))
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
    /// Global function name -> proto_idx (for direct calls).
    global_protos: HashMap<String, u16>,
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
        let base_dir = file_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let canonical =
            std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
        let mut loading = HashSet::new();
        loading.insert(ImportKey::File(canonical.clone()));
        Self::compile_with_ctx(program, base_dir, Some(canonical), loading)
    }

    /// Compile a program without a file context (e.g., from a string).
    pub fn compile(
        program: Program,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        Self::compile_with_ctx(program, PathBuf::from("."), None, HashSet::new())
    }

    fn compile_with_ctx(
        program: Program,
        base_dir: PathBuf,
        entry_file: Option<PathBuf>,
        loading_imports: HashSet<ImportKey>,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        // Desugar and constant-fold
        let program = hiko_syntax::desugar::desugar_program(program);
        let program = hiko_syntax::constfold::fold_program(program);
        let interner = program.interner;
        let desugared = program.decls;

        let mut c = Compiler {
            functions: Vec::new(),
            func_stack: vec![FuncCtx {
                chunk: Chunk::default(),
                locals: Vec::new(),
                upvalues: Vec::new(),
                scope_depth: 0,
                arity: 0,
                name: None,
            }],
            constructor_tags: HashMap::new(),
            constructor_arities: HashMap::new(),
            effect_tags: HashMap::new(),
            next_effect_tag: 0,
            base_dir,
            entry_file,
            loading_imports,
            inferred_imports: HashSet::new(),
            compiled_imports: HashSet::new(),
            imported_programs: HashMap::new(),
            import_lockfile: None,
            module_cache_dir: None,
            global_protos: HashMap::new(),
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
                functions: Arc::<[FunctionProto]>::from(c.functions),
                effects: Arc::<[crate::chunk::EffectMeta]>::from(
                    c.effect_tags
                        .into_iter()
                        .map(|(name, tag)| crate::chunk::EffectMeta { name, tag })
                        .collect::<Vec<_>>(),
                ),
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
        let idx = self
            .chunk()
            .add_constant(c)
            .map_err(CompileError::codegen)?;
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
    fn add_string_constant(&mut self, s: &str) -> Result<u16, CompileError> {
        self.chunk()
            .add_constant(Constant::String(s.to_string()))
            .map_err(CompileError::codegen)
    }

    fn push_new_function(&mut self, name: Option<String>) {
        self.func_stack.push(FuncCtx {
            chunk: Chunk::default(),
            locals: Vec::new(),
            upvalues: Vec::new(),
            scope_depth: 0,
            arity: 1,
            name,
        });
    }

    // ── Variable resolution ──────────────────────────────────────────

    fn resolve_local(&self, name: &str) -> Option<u16> {
        for (i, local) in self.ctx().locals.iter().enumerate().rev() {
            if local.name == name {
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
                return Some(self.add_upvalue(func_idx, true, i as u16));
            }
        }
        if let Some(uv_idx) = self.resolve_upvalue_at(name, func_idx - 1) {
            return Some(self.add_upvalue(func_idx, false, uv_idx));
        }
        None
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

    fn emit_get_var(&mut self, name: &str) -> Result<(), CompileError> {
        if let Some(slot) = self.resolve_local(name) {
            self.emit(Op::GetLocal);
            self.emit_u16(slot);
        } else if let Some(idx) = self.resolve_upvalue(name) {
            self.emit(Op::GetUpvalue);
            self.emit_u16(idx);
        } else {
            let c = self.add_string_constant(name)?;
            self.emit(Op::GetGlobal);
            self.emit_u16(c);
        }
        Ok(())
    }

    fn emit_set_global(&mut self, name: &str) -> Result<(), CompileError> {
        let c = self.add_string_constant(name)?;
        self.emit(Op::SetGlobal);
        self.emit_u16(c);
        Ok(())
    }

    fn add_local(&mut self, name: String) {
        let depth = self.ctx().scope_depth;
        self.ctx_mut().locals.push(Local { name, depth });
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
            self.emit(Op::SetLocal);
            self.emit_u16(base as u16);
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
    fn emit_field_extract(&mut self, slot: u16, idx: u8) -> u16 {
        self.emit(Op::GetLocal);
        self.emit_u16(slot);
        self.emit(Op::GetField);
        self.emit_u8(idx);
        self.add_local("_tmp".to_string());
        (self.ctx().locals.len() - 1) as u16
    }

    /// Emit tag check: GetLocal slot, GetTag, Const tag, Eq, JumpIfFalse → fail.
    fn emit_tag_check(
        &mut self,
        slot: u16,
        tag: i64,
        fail_jumps: &mut Vec<(usize, usize)>,
    ) -> Result<(), CompileError> {
        self.emit(Op::GetLocal);
        self.emit_u16(slot);
        self.emit(Op::GetTag);
        self.emit_constant(Constant::Int(tag))?;
        self.emit(Op::Eq);
        let jmp = self.emit_jump(Op::JumpIfFalse);
        fail_jumps.push((jmp, self.ctx().locals.len()));
        Ok(())
    }

    /// Emit scalar comparison: GetLocal slot, push value, eq_op, JumpIfFalse → fail.
    fn emit_scalar_check(
        &mut self,
        slot: u16,
        value: Constant,
        eq_op: Op,
        fail_jumps: &mut Vec<(usize, usize)>,
    ) -> Result<(), CompileError> {
        self.emit(Op::GetLocal);
        self.emit_u16(slot);
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
                self.compile_expr(expr)?;
                let name = self.interner.resolve(*sym).to_string();
                self.bind_name(&name)?;
                Ok(())
            }
            DeclKind::Fun(bindings) => {
                for binding in bindings {
                    self.compile_fun_binding(binding)?;
                    let name = self.interner.resolve(binding.name).to_string();
                    self.bind_name(&name)?;
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
                let name = self.interner.resolve(*public_name).to_string();
                self.bind_name(&name)?;
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
        let expected_hash = normalize_blake3(expected_hash);
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
                return Ok(ResolvedImport::File {
                    canonical: cache_path,
                    display_name: module_name.to_string(),
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
        std::fs::write(&cache_path, source).map_err(|e| {
            CompileError::codegen(format!(
                "cannot write cached module '{}': {e}",
                cache_path.display()
            ))
        })?;
        Ok(ResolvedImport::File {
            canonical: cache_path,
            display_name: module_name.to_string(),
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
            let lockfile_path = find_lockfile_path(entry_file).ok_or_else(|| {
                CompileError::codegen(format!(
                    "named import '{module_name}' requires hiko.lock.toml alongside the entry file"
                ))
            })?;
            let lockfile_text = std::fs::read_to_string(&lockfile_path).map_err(|e| {
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
            validate_lockfile(&lockfile, &lockfile_path)?;
            self.import_lockfile = Some(lockfile);
            self.module_cache_dir = Some(default_module_cache_dir()?);
        }
        let lockfile = self.import_lockfile.as_ref().unwrap();
        let cache_dir = self.module_cache_dir.as_ref().unwrap();
        Ok((lockfile, cache_dir))
    }

    /// Pass 1: type-check a single declaration (handles `use` recursively)
    fn infer_decl_pass(&mut self, decl: &Decl) -> Result<(), CompileError> {
        match &decl.kind {
            DeclKind::Import(name) => self.infer_named_import(*name),
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

        let result = desugared.iter().try_for_each(|d| self.infer_decl_pass(d));

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
                self.add_local("_tup".to_string());
                let tup_slot = (self.ctx().locals.len() - 1) as u16;
                for (i, p) in pats.iter().enumerate() {
                    self.emit(Op::GetLocal);
                    self.emit_u16(tup_slot);
                    self.emit(Op::GetField);
                    self.emit_u8(i as u8);
                    self.compile_binding_pattern(p)?;
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
            let tag = i as u16;
            let has_payload = con.payload.is_some();
            self.constructor_tags.insert(con_name.clone(), tag);
            self.constructor_arities
                .insert(con_name.clone(), u8::from(has_payload));

            if has_payload {
                self.push_new_function(Some(con_name.clone()));
                self.add_local("_arg".to_string());
                self.emit(Op::GetLocal);
                self.emit_u16(0);
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
        }
        Ok(())
    }

    // ── Functions ────────────────────────────────────────────────────

    fn compile_fun_binding(&mut self, binding: &FunBinding) -> Result<(), CompileError> {
        let name = self.interner.resolve(binding.name).to_string();
        if binding.clauses.len() == 1 && binding.clauses[0].pats.iter().all(is_simple_pat) {
            let clause = &binding.clauses[0];
            return self.compile_curried_fn(Some(&name), &clause.pats, 0, &clause.body);
        }
        let arity = binding.clauses[0].pats.len();
        self.compile_clausal_fn(Some(&name), arity, &binding.clauses)
    }

    fn compile_clausal_fn(
        &mut self,
        name: Option<&str>,
        arity: usize,
        clauses: &[FunClause],
    ) -> Result<(), CompileError> {
        self.compile_clausal_fn_inner(name, arity, 0, clauses)
    }

    fn compile_clausal_fn_inner(
        &mut self,
        name: Option<&str>,
        arity: usize,
        arg_idx: usize,
        clauses: &[FunClause],
    ) -> Result<(), CompileError> {
        let fn_name = if arg_idx == 0 {
            name.map(|s| s.to_string())
        } else {
            None
        };
        self.push_new_function(fn_name);
        self.add_local(format!("_arg{arg_idx}"));

        if arg_idx + 1 < arity {
            self.compile_clausal_fn_inner(None, arity, arg_idx + 1, clauses)?;
        } else {
            self.begin_scope();
            if arity == 1 {
                self.emit_get_var("_arg0")?;
                self.add_local("_scrut".to_string());
                let scrut_slot = (self.ctx().locals.len() - 1) as u16;
                let branches: Vec<_> = clauses.iter().map(|c| (&c.pats[0], &c.body)).collect();
                self.compile_case_branches_inner(scrut_slot, &branches, true)?;
            } else {
                for i in 0..arity {
                    self.emit_get_var(&format!("_arg{i}"))?;
                }
                self.emit(Op::MakeTuple);
                self.emit_u8(arity as u8);
                self.add_local("_scrut".to_string());
                let scrut_slot = (self.ctx().locals.len() - 1) as u16;
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
        self.finish_function()
    }

    fn compile_curried_fn(
        &mut self,
        name: Option<&str>,
        pats: &[Pat],
        idx: usize,
        body: &Expr,
    ) -> Result<(), CompileError> {
        let fn_name = if idx == 0 {
            name.map(|s| s.to_string())
        } else {
            None
        };
        self.push_new_function(fn_name);
        self.add_local(simple_pat_name(&pats[idx], &self.interner));
        if idx + 1 < pats.len() {
            self.compile_curried_fn(None, pats, idx + 1, body)?;
        } else {
            self.compile_expr_tail(body)?;
        }
        self.emit(Op::Return);
        self.finish_function()
    }

    fn compile_lambda(&mut self, pat: &Pat, body: &Expr) -> Result<(), CompileError> {
        self.push_new_function(None);
        if is_simple_pat(pat) {
            self.add_local(simple_pat_name(pat, &self.interner));
            self.compile_expr_tail(body)?;
        } else {
            self.add_local("_arg".to_string());
            self.begin_scope();
            self.emit(Op::GetLocal);
            self.emit_u16(0);
            self.add_local("_scrut".to_string());
            let scrut_slot = (self.ctx().locals.len() - 1) as u16;
            self.compile_case_branches_inner(scrut_slot, &[(pat, body)], true)?;
            self.end_scope_keep_result();
        }
        self.emit(Op::Return);
        self.finish_function()
    }

    fn finish_function(&mut self) -> Result<(), CompileError> {
        let func_ctx = self.func_stack.pop().unwrap();
        let n_captures = func_ctx.upvalues.len() as u8;
        let fn_name = func_ctx.name.clone();
        let proto = FunctionProto {
            name: func_ctx.name,
            arity: func_ctx.arity,
            n_captures,
            chunk: func_ctx.chunk,
        };
        let proto_idx = self.functions.len() as u16;
        self.functions.push(proto);
        // Register for direct calls if this is a top-level function with no captures
        if n_captures == 0
            && let Some(name) = &fn_name
        {
            self.global_protos.insert(name.clone(), proto_idx);
        }
        self.emit(Op::MakeClosure);
        self.emit_u16(proto_idx);
        self.emit_u8(n_captures);
        for uv in &func_ctx.upvalues {
            self.emit_u8(u8::from(uv.is_local));
            self.emit_u16(uv.index);
        }
        Ok(())
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
        let mut end_jumps = Vec::new();

        for (pat, body) in branches {
            let locals_before = self.ctx().locals.len();
            let depth_before = self.ctx().scope_depth;
            self.begin_scope();

            let mut fail_jumps: Vec<(usize, usize)> = Vec::new(); // (jump_pos, locals_at_fail)
            self.compile_pattern_test(scrut_slot, pat, &mut fail_jumps)?;
            self.compile_pattern_bind(scrut_slot, pat)?;

            self.compile_expr_inner(body, tail)?;
            self.end_scope_keep_result();
            end_jumps.push(self.emit_jump(Op::Jump));

            // Fail path: each fail point has a different number of
            // temporaries on the stack. Emit a separate cleanup
            // trampoline for each, jumping to a shared landing pad.
            let mut cleanup_end_jumps = Vec::new();
            for (fj, locals_at_fail) in &fail_jumps {
                self.chunk()
                    .patch_jump(*fj)
                    .map_err(CompileError::codegen)?;
                let n_pops = locals_at_fail - locals_before;
                for _ in 0..n_pops {
                    self.emit(Op::Pop);
                }
                if n_pops > 0 {
                    cleanup_end_jumps.push(self.emit_jump(Op::Jump));
                }
            }
            for j in cleanup_end_jumps {
                self.patch_jump(j)?;
            }

            // Restore compiler state for next branch
            while self.ctx().locals.len() > locals_before {
                self.ctx_mut().locals.pop();
            }
            self.ctx_mut().scope_depth = depth_before;
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
                self.emit(Op::GetLocal);
                self.emit_u16(slot);
                if *b {
                    self.emit(Op::True);
                } else {
                    self.emit(Op::False);
                }
                self.emit(Op::Eq);
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
                self.emit_tag_check(slot, tag as i64, fail_jumps)?;
                if let Some(sub_pat) = payload {
                    // Need to test sub-pattern against field 0
                    // Push temp to get a slot, then pop after testing
                    self.emit_field_extract_test(slot, 0, sub_pat, fail_jumps)?;
                }
            }

            PatKind::Tuple(pats) => {
                for (i, p) in pats.iter().enumerate() {
                    self.emit_field_extract_test(slot, i as u8, p, fail_jumps)?;
                }
            }

            PatKind::Cons(hd, tl) => {
                self.emit_tag_check(slot, 1, fail_jumps)?; // Cons = tag 1
                self.emit_field_extract_test(slot, 0, hd, fail_jumps)?;
                self.emit_field_extract_test(slot, 1, tl, fail_jumps)?;
            }

            PatKind::List(pats) => {
                assert!(
                    pats.is_empty(),
                    "non-empty list pattern should be desugared to Cons"
                );
                self.emit_tag_check(slot, 0, fail_jumps)?; // Nil = tag 0
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
        field_idx: u8,
        sub_pat: &Pat,
        fail_jumps: &mut Vec<(usize, usize)>,
    ) -> Result<(), CompileError> {
        if is_trivial_pat(sub_pat) {
            return Ok(()); // wildcard/var: nothing to test
        }
        // Push field value as temp local for sub-pattern testing
        let field_slot = self.emit_field_extract(slot, field_idx);
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
                self.emit(Op::GetLocal);
                self.emit_u16(slot);
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
                let field_slot = self.emit_field_extract(slot, 0);
                self.compile_pattern_bind(field_slot, sub_pat)?;
            }

            PatKind::Tuple(pats) => {
                for (i, p) in pats.iter().enumerate() {
                    let field_slot = self.emit_field_extract(slot, i as u8);
                    self.compile_pattern_bind(field_slot, p)?;
                }
            }

            PatKind::Cons(hd, tl) => {
                let hd_slot = self.emit_field_extract(slot, 0);
                self.compile_pattern_bind(hd_slot, hd)?;
                let tl_slot = self.emit_field_extract(slot, 1);
                self.compile_pattern_bind(tl_slot, tl)?;
            }

            PatKind::List(_) => {} // empty list, nothing to bind

            PatKind::As(sym, sub_pat) => {
                let name = self.interner.resolve(*sym).to_string();
                self.emit(Op::GetLocal);
                self.emit_u16(slot);
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

    fn compile_expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
        self.compile_expr_inner(expr, false)
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
            ExprKind::Var(sym) => {
                let name = self.interner.resolve(*sym).to_string();
                self.emit_get_var(&name)?;
            }
            ExprKind::Constructor(sym) => {
                let name = self.interner.resolve(*sym).to_string();
                self.emit_get_var(&name)?;
            }

            ExprKind::Tuple(elems) => {
                for e in elems {
                    self.compile_expr(e)?;
                }
                self.emit(Op::MakeTuple);
                self.emit_u8(elems.len() as u8);
            }
            ExprKind::List(elems) => {
                assert!(
                    elems.is_empty(),
                    "non-empty list should be desugared to Cons"
                );
                self.emit(Op::MakeData);
                self.emit_u16(0); // Nil
                self.emit_u8(0);
            }
            ExprKind::Cons(head, tail) => {
                self.compile_expr(head)?;
                self.compile_expr(tail)?;
                self.emit(Op::MakeData);
                self.emit_u16(1);
                self.emit_u8(2);
            }

            ExprKind::BinOp(BinOp::Andalso | BinOp::Orelse, _, _) => {
                unreachable!("desugared to if-then-else")
            }
            ExprKind::BinOp(op, lhs, rhs) => {
                if let Some(resolved_op) = self.resolve_generic_binop(*op, expr.span) {
                    self.compile_expr(lhs)?;
                    self.compile_expr(rhs)?;
                    self.emit(resolved_op);
                } else {
                    self.compile_expr(lhs)?;
                    self.compile_expr(rhs)?;
                    self.emit(binop_to_op(*op));
                }
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

            ExprKind::App(func, arg) => {
                // Check for direct call: App(Var(name), arg) where name
                // is a known global function with no captures
                let direct_proto = if let ExprKind::Var(sym) = &func.kind {
                    let name = self.interner.resolve(*sym).to_string();
                    if self.resolve_local(&name).is_none() && self.resolve_upvalue(&name).is_none()
                    {
                        // Global function
                        self.global_protos.get(&name).copied()
                    } else if self.ctx().name.as_deref() == Some(&name) {
                        // Self-recursive call: proto_idx will be self.functions.len()
                        Some(self.functions.len() as u16)
                    } else {
                        // Also check if name matches a known global even as upvalue
                        // (recursive calls capture the function as an upvalue)
                        self.global_protos.get(&name).copied()
                    }
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
                    self.compile_expr(func)?;
                    self.compile_expr(arg)?;
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
                self.add_local("_scrut".to_string());
                let scrut_slot = (self.ctx().locals.len() - 1) as u16;
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
            } => {
                // Emit InstallHandler: u16 n_clauses, then per clause:
                //   u16 effect_tag, i16 offset (placeholder)
                self.emit(Op::InstallHandler);
                let n_clauses = handlers.len() as u16;
                self.emit_u16(n_clauses);
                // Record positions of clause offset placeholders
                let mut clause_offset_positions = Vec::new();
                for handler in handlers {
                    let eff_name = self.interner.resolve(handler.effect_name).to_string();
                    let tag = *self.effect_tags.get(&eff_name).ok_or_else(|| {
                        CompileError::codegen(format!("unknown effect: {eff_name}"))
                    })?;
                    self.emit_u16(tag);
                    let pos = self.chunk().code.len();
                    self.emit_u16(0); // placeholder offset
                    clause_offset_positions.push(pos);
                }

                // Jump over clause code
                let skip_jump = self.emit_jump(Op::Jump);

                // Compile each effect clause
                let mut clause_end_jumps = Vec::new();
                for (i, handler) in handlers.iter().enumerate() {
                    self.patch_jump(clause_offset_positions[i])?;
                    self.begin_scope();
                    let payload_name = self.interner.resolve(handler.payload_var).to_string();
                    let cont_name = self.interner.resolve(handler.cont_var).to_string();
                    self.add_local(payload_name);
                    self.add_local(cont_name);
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
                let return_var_name = self.interner.resolve(*return_var).to_string();
                self.add_local(return_var_name);
                self.compile_expr_inner(return_body, tail)?;
                self.end_scope_keep_result();

                // Patch all clause end jumps to here
                for j in clause_end_jumps {
                    self.patch_jump(j)?;
                }
            }

            ExprKind::Resume(cont, arg) => {
                self.compile_expr_inner(cont, false)?;
                self.compile_expr_inner(arg, false)?;
                self.emit(Op::Resume);
            }
        }
        Ok(())
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

    fn cached_module_path(url: &str, hash: &str) -> PathBuf {
        default_module_cache_dir()
            .expect("default cache dir")
            .join(format!("{}-{}.hml", blake3_hex(url.as_bytes()), hash))
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
        Compiler::compile_file(parse_program(&source), path)
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
        let entry_path = project_dir.join("main.hml");
        let module_source = "structure List = struct\n  val forty_two = 42\nend\n".to_string();
        let module_hash = blake3_hex(module_source.as_bytes());
        let (list_url, server) = spawn_single_response_server("/modules/List.hml", module_source);
        let base_url = list_url.trim_end_matches("/modules/List.hml").to_string();
        let cache_path = cached_module_path(&format!("{base_url}/modules/List.hml"), &module_hash);

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
            "expected fetched module in default cache"
        );

        let _ = std::fs::remove_file(&cache_path);
        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[test]
    fn named_import_rejects_bad_blake3() {
        let project_dir = unique_temp_dir("named-import-bad-hash");
        let entry_path = project_dir.join("main.hml");
        let module_source = "val remote_answer = 42\n".to_string();
        let (answer_url, server) =
            spawn_single_response_server("/modules/Answer.hml", module_source);
        let base_url = answer_url
            .trim_end_matches("/modules/Answer.hml")
            .to_string();
        let cache_path = cached_module_path(&format!("{base_url}/modules/Answer.hml"), "deadbeef");

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

    #[test]
    fn named_import_revalidates_cached_file_on_later_load() {
        let project_dir = unique_temp_dir("named-import-revalidate");
        let entry_path = project_dir.join("main.hml");
        let module_source = "structure Cache = struct\n  val value = 7\nend\n".to_string();
        let module_hash = blake3_hex(module_source.as_bytes());
        let (cache_url, server) =
            spawn_response_server("/modules/Cache.hml", module_source.clone(), 2);
        let base_url = cache_url.trim_end_matches("/modules/Cache.hml").to_string();
        let cache_path = cached_module_path(&format!("{base_url}/modules/Cache.hml"), &module_hash);

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
}
