use crate::ast::*;
use crate::intern::StringInterner;

pub fn desugar_program(mut program: Program) -> Program {
    let mut interner = std::mem::take(&mut program.interner);
    program.decls = program
        .decls
        .into_iter()
        .map(|d| desugar_decl(d, &mut interner))
        .collect();
    program.interner = interner;
    program
}

pub fn desugar_decl(decl: Decl, interner: &mut StringInterner) -> Decl {
    let span = decl.span;
    let kind = match decl.kind {
        DeclKind::Val(pat, expr) => {
            DeclKind::Val(desugar_pat(pat, interner), desugar_expr(expr, interner))
        }
        DeclKind::ValRec(name, expr) => DeclKind::ValRec(name, desugar_expr(expr, interner)),
        DeclKind::Fun(bindings) => {
            let bindings = bindings
                .into_iter()
                .map(|b| desugar_fun_binding(b, interner))
                .collect();
            DeclKind::Fun(bindings)
        }
        DeclKind::Datatype(dt) => DeclKind::Datatype(dt),
        DeclKind::TypeAlias(ta) => DeclKind::TypeAlias(ta),
        DeclKind::Local(locals, body) => DeclKind::Local(
            locals
                .into_iter()
                .map(|d| desugar_decl(d, interner))
                .collect(),
            body.into_iter()
                .map(|d| desugar_decl(d, interner))
                .collect(),
        ),
        DeclKind::Use(path) => DeclKind::Use(path),
        DeclKind::Effect(name, ty) => DeclKind::Effect(name, ty),
    };
    Decl { kind, span }
}

fn desugar_expr(expr: Expr, interner: &mut StringInterner) -> Expr {
    let span = expr.span;
    let kind = match expr.kind {
        // Unwrap parentheses
        ExprKind::Paren(e) => return desugar_expr(*e, interner),

        // List literal: desugar [e1, e2, e3] to e1 :: e2 :: e3 :: []
        ExprKind::List(elems) if !elems.is_empty() => {
            let mut result = Expr {
                kind: ExprKind::List(vec![]),
                span,
            };
            for elem in elems.into_iter().rev() {
                result = Expr {
                    kind: ExprKind::Cons(Box::new(desugar_expr(elem, interner)), Box::new(result)),
                    span,
                };
            }
            return result;
        }

        // andalso/orelse: desugar to if-then-else
        ExprKind::BinOp(BinOp::Andalso, lhs, rhs) => ExprKind::If(
            Box::new(desugar_expr(*lhs, interner)),
            Box::new(desugar_expr(*rhs, interner)),
            Box::new(Expr {
                kind: ExprKind::BoolLit(false),
                span,
            }),
        ),
        ExprKind::BinOp(BinOp::Orelse, lhs, rhs) => ExprKind::If(
            Box::new(desugar_expr(*lhs, interner)),
            Box::new(Expr {
                kind: ExprKind::BoolLit(true),
                span,
            }),
            Box::new(desugar_expr(*rhs, interner)),
        ),

        // not: desugar to if e then false else true
        ExprKind::Not(e) => ExprKind::If(
            Box::new(desugar_expr(*e, interner)),
            Box::new(Expr {
                kind: ExprKind::BoolLit(false),
                span,
            }),
            Box::new(Expr {
                kind: ExprKind::BoolLit(true),
                span,
            }),
        ),

        // Recursive cases
        ExprKind::BinOp(op, lhs, rhs) => ExprKind::BinOp(
            op,
            Box::new(desugar_expr(*lhs, interner)),
            Box::new(desugar_expr(*rhs, interner)),
        ),
        ExprKind::UnaryNeg(e) => ExprKind::UnaryNeg(Box::new(desugar_expr(*e, interner))),
        ExprKind::App(f, arg) => ExprKind::App(
            Box::new(desugar_expr(*f, interner)),
            Box::new(desugar_expr(*arg, interner)),
        ),
        ExprKind::Fn(pat, body) => ExprKind::Fn(
            desugar_pat(pat, interner),
            Box::new(desugar_expr(*body, interner)),
        ),
        ExprKind::If(c, t, e) => ExprKind::If(
            Box::new(desugar_expr(*c, interner)),
            Box::new(desugar_expr(*t, interner)),
            Box::new(desugar_expr(*e, interner)),
        ),
        ExprKind::Let(decls, body) => ExprKind::Let(
            decls
                .into_iter()
                .map(|d| desugar_decl(d, interner))
                .collect(),
            Box::new(desugar_expr(*body, interner)),
        ),
        ExprKind::Case(scrutinee, arms) => ExprKind::Case(
            Box::new(desugar_expr(*scrutinee, interner)),
            arms.into_iter()
                .map(|(pat, body)| (desugar_pat(pat, interner), desugar_expr(body, interner)))
                .collect(),
        ),
        ExprKind::Tuple(elems) => ExprKind::Tuple(
            elems
                .into_iter()
                .map(|e| desugar_expr(e, interner))
                .collect(),
        ),
        ExprKind::Cons(hd, tl) => ExprKind::Cons(
            Box::new(desugar_expr(*hd, interner)),
            Box::new(desugar_expr(*tl, interner)),
        ),
        ExprKind::Ann(e, ty) => ExprKind::Ann(Box::new(desugar_expr(*e, interner)), ty),
        ExprKind::Perform(name, arg) => {
            ExprKind::Perform(name, Box::new(desugar_expr(*arg, interner)))
        }
        ExprKind::Handle {
            body,
            return_var,
            return_body,
            handlers,
        } => ExprKind::Handle {
            body: Box::new(desugar_expr(*body, interner)),
            return_var,
            return_body: Box::new(desugar_expr(*return_body, interner)),
            handlers: handlers
                .into_iter()
                .map(|h| EffectHandler {
                    body: desugar_expr(h.body, interner),
                    ..h
                })
                .collect(),
        },
        ExprKind::Resume(cont, arg) => ExprKind::Resume(
            Box::new(desugar_expr(*cont, interner)),
            Box::new(desugar_expr(*arg, interner)),
        ),

        // Leaves (pass through)
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::CharLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Unit
        | ExprKind::Var(_)
        | ExprKind::Constructor(_)
        | ExprKind::List(_) => expr.kind,
    };
    Expr { kind, span }
}

#[allow(clippy::only_used_in_recursion)]
fn desugar_pat(pat: Pat, interner: &mut StringInterner) -> Pat {
    let span = pat.span;
    let kind = match pat.kind {
        // Unwrap parentheses
        PatKind::Paren(p) => return desugar_pat(*p, interner),

        // List pattern: desugar [p1, p2] to p1 :: p2 :: []
        PatKind::List(pats) if !pats.is_empty() => {
            let mut result = Pat {
                kind: PatKind::List(vec![]),
                span,
            };
            for p in pats.into_iter().rev() {
                result = Pat {
                    kind: PatKind::Cons(Box::new(desugar_pat(p, interner)), Box::new(result)),
                    span,
                };
            }
            return result;
        }

        // Recursive cases
        PatKind::Tuple(pats) => {
            PatKind::Tuple(pats.into_iter().map(|p| desugar_pat(p, interner)).collect())
        }
        PatKind::Constructor(name, payload) => {
            PatKind::Constructor(name, payload.map(|p| Box::new(desugar_pat(*p, interner))))
        }
        PatKind::Cons(hd, tl) => PatKind::Cons(
            Box::new(desugar_pat(*hd, interner)),
            Box::new(desugar_pat(*tl, interner)),
        ),
        PatKind::Ann(p, ty) => PatKind::Ann(Box::new(desugar_pat(*p, interner)), ty),
        PatKind::As(name, p) => PatKind::As(name, Box::new(desugar_pat(*p, interner))),

        // Leaves
        PatKind::Wildcard
        | PatKind::Var(_)
        | PatKind::IntLit(_)
        | PatKind::FloatLit(_)
        | PatKind::StringLit(_)
        | PatKind::CharLit(_)
        | PatKind::BoolLit(_)
        | PatKind::Unit
        | PatKind::List(_) => pat.kind,
    };
    Pat { kind, span }
}

/// Desugar a fun binding: multi-clause -> single-clause with case
fn desugar_fun_binding(mut binding: FunBinding, interner: &mut StringInterner) -> FunBinding {
    // Desugar sub-expressions in all clauses first
    for clause in &mut binding.clauses {
        clause.pats = clause
            .pats
            .drain(..)
            .map(|p| desugar_pat(p, interner))
            .collect();
        clause.body = desugar_expr(
            std::mem::replace(
                &mut clause.body,
                Expr {
                    kind: ExprKind::Unit,
                    span: clause.span,
                },
            ),
            interner,
        );
    }

    // Single clause, keep as is
    if binding.clauses.len() == 1 {
        return binding;
    }

    // Multi-clause: desugar to single clause with case
    let span = binding.span;
    let arity = binding.clauses[0].pats.len();

    let arg_names: Vec<_> = (0..arity)
        .map(|i| interner.intern(&format!("_arg{i}")))
        .collect();

    // Build case arms from clauses
    let arms: Vec<(Pat, Expr)> = binding
        .clauses
        .drain(..)
        .map(|clause| {
            let pat = if arity == 1 {
                clause.pats.into_iter().next().unwrap()
            } else {
                Pat {
                    kind: PatKind::Tuple(clause.pats),
                    span: clause.span,
                }
            };
            (pat, clause.body)
        })
        .collect();

    // Build scrutinee
    let scrutinee = if arity == 1 {
        Expr {
            kind: ExprKind::Var(arg_names[0]),
            span,
        }
    } else {
        Expr {
            kind: ExprKind::Tuple(
                arg_names
                    .iter()
                    .map(|n| Expr {
                        kind: ExprKind::Var(*n),
                        span,
                    })
                    .collect(),
            ),
            span,
        }
    };

    let case_expr = Expr {
        kind: ExprKind::Case(Box::new(scrutinee), arms),
        span,
    };

    // Build pattern list for the single clause
    let pats: Vec<Pat> = arg_names
        .into_iter()
        .map(|n| Pat {
            kind: PatKind::Var(n),
            span,
        })
        .collect();

    binding.clauses = vec![FunClause {
        pats,
        body: case_expr,
        span,
    }];

    binding
}
