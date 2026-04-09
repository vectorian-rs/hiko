use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use hiko_syntax::ast::*;
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
    /// Base directory for resolving relative imports.
    base_dir: PathBuf,
    /// Canonical paths of files already loaded (cycle detection + single eval).
    loaded_files: HashSet<PathBuf>,
    /// Shared type inference context (imports add to the same environment).
    infer_ctx: InferCtx,
}

impl Compiler {
    /// Compile a program from a file. Type inference is run first.
    /// `file_path` is used to resolve relative `use` imports.
    pub fn compile_file(
        program: &Program,
        file_path: &Path,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        let base_dir = file_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let canonical =
            std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
        let mut loaded = HashSet::new();
        loaded.insert(canonical);
        Self::compile_with_ctx(program, base_dir, loaded)
    }

    /// Compile a program without a file context (e.g., from a string).
    pub fn compile(
        program: &Program,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
        Self::compile_with_ctx(program, PathBuf::from("."), HashSet::new())
    }

    fn compile_with_ctx(
        program: &Program,
        base_dir: PathBuf,
        loaded_files: HashSet<PathBuf>,
    ) -> Result<(CompiledProgram, Vec<hiko_types::infer::Warning>), CompileError> {
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
            base_dir,
            loaded_files,
            infer_ctx: InferCtx::new(),
        };
        for decl in &program.decls {
            c.infer_ctx.infer_decl(decl)?;
            c.compile_decl(decl)?;
        }
        c.emit(Op::Halt);
        let main = c.func_stack.pop().unwrap();
        let warnings = c.infer_ctx.warnings;
        Ok((
            CompiledProgram {
                main: main.chunk,
                functions: c.functions,
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
    fn emit_constant(&mut self, c: Constant) {
        let idx = self.chunk().add_constant(c);
        self.emit(Op::Const);
        self.emit_u16(idx);
    }
    fn emit_jump(&mut self, op: Op) -> usize {
        self.chunk().emit_jump(op)
    }
    fn patch_jump(&mut self, pos: usize) {
        self.chunk().patch_jump(pos);
    }
    fn add_string_constant(&mut self, s: &str) -> u16 {
        self.chunk().add_constant(Constant::String(s.to_string()))
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

    fn emit_get_var(&mut self, name: &str) {
        if let Some(slot) = self.resolve_local(name) {
            self.emit(Op::GetLocal);
            self.emit_u16(slot);
        } else if let Some(idx) = self.resolve_upvalue(name) {
            self.emit(Op::GetUpvalue);
            self.emit_u16(idx);
        } else {
            let c = self.add_string_constant(name);
            self.emit(Op::GetGlobal);
            self.emit_u16(c);
        }
    }

    fn emit_set_global(&mut self, name: &str) {
        let c = self.add_string_constant(name);
        self.emit(Op::SetGlobal);
        self.emit_u16(c);
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

    fn bind_name(&mut self, name: &str) {
        if self.ctx().scope_depth == 0 {
            self.emit_set_global(name);
        } else {
            self.add_local(name.to_string());
        }
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

    /// Emit tag check: GetLocal slot, GetTag, Const tag, EqInt, JumpIfFalse → fail.
    fn emit_tag_check(&mut self, slot: u16, tag: i64, fail_jumps: &mut Vec<usize>) {
        self.emit(Op::GetLocal);
        self.emit_u16(slot);
        self.emit(Op::GetTag);
        self.emit_constant(Constant::Int(tag));
        self.emit(Op::EqInt);
        fail_jumps.push(self.emit_jump(Op::JumpIfFalse));
    }

    /// Emit scalar comparison: GetLocal slot, push value, eq_op, JumpIfFalse → fail.
    fn emit_scalar_check(
        &mut self,
        slot: u16,
        value: Constant,
        eq_op: Op,
        fail_jumps: &mut Vec<usize>,
    ) {
        self.emit(Op::GetLocal);
        self.emit_u16(slot);
        self.emit_constant(value);
        self.emit(eq_op);
        fail_jumps.push(self.emit_jump(Op::JumpIfFalse));
    }

    // ── Declarations ─────────────────────────────────────────────────

    fn compile_decl(&mut self, decl: &Decl) -> Result<(), CompileError> {
        match &decl.kind {
            DeclKind::Val(pat, expr) => {
                self.compile_expr(expr)?;
                self.compile_binding_pattern(pat)
            }
            DeclKind::ValRec(name, expr) => {
                self.compile_expr(expr)?;
                self.bind_name(name);
                Ok(())
            }
            DeclKind::Fun(bindings) => {
                for binding in bindings {
                    self.compile_fun_binding(binding)?;
                    self.bind_name(&binding.name);
                }
                Ok(())
            }
            DeclKind::Datatype(dt) => self.compile_datatype(dt),
            DeclKind::TypeAlias(_) => Ok(()),
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

    fn compile_use(&mut self, path: &str) -> Result<(), CompileError> {
        let resolved = self.base_dir.join(path);
        let canonical = std::fs::canonicalize(&resolved).map_err(|e| {
            CompileError::codegen(format!(
                "cannot resolve import '{}': {e}",
                resolved.display()
            ))
        })?;

        // Single evaluation: skip if already loaded
        if self.loaded_files.contains(&canonical) {
            return Ok(());
        }
        self.loaded_files.insert(canonical.clone());

        // Read, lex, parse
        let source = std::fs::read_to_string(&canonical).map_err(|e| {
            CompileError::codegen(format!("cannot read '{}': {e}", canonical.display()))
        })?;
        let tokens = Lexer::new(&source, 0)
            .tokenize()
            .map_err(|e| CompileError::codegen(format!("lex error in '{path}': {}", e.message)))?;
        let program = Parser::new(tokens).parse_program().map_err(|e| {
            CompileError::codegen(format!("parse error in '{path}': {}", e.message))
        })?;

        // Type-check and compile imported declarations
        let old_base = self.base_dir.clone();
        self.base_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
        for decl in &program.decls {
            self.infer_ctx.infer_decl(decl)?;
            self.compile_decl(decl)?;
        }
        self.base_dir = old_base;
        Ok(())
    }

    fn compile_binding_pattern(&mut self, pat: &Pat) -> Result<(), CompileError> {
        match &pat.kind {
            PatKind::Var(name) => {
                self.bind_name(name);
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
            let tag = i as u16;
            let has_payload = con.payload.is_some();
            self.constructor_tags.insert(con.name.clone(), tag);
            self.constructor_arities
                .insert(con.name.clone(), u8::from(has_payload));

            if has_payload {
                self.push_new_function(Some(con.name.clone()));
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
            self.bind_name(&con.name);
        }
        Ok(())
    }

    // ── Functions ────────────────────────────────────────────────────

    fn compile_fun_binding(&mut self, binding: &FunBinding) -> Result<(), CompileError> {
        if binding.clauses.len() == 1 && binding.clauses[0].pats.iter().all(is_simple_pat) {
            let clause = &binding.clauses[0];
            return self.compile_curried_fn(Some(&binding.name), &clause.pats, 0, &clause.body);
        }
        let arity = binding.clauses[0].pats.len();
        self.compile_clausal_fn(Some(&binding.name), arity, &binding.clauses)
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
                self.emit_get_var("_arg0");
                self.add_local("_scrut".to_string());
                let scrut_slot = (self.ctx().locals.len() - 1) as u16;
                let branches: Vec<_> = clauses.iter().map(|c| (&c.pats[0], &c.body)).collect();
                self.compile_case_branches(scrut_slot, &branches)?;
            } else {
                for i in 0..arity {
                    self.emit_get_var(&format!("_arg{i}"));
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
                self.compile_case_branches(scrut_slot, &branches)?;
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
        self.add_local(simple_pat_name(&pats[idx]));
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
            self.add_local(simple_pat_name(pat));
            self.compile_expr_tail(body)?;
        } else {
            self.add_local("_arg".to_string());
            self.begin_scope();
            self.emit(Op::GetLocal);
            self.emit_u16(0);
            self.add_local("_scrut".to_string());
            let scrut_slot = (self.ctx().locals.len() - 1) as u16;
            self.compile_case_branches(scrut_slot, &[(pat, body)])?;
            self.end_scope_keep_result();
        }
        self.emit(Op::Return);
        self.finish_function()
    }

    fn finish_function(&mut self) -> Result<(), CompileError> {
        let func_ctx = self.func_stack.pop().unwrap();
        let n_captures = func_ctx.upvalues.len() as u8;
        let proto = FunctionProto {
            name: func_ctx.name,
            arity: func_ctx.arity,
            n_captures,
            chunk: func_ctx.chunk,
        };
        let proto_idx = self.functions.len() as u16;
        self.functions.push(proto);
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

    fn compile_case_branches(
        &mut self,
        scrut_slot: u16,
        branches: &[(&Pat, &Expr)],
    ) -> Result<(), CompileError> {
        self.compile_case_branches_inner(scrut_slot, branches, false)
    }

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

            let mut fail_jumps = Vec::new();
            self.compile_pattern_test(scrut_slot, pat, &mut fail_jumps)?;
            self.compile_pattern_bind(scrut_slot, pat)?;

            self.compile_expr_inner(body, tail)?;
            self.end_scope_keep_result();
            end_jumps.push(self.emit_jump(Op::Jump));

            // Fail: patch all fail jumps to here. No cleanup needed since
            // no locals were pushed during the test phase.
            for fj in fail_jumps {
                self.chunk().patch_jump(fj);
            }

            // Restore compiler state for next branch
            while self.ctx().locals.len() > locals_before {
                self.ctx_mut().locals.pop();
            }
            self.ctx_mut().scope_depth = depth_before;
        }

        // Non-exhaustive match: emit runtime panic
        let msg_idx = self.add_string_constant("non-exhaustive match");
        self.emit(Op::Panic);
        self.emit_u16(msg_idx);

        for j in end_jumps {
            self.patch_jump(j);
        }
        Ok(())
    }

    /// Pass 1: emit tests only. No locals are pushed.
    /// Each failed test emits JumpIfFalse into fail_jumps.
    fn compile_pattern_test(
        &mut self,
        slot: u16,
        pat: &Pat,
        fail_jumps: &mut Vec<usize>,
    ) -> Result<(), CompileError> {
        match &pat.kind {
            PatKind::Wildcard | PatKind::Var(_) | PatKind::Unit => {} // always match

            PatKind::IntLit(n) => {
                self.emit_scalar_check(slot, Constant::Int(*n), Op::EqInt, fail_jumps);
            }
            PatKind::FloatLit(f) => {
                self.emit_scalar_check(slot, Constant::Float(*f), Op::EqFloat, fail_jumps);
            }
            PatKind::BoolLit(b) => {
                self.emit(Op::GetLocal);
                self.emit_u16(slot);
                if *b {
                    self.emit(Op::True);
                } else {
                    self.emit(Op::False);
                }
                self.emit(Op::EqBool);
                fail_jumps.push(self.emit_jump(Op::JumpIfFalse));
            }
            PatKind::StringLit(s) => {
                self.emit_scalar_check(slot, Constant::String(s.clone()), Op::EqString, fail_jumps);
            }
            PatKind::CharLit(c) => {
                self.emit_scalar_check(slot, Constant::Char(*c), Op::EqChar, fail_jumps);
            }

            PatKind::Constructor(name, payload) => {
                let tag = *self
                    .constructor_tags
                    .get(name)
                    .ok_or_else(|| CompileError::codegen(format!("unknown constructor: {name}")))?;
                self.emit_tag_check(slot, tag as i64, fail_jumps);
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
                self.emit_tag_check(slot, 1, fail_jumps); // Cons = tag 1
                self.emit_field_extract_test(slot, 0, hd, fail_jumps)?;
                self.emit_field_extract_test(slot, 1, tl, fail_jumps)?;
            }

            PatKind::List(pats) if pats.is_empty() => {
                self.emit_tag_check(slot, 0, fail_jumps); // Nil = tag 0
            }
            PatKind::List(pats) => {
                // [p1, p2, ...] = p1 :: p2 :: ... :: []
                let mut current_slot = slot;
                for (i, p) in pats.iter().enumerate() {
                    self.emit_tag_check(current_slot, 1, fail_jumps); // Cons
                    self.emit_field_extract_test(current_slot, 0, p, fail_jumps)?;
                    // Get tail for next iteration
                    self.emit(Op::GetLocal);
                    self.emit_u16(current_slot);
                    self.emit(Op::GetField);
                    self.emit_u8(1);
                    self.add_local("_ltl".to_string());
                    current_slot = (self.ctx().locals.len() - 1) as u16;
                    if i == pats.len() - 1 {
                        self.emit_tag_check(current_slot, 0, fail_jumps); // must be Nil
                    }
                }
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
        fail_jumps: &mut Vec<usize>,
    ) -> Result<(), CompileError> {
        if is_trivial_pat(sub_pat) {
            return Ok(()); // wildcard/var — nothing to test
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
            PatKind::Var(name) => {
                self.emit(Op::GetLocal);
                self.emit_u16(slot);
                self.add_local(name.clone());
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

            PatKind::List(pats) if pats.is_empty() => {}
            PatKind::List(pats) => {
                let mut current_slot = slot;
                for p in pats {
                    let hd_slot = self.emit_field_extract(current_slot, 0);
                    self.compile_pattern_bind(hd_slot, p)?;
                    current_slot = self.emit_field_extract(current_slot, 1);
                }
            }

            PatKind::As(name, sub_pat) => {
                self.emit(Op::GetLocal);
                self.emit_u16(slot);
                self.add_local(name.clone());
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
            ExprKind::IntLit(n) => self.emit_constant(Constant::Int(*n)),
            ExprKind::FloatLit(f) => self.emit_constant(Constant::Float(*f)),
            ExprKind::StringLit(s) => self.emit_constant(Constant::String(s.clone())),
            ExprKind::CharLit(c) => self.emit_constant(Constant::Char(*c)),
            ExprKind::BoolLit(true) => self.emit(Op::True),
            ExprKind::BoolLit(false) => self.emit(Op::False),
            ExprKind::Unit => self.emit(Op::Unit),
            ExprKind::Var(name) => self.emit_get_var(name),
            ExprKind::Constructor(name) => self.emit_get_var(name),

            ExprKind::Tuple(elems) => {
                for e in elems {
                    self.compile_expr(e)?;
                }
                self.emit(Op::MakeTuple);
                self.emit_u8(elems.len() as u8);
            }
            ExprKind::List(elems) => {
                if elems.is_empty() {
                    self.emit(Op::MakeData);
                    self.emit_u16(0);
                    self.emit_u8(0);
                } else {
                    // Build as nested Cons: [1,2,3] = Cons(1, Cons(2, Cons(3, Nil)))
                    // Use iterative approach to avoid stack overflow on long lists
                    for e in elems {
                        self.compile_expr(e)?;
                    }
                    // Now stack has: [e1, e2, ..., en]
                    // Build from right: push Nil, then Cons each element
                    self.emit(Op::MakeData);
                    self.emit_u16(0); // Nil
                    self.emit_u8(0);
                    // Stack: [e1, e2, ..., en, Nil]
                    // We need to Cons en with Nil, then Cons e(n-1) with that, etc.
                    // But elements are in wrong order on stack for MakeData.
                    // MakeData 1 2 pops: [head, tail] where head is deeper.
                    // Stack after each MakeData: consumes top 2, pushes 1.
                    for _ in 0..elems.len() {
                        self.emit(Op::MakeData);
                        self.emit_u16(1); // Cons
                        self.emit_u8(2);
                    }
                }
            }
            ExprKind::Cons(head, tail) => {
                self.compile_expr(head)?;
                self.compile_expr(tail)?;
                self.emit(Op::MakeData);
                self.emit_u16(1);
                self.emit_u8(2);
            }

            ExprKind::BinOp(BinOp::Andalso, lhs, rhs) => {
                self.compile_expr(lhs)?;
                let short = self.emit_jump(Op::JumpIfFalse);
                self.compile_expr(rhs)?;
                let end = self.emit_jump(Op::Jump);
                self.patch_jump(short);
                self.emit(Op::False);
                self.patch_jump(end);
            }
            ExprKind::BinOp(BinOp::Orelse, lhs, rhs) => {
                self.compile_expr(lhs)?;
                let short = self.emit_jump(Op::JumpIfFalse);
                self.emit(Op::True);
                let end = self.emit_jump(Op::Jump);
                self.patch_jump(short);
                self.compile_expr(rhs)?;
                self.patch_jump(end);
            }
            ExprKind::BinOp(op, lhs, rhs) => {
                self.compile_expr(lhs)?;
                self.compile_expr(rhs)?;
                self.emit(binop_to_op(*op));
            }

            ExprKind::UnaryNeg(e) => {
                self.compile_expr(e)?;
                self.emit(Op::NegInt);
            }
            ExprKind::Not(e) => {
                self.compile_expr(e)?;
                self.emit(Op::Not);
            }

            ExprKind::App(func, arg) => {
                self.compile_expr(func)?;
                self.compile_expr(arg)?;
                if tail {
                    self.emit(Op::TailCall);
                } else {
                    self.emit(Op::Call);
                }
                self.emit_u8(1);
            }

            ExprKind::Fn(pat, body) => self.compile_lambda(pat, body)?,

            ExprKind::If(cond, then_br, else_br) => {
                self.compile_expr(cond)?;
                let else_jump = self.emit_jump(Op::JumpIfFalse);
                self.compile_expr_inner(then_br, tail)?;
                let end_jump = self.emit_jump(Op::Jump);
                self.patch_jump(else_jump);
                self.compile_expr_inner(else_br, tail)?;
                self.patch_jump(end_jump);
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
        }
        Ok(())
    }
}

fn simple_pat_name(pat: &Pat) -> String {
    match &pat.kind {
        PatKind::Var(name) => name.clone(),
        PatKind::Wildcard => "_".to_string(),
        PatKind::Paren(p) | PatKind::Ann(p, _) => simple_pat_name(p),
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
        BinOp::Eq => Op::EqInt,
        BinOp::Ne => Op::NeInt,
        BinOp::Andalso | BinOp::Orelse => unreachable!("short-circuit ops handled separately"),
    }
}
