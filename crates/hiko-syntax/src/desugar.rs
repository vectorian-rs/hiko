use crate::ast::*;

pub fn desugar_program(mut program: Program) -> Program {
    program.decls = program.decls.into_iter().map(desugar_decl).collect();
    program
}

pub fn desugar_decl(decl: Decl) -> Decl {
    let span = decl.span;
    let kind = match decl.kind {
        DeclKind::Val(pat, expr) => DeclKind::Val(desugar_pat(pat), desugar_expr(expr)),
        DeclKind::ValRec(name, expr) => DeclKind::ValRec(name, desugar_expr(expr)),
        DeclKind::Fun(bindings) => {
            let bindings = bindings.into_iter().map(desugar_fun_binding).collect();
            DeclKind::Fun(bindings)
        }
        DeclKind::Datatype(dt) => DeclKind::Datatype(dt),
        DeclKind::TypeAlias(ta) => DeclKind::TypeAlias(ta),
        DeclKind::Local(locals, body) => DeclKind::Local(
            locals.into_iter().map(desugar_decl).collect(),
            body.into_iter().map(desugar_decl).collect(),
        ),
        DeclKind::Use(path) => DeclKind::Use(path),
        DeclKind::Effect(name, ty) => DeclKind::Effect(name, ty),
    };
    Decl { kind, span }
}

fn desugar_expr(expr: Expr) -> Expr {
    let span = expr.span;
    let kind = match expr.kind {
        // Unwrap parentheses
        ExprKind::Paren(e) => return desugar_expr(*e),

        // List literal: desugar [e1, e2, e3] to e1 :: e2 :: e3 :: []
        ExprKind::List(elems) if !elems.is_empty() => {
            let mut result = Expr {
                kind: ExprKind::List(vec![]),
                span,
            };
            for elem in elems.into_iter().rev() {
                result = Expr {
                    kind: ExprKind::Cons(Box::new(desugar_expr(elem)), Box::new(result)),
                    span,
                };
            }
            return result;
        }

        // andalso/orelse: desugar to if-then-else
        ExprKind::BinOp(BinOp::Andalso, lhs, rhs) => ExprKind::If(
            Box::new(desugar_expr(*lhs)),
            Box::new(desugar_expr(*rhs)),
            Box::new(Expr {
                kind: ExprKind::BoolLit(false),
                span,
            }),
        ),
        ExprKind::BinOp(BinOp::Orelse, lhs, rhs) => ExprKind::If(
            Box::new(desugar_expr(*lhs)),
            Box::new(Expr {
                kind: ExprKind::BoolLit(true),
                span,
            }),
            Box::new(desugar_expr(*rhs)),
        ),

        // not: desugar to if e then false else true
        ExprKind::Not(e) => ExprKind::If(
            Box::new(desugar_expr(*e)),
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
            Box::new(desugar_expr(*lhs)),
            Box::new(desugar_expr(*rhs)),
        ),
        ExprKind::UnaryNeg(e) => ExprKind::UnaryNeg(Box::new(desugar_expr(*e))),
        ExprKind::App(f, arg) => {
            ExprKind::App(Box::new(desugar_expr(*f)), Box::new(desugar_expr(*arg)))
        }
        ExprKind::Fn(pat, body) => ExprKind::Fn(desugar_pat(pat), Box::new(desugar_expr(*body))),
        ExprKind::If(c, t, e) => ExprKind::If(
            Box::new(desugar_expr(*c)),
            Box::new(desugar_expr(*t)),
            Box::new(desugar_expr(*e)),
        ),
        ExprKind::Let(decls, body) => ExprKind::Let(
            decls.into_iter().map(desugar_decl).collect(),
            Box::new(desugar_expr(*body)),
        ),
        ExprKind::Case(scrutinee, arms) => ExprKind::Case(
            Box::new(desugar_expr(*scrutinee)),
            arms.into_iter()
                .map(|(pat, body)| (desugar_pat(pat), desugar_expr(body)))
                .collect(),
        ),
        ExprKind::Tuple(elems) => ExprKind::Tuple(elems.into_iter().map(desugar_expr).collect()),
        ExprKind::Cons(hd, tl) => {
            ExprKind::Cons(Box::new(desugar_expr(*hd)), Box::new(desugar_expr(*tl)))
        }
        ExprKind::Ann(e, ty) => ExprKind::Ann(Box::new(desugar_expr(*e)), ty),
        ExprKind::Perform(name, arg) => ExprKind::Perform(name, Box::new(desugar_expr(*arg))),
        ExprKind::Handle {
            body,
            return_var,
            return_body,
            handlers,
        } => ExprKind::Handle {
            body: Box::new(desugar_expr(*body)),
            return_var,
            return_body: Box::new(desugar_expr(*return_body)),
            handlers: handlers
                .into_iter()
                .map(|h| EffectHandler {
                    body: desugar_expr(h.body),
                    ..h
                })
                .collect(),
        },
        ExprKind::Resume(cont, arg) => {
            ExprKind::Resume(Box::new(desugar_expr(*cont)), Box::new(desugar_expr(*arg)))
        }

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

fn desugar_pat(pat: Pat) -> Pat {
    let span = pat.span;
    let kind = match pat.kind {
        // Unwrap parentheses
        PatKind::Paren(p) => return desugar_pat(*p),

        // List pattern: desugar [p1, p2] to p1 :: p2 :: []
        PatKind::List(pats) if !pats.is_empty() => {
            let mut result = Pat {
                kind: PatKind::List(vec![]),
                span,
            };
            for p in pats.into_iter().rev() {
                result = Pat {
                    kind: PatKind::Cons(Box::new(desugar_pat(p)), Box::new(result)),
                    span,
                };
            }
            return result;
        }

        // Recursive cases
        PatKind::Tuple(pats) => PatKind::Tuple(pats.into_iter().map(desugar_pat).collect()),
        PatKind::Constructor(name, payload) => {
            PatKind::Constructor(name, payload.map(|p| Box::new(desugar_pat(*p))))
        }
        PatKind::Cons(hd, tl) => {
            PatKind::Cons(Box::new(desugar_pat(*hd)), Box::new(desugar_pat(*tl)))
        }
        PatKind::Ann(p, ty) => PatKind::Ann(Box::new(desugar_pat(*p)), ty),
        PatKind::As(name, p) => PatKind::As(name, Box::new(desugar_pat(*p))),

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

/// Desugar a fun binding: multi-clause → single-clause with case
fn desugar_fun_binding(mut binding: FunBinding) -> FunBinding {
    // Desugar sub-expressions in all clauses first
    for clause in &mut binding.clauses {
        clause.pats = clause.pats.drain(..).map(desugar_pat).collect();
        clause.body = desugar_expr(std::mem::replace(
            &mut clause.body,
            Expr {
                kind: ExprKind::Unit,
                span: clause.span,
            },
        ));
    }

    // Single clause, keep as is
    if binding.clauses.len() == 1 {
        return binding;
    }

    // Multi-clause: desugar to single clause with case
    let span = binding.span;
    let arity = binding.clauses[0].pats.len();

    let arg_names: Vec<String> = (0..arity).map(|i| format!("_arg{i}")).collect();

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
            kind: ExprKind::Var(arg_names[0].clone()),
            span,
        }
    } else {
        Expr {
            kind: ExprKind::Tuple(
                arg_names
                    .iter()
                    .map(|n| Expr {
                        kind: ExprKind::Var(n.clone()),
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
