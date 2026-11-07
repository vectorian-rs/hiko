//! Decide whether an exported function needs its enclosing private values.
//! Run before reserving direct-call targets, including recursive group calls.

use std::collections::HashSet;

use hiko_syntax::ast::*;
use hiko_syntax::intern::{StringInterner, Symbol};

pub(crate) struct PrivateCaptures<'a> {
    private: HashSet<&'a str>,
    interner: &'a StringInterner,
    bound: HashSet<Symbol>,
    added: Vec<Symbol>,
}

impl<'a> PrivateCaptures<'a> {
    pub(crate) fn new(private: HashSet<&'a str>, interner: &'a StringInterner) -> Self {
        Self {
            private,
            interner,
            bound: HashSet::new(),
            added: Vec::new(),
        }
    }

    fn bind(&mut self, name: Symbol) {
        if self.bound.insert(name) {
            self.added.push(name);
        }
    }

    fn restore(&mut self, mark: usize) {
        for name in self.added.drain(mark..) {
            self.bound.remove(&name);
        }
    }

    fn var(&self, name: Symbol) -> bool {
        !self.bound.contains(&name) && self.private.contains(self.interner.resolve(name))
    }

    fn pattern(&mut self, pat: &Pat) {
        let mut names = Vec::new();
        pattern_names(pat, &mut names);
        for name in names {
            self.bind(name);
        }
    }

    pub(crate) fn function(&mut self, binding: &FunBinding) -> bool {
        binding.clauses.iter().any(|clause| {
            let mark = self.added.len();
            for pat in &clause.pats {
                self.pattern(pat);
            }
            let captures = self.expr(&clause.body);
            self.restore(mark);
            captures
        })
    }

    fn expr(&mut self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Var(name) | ExprKind::Constructor(name) => self.var(*name),
            ExprKind::Tuple(items) | ExprKind::List(items) => items.iter().any(|e| self.expr(e)),
            ExprKind::Cons(a, b)
            | ExprKind::BinOp(_, a, b)
            | ExprKind::App(a, b)
            | ExprKind::Resume(a, b) => self.expr(a) || self.expr(b),
            ExprKind::UnaryNeg(e)
            | ExprKind::Not(e)
            | ExprKind::Ann(e, _)
            | ExprKind::Paren(e)
            | ExprKind::Perform(_, e) => self.expr(e),
            ExprKind::Fn(pat, body) => {
                let mark = self.added.len();
                self.pattern(pat);
                let captures = self.expr(body);
                self.restore(mark);
                captures
            }
            ExprKind::If(a, b, c) => self.expr(a) || self.expr(b) || self.expr(c),
            ExprKind::Let(decls, body) => {
                let mark = self.added.len();
                let captures =
                    decls.iter().any(|d| self.decl(d, &mut Vec::new())) || self.expr(body);
                self.restore(mark);
                captures
            }
            ExprKind::Case(value, arms) => {
                self.expr(value)
                    || arms.iter().any(|(pat, body)| {
                        let mark = self.added.len();
                        self.pattern(pat);
                        let captures = self.expr(body);
                        self.restore(mark);
                        captures
                    })
            }
            ExprKind::Handle {
                body,
                return_var,
                return_body,
                handlers,
            } => {
                if self.expr(body) {
                    return true;
                }
                let mark = self.added.len();
                self.bind(*return_var);
                let captures = self.expr(return_body);
                self.restore(mark);
                captures
                    || handlers.iter().any(|handler| {
                        self.bind(handler.payload_var);
                        self.bind(handler.cont_var);
                        let captures = self.expr(&handler.body);
                        self.restore(mark);
                        captures
                    })
            }
            ExprKind::IntLit(_)
            | ExprKind::FloatLit(_)
            | ExprKind::WordLit(_)
            | ExprKind::StringLit(_)
            | ExprKind::CharLit(_)
            | ExprKind::BoolLit(_)
            | ExprKind::Unit => false,
        }
    }

    // Track the exports separately: a local body's binding may shadow a name
    // already added by its private declarations, so the scope log alone is
    // insufficient to restore only the public bindings.
    fn decl(&mut self, decl: &Decl, exports: &mut Vec<Symbol>) -> bool {
        match &decl.kind {
            DeclKind::Val(pat, expr) => {
                if self.expr(expr) {
                    return true;
                }
                if let (PatKind::Constructor(alias, None), ExprKind::Constructor(_)) =
                    (&pat.kind, &expr.kind)
                {
                    self.bind(*alias);
                    exports.push(*alias);
                } else {
                    let mut names = Vec::new();
                    pattern_names(pat, &mut names);
                    for name in names {
                        self.bind(name);
                        exports.push(name);
                    }
                }
                false
            }
            DeclKind::ValRec(name, expr) => {
                self.bind(*name);
                exports.push(*name);
                self.expr(expr)
            }
            DeclKind::Fun(bindings) => {
                for binding in bindings {
                    self.bind(binding.name);
                    exports.push(binding.name);
                }
                bindings.iter().any(|binding| self.function(binding))
            }
            DeclKind::Datatype(dt) => {
                for con in &dt.constructors {
                    self.bind(con.name);
                    exports.push(con.name);
                }
                false
            }
            DeclKind::Local(private, body) => {
                let mark = self.added.len();
                let mut public = Vec::new();
                let captures = private.iter().any(|d| self.decl(d, &mut Vec::new()))
                    || body.iter().any(|d| self.decl(d, &mut public));
                self.restore(mark);
                for name in public {
                    self.bind(name);
                    exports.push(name);
                }
                captures
            }
            DeclKind::ExportVal {
                public_name,
                internal_name,
                ..
            } => {
                let captures = self.var(*internal_name);
                self.bind(*public_name);
                exports.push(*public_name);
                captures
            }
            DeclKind::TypeAlias(_)
            | DeclKind::AbstractType(_)
            | DeclKind::Effect(_, _)
            | DeclKind::Import(_)
            | DeclKind::ImportWithNames(_, _)
            | DeclKind::Use(_)
            | DeclKind::Signature(_)
            | DeclKind::Structure { .. } => false,
        }
    }
}

fn pattern_names(pat: &Pat, names: &mut Vec<Symbol>) {
    match &pat.kind {
        PatKind::Var(name) => names.push(*name),
        PatKind::As(name, inner) => {
            names.push(*name);
            pattern_names(inner, names);
        }
        PatKind::Tuple(items) | PatKind::List(items) => {
            for item in items {
                pattern_names(item, names);
            }
        }
        PatKind::Cons(head, tail) => {
            pattern_names(head, names);
            pattern_names(tail, names);
        }
        PatKind::Constructor(_, Some(inner)) | PatKind::Ann(inner, _) | PatKind::Paren(inner) => {
            pattern_names(inner, names)
        }
        _ => {}
    }
}
