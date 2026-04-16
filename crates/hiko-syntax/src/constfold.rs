use crate::ast::*;

pub fn fold_program(mut program: Program) -> Program {
    program.decls = program.decls.into_iter().map(fold_decl).collect();
    program
}

fn fold_decl(decl: Decl) -> Decl {
    let span = decl.span;
    let kind = match decl.kind {
        DeclKind::Val(pat, expr) => DeclKind::Val(pat, fold_expr(expr)),
        DeclKind::ValRec(name, expr) => DeclKind::ValRec(name, fold_expr(expr)),
        DeclKind::Fun(bindings) => DeclKind::Fun(
            bindings
                .into_iter()
                .map(|mut b| {
                    b.clauses = b
                        .clauses
                        .into_iter()
                        .map(|mut c| {
                            c.body = fold_expr(c.body);
                            c
                        })
                        .collect();
                    b
                })
                .collect(),
        ),
        DeclKind::Local(locals, body) => DeclKind::Local(
            locals.into_iter().map(fold_decl).collect(),
            body.into_iter().map(fold_decl).collect(),
        ),
        DeclKind::Signature(sig) => DeclKind::Signature(sig),
        DeclKind::Structure {
            name,
            signature,
            opaque,
            decls,
        } => DeclKind::Structure {
            name,
            signature,
            opaque,
            decls: decls.into_iter().map(fold_decl).collect(),
        },
        other => other,
    };
    Decl { kind, span }
}

fn fold_expr(expr: Expr) -> Expr {
    let span = expr.span;
    let kind = match expr.kind {
        // Constant fold: if true/false
        ExprKind::If(cond, then_br, else_br) => {
            let cond = fold_expr(*cond);
            let then_br = fold_expr(*then_br);
            let else_br = fold_expr(*else_br);
            match &cond.kind {
                ExprKind::BoolLit(true) => return then_br,
                ExprKind::BoolLit(false) => return else_br,
                _ => ExprKind::If(Box::new(cond), Box::new(then_br), Box::new(else_br)),
            }
        }

        // Constant fold: integer arithmetic
        ExprKind::BinOp(op, lhs, rhs) => {
            let lhs = fold_expr(*lhs);
            let rhs = fold_expr(*rhs);
            if let Some(result) = try_fold_binop(op, &lhs, &rhs) {
                return Expr { kind: result, span };
            }
            ExprKind::BinOp(op, Box::new(lhs), Box::new(rhs))
        }

        // Constant fold: negation
        ExprKind::UnaryNeg(e) => {
            let e = fold_expr(*e);
            match &e.kind {
                ExprKind::IntLit(n) => match n.checked_neg() {
                    Some(neg) => ExprKind::IntLit(neg),
                    None => ExprKind::UnaryNeg(Box::new(e)),
                },
                ExprKind::FloatLit(f) => ExprKind::FloatLit(-f),
                _ => ExprKind::UnaryNeg(Box::new(e)),
            }
        }

        // Recurse into sub-expressions
        ExprKind::App(f, arg) => ExprKind::App(Box::new(fold_expr(*f)), Box::new(fold_expr(*arg))),
        ExprKind::Fn(pat, body) => ExprKind::Fn(pat, Box::new(fold_expr(*body))),
        ExprKind::Let(decls, body) => ExprKind::Let(
            decls.into_iter().map(fold_decl).collect(),
            Box::new(fold_expr(*body)),
        ),
        ExprKind::Case(scrutinee, arms) => ExprKind::Case(
            Box::new(fold_expr(*scrutinee)),
            arms.into_iter()
                .map(|(pat, body)| (pat, fold_expr(body)))
                .collect(),
        ),
        ExprKind::Tuple(elems) => ExprKind::Tuple(elems.into_iter().map(fold_expr).collect()),
        ExprKind::Cons(hd, tl) => {
            ExprKind::Cons(Box::new(fold_expr(*hd)), Box::new(fold_expr(*tl)))
        }
        ExprKind::Ann(e, ty) => ExprKind::Ann(Box::new(fold_expr(*e)), ty),
        ExprKind::Perform(name, arg) => ExprKind::Perform(name, Box::new(fold_expr(*arg))),
        ExprKind::Handle {
            body,
            return_var,
            return_body,
            handlers,
        } => ExprKind::Handle {
            body: Box::new(fold_expr(*body)),
            return_var,
            return_body: Box::new(fold_expr(*return_body)),
            handlers: handlers
                .into_iter()
                .map(|h| EffectHandler {
                    body: fold_expr(h.body),
                    ..h
                })
                .collect(),
        },
        ExprKind::Resume(cont, arg) => {
            ExprKind::Resume(Box::new(fold_expr(*cont)), Box::new(fold_expr(*arg)))
        }

        // Leaves
        other => other,
    };
    Expr { kind, span }
}

fn try_fold_binop(op: BinOp, lhs: &Expr, rhs: &Expr) -> Option<ExprKind> {
    match (op, &lhs.kind, &rhs.kind) {
        // Int arithmetic
        (BinOp::AddInt, ExprKind::IntLit(a), ExprKind::IntLit(b)) => {
            a.checked_add(*b).map(ExprKind::IntLit)
        }
        (BinOp::SubInt, ExprKind::IntLit(a), ExprKind::IntLit(b)) => {
            a.checked_sub(*b).map(ExprKind::IntLit)
        }
        (BinOp::MulInt, ExprKind::IntLit(a), ExprKind::IntLit(b)) => {
            a.checked_mul(*b).map(ExprKind::IntLit)
        }
        (BinOp::DivInt, ExprKind::IntLit(a), ExprKind::IntLit(b)) if *b != 0 => {
            a.checked_div(*b).map(ExprKind::IntLit)
        }
        (BinOp::ModInt, ExprKind::IntLit(a), ExprKind::IntLit(b)) if *b != 0 => {
            a.checked_rem(*b).map(ExprKind::IntLit)
        }

        // Float arithmetic
        (BinOp::AddFloat, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::FloatLit(a + b))
        }
        (BinOp::SubFloat, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::FloatLit(a - b))
        }
        (BinOp::MulFloat, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::FloatLit(a * b))
        }
        (BinOp::DivFloat, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::FloatLit(a / b))
        }

        // String concat
        (BinOp::ConcatStr, ExprKind::StringLit(a), ExprKind::StringLit(b)) => {
            let mut s = a.clone();
            s.push_str(b);
            Some(ExprKind::StringLit(s))
        }

        // Int comparison
        (BinOp::Eq, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a == b)),
        (BinOp::Ne, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a != b)),
        (BinOp::LtInt, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a < b)),
        (BinOp::GtInt, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a > b)),
        (BinOp::LeInt, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a <= b)),
        (BinOp::GeInt, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a >= b)),

        _ => None,
    }
}
