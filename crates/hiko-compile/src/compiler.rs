use hiko_syntax::ast::*;

use crate::chunk::{Chunk, CompiledProgram, Constant, FunctionProto};
use crate::op::Op;

#[derive(Debug)]
pub struct CompileError {
    pub message: String,
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
}

impl Compiler {
    pub fn compile(program: &Program) -> Result<CompiledProgram, CompileError> {
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
        };
        for decl in &program.decls {
            c.compile_decl(decl)?;
        }
        c.emit(Op::Halt);
        let main = c.func_stack.pop().unwrap();
        Ok(CompiledProgram {
            main: main.chunk,
            functions: c.functions,
        })
    }

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

    fn locals_in_current_scope(&self) -> usize {
        let depth = self.ctx().scope_depth;
        self.ctx()
            .locals
            .iter()
            .rev()
            .take_while(|l| l.depth == depth)
            .count()
    }

    fn end_scope_keep_result(&mut self) {
        let n = self.locals_in_current_scope();
        let depth = self.ctx().scope_depth;
        // Remove locals from tracking
        while self.ctx().locals.last().is_some_and(|l| l.depth == depth) {
            self.ctx_mut().locals.pop();
        }
        self.ctx_mut().scope_depth -= 1;

        if n > 0 {
            // Stack: [... locals... result]
            // Store result into the first local's slot, pop the rest
            let base = self.ctx().locals.len();
            self.emit(Op::SetLocal);
            self.emit_u16(base as u16);
            for _ in 0..n - 1 {
                self.emit(Op::Pop);
            }
            // Now the result is at `base`, which is TOS
        }
    }

    fn end_scope_no_result(&mut self) {
        let depth = self.ctx().scope_depth;
        while self.ctx().locals.last().is_some_and(|l| l.depth == depth) {
            self.ctx_mut().locals.pop();
            self.emit(Op::Pop);
        }
        self.ctx_mut().scope_depth -= 1;
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
            DeclKind::Datatype(_) | DeclKind::TypeAlias(_) | DeclKind::Use(_) => Ok(()),
            DeclKind::Local(locals, body) => {
                self.begin_scope();
                for d in locals {
                    self.compile_decl(d)?;
                }
                for d in body {
                    self.compile_decl(d)?;
                }
                self.end_scope_no_result();
                Ok(())
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
            PatKind::Paren(p) => self.compile_binding_pattern(p),
            PatKind::Ann(p, _) => self.compile_binding_pattern(p),
            _ => Err(CompileError {
                message: format!("unsupported pattern in binding: {:?}", pat.kind),
            }),
        }
    }

    fn compile_fun_binding(&mut self, binding: &FunBinding) -> Result<(), CompileError> {
        if binding.clauses.len() != 1 {
            return Err(CompileError {
                message: "clausal functions require pattern matching (Phase 4)".to_string(),
            });
        }
        let clause = &binding.clauses[0];

        // Compile as nested single-arg functions (currying)
        // fun f x y z = body  →  fn x => fn y => fn z => body
        self.compile_curried_fn(Some(&binding.name), &clause.pats, 0, &clause.body)
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
        self.func_stack.push(FuncCtx {
            chunk: Chunk::default(),
            locals: Vec::new(),
            upvalues: Vec::new(),
            scope_depth: 0,
            arity: 1,
            name: fn_name,
        });

        self.add_local(simple_pat_name(&pats[idx]));

        if idx + 1 < pats.len() {
            // More parameters: emit a nested closure
            self.compile_curried_fn(None, pats, idx + 1, body)?;
        } else {
            // Last parameter: compile the body
            self.compile_expr(body)?;
        }
        self.emit(Op::Return);

        self.finish_function()
    }

    fn compile_lambda(&mut self, pat: &Pat, body: &Expr) -> Result<(), CompileError> {
        self.func_stack.push(FuncCtx {
            chunk: Chunk::default(),
            locals: Vec::new(),
            upvalues: Vec::new(),
            scope_depth: 0,
            arity: 1,
            name: None,
        });

        self.add_local(simple_pat_name(pat));
        self.compile_expr(body)?;
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

    // ── Expressions ──────────────────────────────────────────────────

    fn compile_expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
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
                // Build Nil, then Cons each element right-to-left
                self.emit(Op::MakeData);
                self.emit_u16(0); // Nil
                self.emit_u8(0);
                for e in elems.iter().rev() {
                    self.compile_expr(e)?;
                    self.emit(Op::MakeData);
                    self.emit_u16(1); // Cons
                    self.emit_u8(2);
                }
            }
            ExprKind::Cons(head, tail) => {
                self.compile_expr(tail)?;
                self.compile_expr(head)?;
                self.emit(Op::MakeData);
                self.emit_u16(1); // Cons
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
                self.emit(Op::Call);
                self.emit_u8(1);
            }

            ExprKind::Fn(pat, body) => {
                self.compile_lambda(pat, body)?;
            }

            ExprKind::If(cond, then_br, else_br) => {
                self.compile_expr(cond)?;
                let else_jump = self.emit_jump(Op::JumpIfFalse);
                self.compile_expr(then_br)?;
                let end_jump = self.emit_jump(Op::Jump);
                self.patch_jump(else_jump);
                self.compile_expr(else_br)?;
                self.patch_jump(end_jump);
            }

            ExprKind::Let(decls, body) => {
                self.begin_scope();
                for d in decls {
                    self.compile_decl(d)?;
                }
                self.compile_expr(body)?;
                self.end_scope_keep_result();
            }

            ExprKind::Case(_, _) => {
                return Err(CompileError {
                    message: "case expressions not yet implemented (Phase 4)".to_string(),
                });
            }

            ExprKind::Ann(e, _) => self.compile_expr(e)?,
            ExprKind::Paren(e) => self.compile_expr(e)?,
        }
        Ok(())
    }
}

fn simple_pat_name(pat: &Pat) -> String {
    match &pat.kind {
        PatKind::Var(name) => name.clone(),
        PatKind::Paren(p) => simple_pat_name(p),
        PatKind::Ann(p, _) => simple_pat_name(p),
        _ => "_".to_string(),
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
