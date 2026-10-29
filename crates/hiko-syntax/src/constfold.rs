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
        // No expressions inside.
        other => other,
    };
    Decl { kind, span }
}

fn fold_expr(expr: Expr) -> Expr {
    let span = expr.span;
    let kind = match expr.kind {
        // Folding runs before type inference, so both branches must stay in
        // the tree even when the condition is a literal: dropping the dead
        // branch would let ill-typed or unbound code through the checker.
        ExprKind::If(cond, then_br, else_br) => ExprKind::If(
            Box::new(fold_expr(*cond)),
            Box::new(fold_expr(*then_br)),
            Box::new(fold_expr(*else_br)),
        ),

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
        (BinOp::Pipe, _, _) => None,

        // Generic Add: int + int, float + float, word + word
        (BinOp::Add, ExprKind::IntLit(a), ExprKind::IntLit(b)) => {
            a.checked_add(*b).map(ExprKind::IntLit)
        }
        (BinOp::Add, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::FloatLit(a + b))
        }
        (BinOp::Add, ExprKind::WordLit(a), ExprKind::WordLit(b)) => {
            Some(ExprKind::WordLit(a.wrapping_add(*b)))
        }

        // Generic Sub
        (BinOp::Sub, ExprKind::IntLit(a), ExprKind::IntLit(b)) => {
            a.checked_sub(*b).map(ExprKind::IntLit)
        }
        (BinOp::Sub, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::FloatLit(a - b))
        }
        (BinOp::Sub, ExprKind::WordLit(a), ExprKind::WordLit(b)) => {
            Some(ExprKind::WordLit(a.wrapping_sub(*b)))
        }

        // Generic Mul
        (BinOp::Mul, ExprKind::IntLit(a), ExprKind::IntLit(b)) => {
            a.checked_mul(*b).map(ExprKind::IntLit)
        }
        (BinOp::Mul, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::FloatLit(a * b))
        }
        (BinOp::Mul, ExprKind::WordLit(a), ExprKind::WordLit(b)) => {
            Some(ExprKind::WordLit(a.wrapping_mul(*b)))
        }

        // Generic Div
        (BinOp::Div, ExprKind::IntLit(a), ExprKind::IntLit(b)) if *b != 0 => {
            a.checked_div(*b).map(ExprKind::IntLit)
        }
        (BinOp::Div, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::FloatLit(a / b))
        }
        (BinOp::Div, ExprKind::WordLit(a), ExprKind::WordLit(b)) if *b != 0 => {
            Some(ExprKind::WordLit(a / b))
        }

        // Generic Mod
        (BinOp::Mod, ExprKind::IntLit(a), ExprKind::IntLit(b)) if *b != 0 => {
            a.checked_rem(*b).map(ExprKind::IntLit)
        }
        (BinOp::Mod, ExprKind::WordLit(a), ExprKind::WordLit(b)) if *b != 0 => {
            Some(ExprKind::WordLit(a % b))
        }

        // String concat
        (BinOp::ConcatStr, ExprKind::StringLit(a), ExprKind::StringLit(b)) => {
            let mut s = a.clone();
            s.push_str(b);
            Some(ExprKind::StringLit(s))
        }

        // Equality
        (BinOp::Eq, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a == b)),
        (BinOp::Ne, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a != b)),
        (BinOp::Eq, ExprKind::WordLit(a), ExprKind::WordLit(b)) => Some(ExprKind::BoolLit(a == b)),
        (BinOp::Ne, ExprKind::WordLit(a), ExprKind::WordLit(b)) => Some(ExprKind::BoolLit(a != b)),

        // Generic comparison: int
        (BinOp::Lt, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a < b)),
        (BinOp::Gt, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a > b)),
        (BinOp::Le, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a <= b)),
        (BinOp::Ge, ExprKind::IntLit(a), ExprKind::IntLit(b)) => Some(ExprKind::BoolLit(a >= b)),

        // Generic comparison: float
        (BinOp::Lt, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => Some(ExprKind::BoolLit(a < b)),
        (BinOp::Gt, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => Some(ExprKind::BoolLit(a > b)),
        (BinOp::Le, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::BoolLit(a <= b))
        }
        (BinOp::Ge, ExprKind::FloatLit(a), ExprKind::FloatLit(b)) => {
            Some(ExprKind::BoolLit(a >= b))
        }

        // Generic comparison: word
        (BinOp::Lt, ExprKind::WordLit(a), ExprKind::WordLit(b)) => Some(ExprKind::BoolLit(a < b)),
        (BinOp::Gt, ExprKind::WordLit(a), ExprKind::WordLit(b)) => Some(ExprKind::BoolLit(a > b)),
        (BinOp::Le, ExprKind::WordLit(a), ExprKind::WordLit(b)) => Some(ExprKind::BoolLit(a <= b)),
        (BinOp::Ge, ExprKind::WordLit(a), ExprKind::WordLit(b)) => Some(ExprKind::BoolLit(a >= b)),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn fold_val_expr(src: &str) -> ExprKind {
        let tokens = Lexer::new(src, 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let program = fold_program(crate::desugar::desugar_program(program));
        match program.decls.into_iter().next().unwrap().kind {
            DeclKind::Val(_, expr) => expr.kind,
            other => panic!("expected val, got {other:?}"),
        }
    }

    #[test]
    fn literal_if_keeps_both_branches_for_type_checking() {
        let kind = fold_val_expr("val x = if true then 1 else \"dead\"");
        let ExprKind::If(cond, then_br, else_br) = kind else {
            panic!("expected if to survive folding, got {kind:?}");
        };
        assert_eq!(cond.kind, ExprKind::BoolLit(true));
        assert_eq!(then_br.kind, ExprKind::IntLit(1));
        assert_eq!(else_br.kind, ExprKind::StringLit("dead".into()));
    }

    #[test]
    fn folds_branches_of_literal_if() {
        let kind = fold_val_expr("val x = if 1 < 2 then 1 + 2 else 3");
        let ExprKind::If(cond, then_br, _) = kind else {
            panic!("expected if, got {kind:?}");
        };
        assert_eq!(cond.kind, ExprKind::BoolLit(true));
        assert_eq!(then_br.kind, ExprKind::IntLit(3));
    }
}
