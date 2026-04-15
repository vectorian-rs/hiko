use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hiko_syntax::ast::*;
use hiko_syntax::intern::StringInterner;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_types::infer::{InferCtx, TypeError};

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

pub struct Compiler {
    functions: Vec<FunctionProto>,
    func_stack: Vec<FuncCtx>,
    constructor_tags: HashMap<String, u16>,
    constructor_arities: HashMap<String, u8>,
    /// Effect name -> tag for effect handler dispatch.
    effect_tags: HashMap<String, u16>,
    next_effect_tag: u16,
    /// Base directory for resolving relative imports.
    base_dir: PathBuf,
    /// Files currently being loaded (for cycle detection).
    loading_files: HashSet<PathBuf>,
    /// Files whose type inference is complete.
    inferred_files: HashSet<PathBuf>,
    /// Files whose codegen is complete.
    compiled_files: HashSet<PathBuf>,
    /// Cached desugared programs for imported files (used between passes).
    imported_programs: HashMap<PathBuf, Vec<Decl>>,
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
        loading.insert(canonical);
        Self::compile_with_ctx(program, base_dir, loading)
    }

    /// Compile a program without a file context (e.g., from a string).
    pub fn compile(
        program: Program,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        Self::compile_with_ctx(program, PathBuf::from("."), HashSet::new())
    }

    fn compile_with_ctx(
        program: Program,
        base_dir: PathBuf,
        loading_files: HashSet<PathBuf>,
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
            loading_files,
            inferred_files: HashSet::new(),
            compiled_files: HashSet::new(),
            imported_programs: HashMap::new(),
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
            DeclKind::Effect(sym, _) => {
                let name = self.interner.resolve(*sym).to_string();
                let tag = self.next_effect_tag;
                self.next_effect_tag += 1;
                self.effect_tags.insert(name, tag);
                Ok(())
            }
            DeclKind::Use(path) => self.compile_use(path),
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

    // ── Two-pass import handling ────────────────────────────────────

    fn resolve_import(&self, path: &str) -> Result<PathBuf, CompileError> {
        let resolved = self.base_dir.join(path);
        std::fs::canonicalize(&resolved).map_err(|e| {
            CompileError::codegen(format!(
                "cannot resolve import '{}': {e}",
                resolved.display()
            ))
        })
    }

    /// Pass 1: type-check a single declaration (handles `use` recursively)
    fn infer_decl_pass(&mut self, decl: &Decl) -> Result<(), CompileError> {
        match &decl.kind {
            DeclKind::Use(path) => self.infer_use(path),
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

    /// Pass 1 for imports: load, parse, desugar, infer, and cache
    fn infer_use(&mut self, path: &str) -> Result<(), CompileError> {
        let canonical = self.resolve_import(path)?;

        if self.loading_files.contains(&canonical) {
            return Err(CompileError::codegen(format!(
                "circular import detected: '{}'",
                canonical.display()
            )));
        }

        if self.inferred_files.contains(&canonical) {
            return Ok(());
        }

        let source = std::fs::read_to_string(&canonical).map_err(|e| {
            CompileError::codegen(format!("cannot read '{}': {e}", canonical.display()))
        })?;
        let tokens = Lexer::new(&source, 0)
            .tokenize()
            .map_err(|e| CompileError::codegen(format!("lex error in '{path}': {}", e.message)))?;
        // Use the same interner for import parsing so symbols are shared
        let import_interner = std::mem::take(&mut self.interner);
        let program = Parser::with_interner(tokens, import_interner)
            .parse_program()
            .map_err(|e| {
                CompileError::codegen(format!("parse error in '{path}': {}", e.message))
            })?;

        let program = hiko_syntax::desugar::desugar_program(program);
        let program = hiko_syntax::constfold::fold_program(program);
        self.interner = program.interner;
        self.infer_ctx.interner = self.interner.clone();
        let desugared = program.decls;

        self.loading_files.insert(canonical.clone());
        let old_base = self.base_dir.clone();
        self.base_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();

        let result = desugared.iter().try_for_each(|d| self.infer_decl_pass(d));

        self.base_dir = old_base;
        self.loading_files.remove(&canonical);

        result?;
        self.inferred_files.insert(canonical.clone());
        self.imported_programs.insert(canonical, desugared);
        Ok(())
    }

    /// Pass 2 for imports: compile from cached desugared AST
    fn compile_use(&mut self, path: &str) -> Result<(), CompileError> {
        let canonical = self.resolve_import(path)?;

        if self.compiled_files.contains(&canonical) {
            return Ok(());
        }

        let desugared = self
            .imported_programs
            .remove(&canonical)
            .unwrap_or_default();

        let old_base = self.base_dir.clone();
        self.base_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
        for decl in &desugared {
            self.compile_decl(decl)?;
        }
        self.base_dir = old_base;
        self.compiled_files.insert(canonical);
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
                self.compile_expr(lhs)?;
                self.compile_expr(rhs)?;
                self.emit(binop_to_op(*op));
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
        BinOp::AddInt => Op::AddInt,
        BinOp::SubInt => Op::SubInt,
        BinOp::MulInt => Op::MulInt,
        BinOp::DivInt => Op::DivInt,
        BinOp::ModInt => Op::ModInt,
        BinOp::AddFloat => Op::AddFloat,
        BinOp::SubFloat => Op::SubFloat,
        BinOp::MulFloat => Op::MulFloat,
        BinOp::DivFloat => Op::DivFloat,
        BinOp::ConcatStr => Op::ConcatString,
        BinOp::LtInt => Op::LtInt,
        BinOp::GtInt => Op::GtInt,
        BinOp::LeInt => Op::LeInt,
        BinOp::GeInt => Op::GeInt,
        BinOp::LtFloat => Op::LtFloat,
        BinOp::GtFloat => Op::GtFloat,
        BinOp::LeFloat => Op::LeFloat,
        BinOp::GeFloat => Op::GeFloat,
        BinOp::Eq => Op::Eq,
        BinOp::Ne => Op::Ne,
        BinOp::Andalso | BinOp::Orelse => unreachable!("short-circuit ops handled separately"),
    }
}
