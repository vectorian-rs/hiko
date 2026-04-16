use crate::ast::*;
use crate::intern::StringInterner;
use std::fmt::Write;

pub fn pretty_program(prog: &Program) -> String {
    let interner = &prog.interner;
    let mut buf = String::new();
    for (i, decl) in prog.decls.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        pretty_decl(&mut buf, decl, 0, interner);
    }
    buf
}

// ── Declarations ─────────────────────────────────────────────────────

fn pretty_decl(buf: &mut String, decl: &Decl, indent: usize, interner: &StringInterner) {
    match &decl.kind {
        DeclKind::Val(pat, expr) => {
            write_indent(buf, indent);
            buf.push_str("val ");
            pretty_pat(buf, pat, interner);
            buf.push_str(" = ");
            pretty_expr(buf, expr, indent, interner);
        }
        DeclKind::ValRec(name, expr) => {
            write_indent(buf, indent);
            write!(buf, "val rec {} = ", interner.resolve(*name)).unwrap();
            pretty_expr(buf, expr, indent, interner);
        }
        DeclKind::Fun(bindings) => {
            for (i, binding) in bindings.iter().enumerate() {
                write_indent(buf, indent);
                if i == 0 {
                    buf.push_str("fun ");
                } else {
                    buf.push_str("and ");
                }
                pretty_fun_binding(buf, binding, indent, interner);
            }
        }
        DeclKind::Datatype(dt) => {
            write_indent(buf, indent);
            buf.push_str("datatype ");
            pretty_tyvars(buf, &dt.tyvars, interner);
            buf.push_str(interner.resolve(dt.name));
            buf.push_str(" =");
            for (i, con) in dt.constructors.iter().enumerate() {
                if i > 0 {
                    buf.push_str(" |");
                }
                write!(buf, " {}", interner.resolve(con.name)).unwrap();
                if let Some(ref ty) = con.payload {
                    buf.push_str(" of ");
                    pretty_type(buf, ty, interner);
                }
            }
        }
        DeclKind::TypeAlias(ta) => {
            write_indent(buf, indent);
            buf.push_str("type ");
            pretty_tyvars(buf, &ta.tyvars, interner);
            write!(buf, "{} = ", interner.resolve(ta.name)).unwrap();
            pretty_type(buf, &ta.ty, interner);
        }
        DeclKind::Local(locals, body) => {
            write_indent(buf, indent);
            buf.push_str("local\n");
            for d in locals {
                pretty_decl(buf, d, indent + 2, interner);
                buf.push('\n');
            }
            write_indent(buf, indent);
            buf.push_str("in\n");
            for d in body {
                pretty_decl(buf, d, indent + 2, interner);
                buf.push('\n');
            }
            write_indent(buf, indent);
            buf.push_str("end");
        }
        DeclKind::Use(path) => {
            write_indent(buf, indent);
            buf.push_str("use ");
            write_escaped_string(buf, path);
        }
        DeclKind::Signature(sig) => {
            write_indent(buf, indent);
            write!(buf, "signature {} = sig\n", interner.resolve(sig.name)).unwrap();
            for spec in &sig.specs {
                write_indent(buf, indent + 2);
                match spec {
                    SignatureSpec::Val { name, ty, .. } => {
                        write!(buf, "val {} : ", interner.resolve(*name)).unwrap();
                        pretty_type(buf, ty, interner);
                    }
                }
                buf.push('\n');
            }
            write_indent(buf, indent);
            buf.push_str("end");
        }
        DeclKind::Structure {
            name,
            signature,
            opaque,
            decls,
        } => {
            write_indent(buf, indent);
            write!(buf, "structure {}", interner.resolve(*name)).unwrap();
            if let Some(signature) = signature {
                if *opaque {
                    write!(buf, " :> {}", interner.resolve(*signature)).unwrap();
                } else {
                    write!(buf, " : {}", interner.resolve(*signature)).unwrap();
                }
            }
            buf.push_str(" = struct\n");
            for d in decls {
                pretty_decl(buf, d, indent + 2, interner);
                buf.push('\n');
            }
            write_indent(buf, indent);
            buf.push_str("end");
        }
        DeclKind::Effect(name, payload) => {
            write_indent(buf, indent);
            write!(buf, "effect {}", interner.resolve(*name)).unwrap();
            if let Some(ty) = payload {
                buf.push_str(" of ");
                pretty_type(buf, ty, interner);
            }
        }
    }
}

fn pretty_fun_binding(
    buf: &mut String,
    binding: &FunBinding,
    indent: usize,
    interner: &StringInterner,
) {
    for (i, clause) in binding.clauses.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
            write_indent(buf, indent + 2);
            buf.push_str("| ");
        }
        buf.push_str(interner.resolve(binding.name));
        for pat in &clause.pats {
            buf.push(' ');
            pretty_atom_pat(buf, pat, interner);
        }
        buf.push_str(" = ");
        pretty_expr(buf, &clause.body, indent + 2, interner);
    }
}

fn pretty_tyvars(buf: &mut String, tyvars: &[crate::intern::Symbol], interner: &StringInterner) {
    match tyvars.len() {
        0 => {}
        1 => write!(buf, "{} ", interner.resolve(tyvars[0])).unwrap(),
        _ => {
            buf.push('(');
            for (i, tv) in tyvars.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                buf.push_str(interner.resolve(*tv));
            }
            buf.push_str(") ");
        }
    }
}

// ── Expressions ──────────────────────────────────────────────────────

fn pretty_expr(buf: &mut String, expr: &Expr, indent: usize, interner: &StringInterner) {
    match &expr.kind {
        ExprKind::IntLit(n) => write!(buf, "{n}").unwrap(),
        ExprKind::FloatLit(f) => pretty_float(buf, *f),
        ExprKind::StringLit(s) => write_escaped_string(buf, s),
        ExprKind::CharLit(c) => write_escaped_char(buf, *c),
        ExprKind::BoolLit(true) => buf.push_str("true"),
        ExprKind::BoolLit(false) => buf.push_str("false"),
        ExprKind::Unit => buf.push_str("()"),
        ExprKind::Var(sym) => buf.push_str(interner.resolve(*sym)),
        ExprKind::Constructor(sym) => buf.push_str(interner.resolve(*sym)),

        ExprKind::Tuple(elems) => {
            buf.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                pretty_expr(buf, e, indent, interner);
            }
            buf.push(')');
        }
        ExprKind::List(elems) => {
            buf.push('[');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                pretty_expr(buf, e, indent, interner);
            }
            buf.push(']');
        }
        ExprKind::Cons(hd, tl) => {
            pretty_cons_operand(buf, hd, indent, interner);
            buf.push_str(" :: ");
            pretty_expr(buf, tl, indent, interner);
        }
        ExprKind::BinOp(op, lhs, rhs) => {
            let needs_parens_lhs = binop_needs_parens_lhs(op, lhs);
            let needs_parens_rhs = binop_needs_parens_rhs(op, rhs);
            if needs_parens_lhs {
                buf.push('(');
            }
            pretty_expr(buf, lhs, indent, interner);
            if needs_parens_lhs {
                buf.push(')');
            }
            write!(buf, " {} ", binop_str(op)).unwrap();
            if needs_parens_rhs {
                buf.push('(');
            }
            pretty_expr(buf, rhs, indent, interner);
            if needs_parens_rhs {
                buf.push(')');
            }
        }
        ExprKind::UnaryNeg(e) => {
            buf.push('~');
            pretty_atom_expr(buf, e, indent, interner);
        }
        ExprKind::Not(e) => {
            buf.push_str("not ");
            pretty_atom_expr(buf, e, indent, interner);
        }
        ExprKind::App(func, arg) => {
            pretty_app_func(buf, func, indent, interner);
            buf.push(' ');
            pretty_atom_expr(buf, arg, indent, interner);
        }
        ExprKind::Fn(pat, body) => {
            buf.push_str("fn ");
            pretty_pat(buf, pat, interner);
            buf.push_str(" => ");
            pretty_expr(buf, body, indent, interner);
        }
        ExprKind::If(cond, then_br, else_br) => {
            buf.push_str("if ");
            pretty_expr(buf, cond, indent, interner);
            buf.push_str(" then ");
            pretty_expr(buf, then_br, indent, interner);
            buf.push_str(" else ");
            pretty_expr(buf, else_br, indent, interner);
        }
        ExprKind::Let(decls, body) => {
            buf.push_str("let\n");
            for d in decls {
                pretty_decl(buf, d, indent + 2, interner);
                buf.push('\n');
            }
            write_indent(buf, indent);
            buf.push_str("in\n");
            write_indent(buf, indent + 2);
            pretty_expr(buf, body, indent + 2, interner);
            buf.push('\n');
            write_indent(buf, indent);
            buf.push_str("end");
        }
        ExprKind::Case(scrutinee, branches) => {
            buf.push_str("case ");
            pretty_expr(buf, scrutinee, indent, interner);
            buf.push_str(" of\n");
            for (i, (pat, body)) in branches.iter().enumerate() {
                write_indent(buf, indent + 2);
                if i > 0 {
                    buf.push_str("| ");
                } else {
                    buf.push_str("  ");
                }
                pretty_pat(buf, pat, interner);
                buf.push_str(" => ");
                pretty_expr(buf, body, indent + 4, interner);
                if i + 1 < branches.len() {
                    buf.push('\n');
                }
            }
        }
        ExprKind::Ann(e, ty) => {
            pretty_expr(buf, e, indent, interner);
            buf.push_str(" : ");
            pretty_type(buf, ty, interner);
        }
        ExprKind::Paren(e) => {
            buf.push('(');
            pretty_expr(buf, e, indent, interner);
            buf.push(')');
        }
        ExprKind::Perform(sym, arg) => {
            write!(buf, "perform {} ", interner.resolve(*sym)).unwrap();
            pretty_atom_expr(buf, arg, indent, interner);
        }
        ExprKind::Handle {
            body,
            return_var,
            return_body,
            handlers,
        } => {
            buf.push_str("handle\n");
            write_indent(buf, indent + 2);
            pretty_expr(buf, body, indent + 2, interner);
            buf.push('\n');
            write_indent(buf, indent);
            buf.push_str("with\n");
            write_indent(buf, indent + 2);
            write!(buf, "return {} => ", interner.resolve(*return_var)).unwrap();
            pretty_expr(buf, return_body, indent + 2, interner);
            for handler in handlers {
                buf.push('\n');
                write_indent(buf, indent);
                write!(
                    buf,
                    "| {} {} {} => ",
                    interner.resolve(handler.effect_name),
                    interner.resolve(handler.payload_var),
                    interner.resolve(handler.cont_var),
                )
                .unwrap();
                pretty_expr(buf, &handler.body, indent + 2, interner);
            }
        }
        ExprKind::Resume(cont, arg) => {
            buf.push_str("resume ");
            pretty_atom_expr(buf, cont, indent, interner);
            buf.push(' ');
            pretty_atom_expr(buf, arg, indent, interner);
        }
    }
}

fn pretty_atom_expr(buf: &mut String, expr: &Expr, indent: usize, interner: &StringInterner) {
    if needs_parens_as_atom(expr) {
        buf.push('(');
        pretty_expr(buf, expr, indent, interner);
        buf.push(')');
    } else {
        pretty_expr(buf, expr, indent, interner);
    }
}

fn pretty_app_func(buf: &mut String, expr: &Expr, indent: usize, interner: &StringInterner) {
    match &expr.kind {
        ExprKind::App(_, _) | ExprKind::Var(_) | ExprKind::Constructor(_) | ExprKind::Paren(_) => {
            pretty_expr(buf, expr, indent, interner)
        }
        _ => {
            buf.push('(');
            pretty_expr(buf, expr, indent, interner);
            buf.push(')');
        }
    }
}

fn pretty_cons_operand(buf: &mut String, expr: &Expr, indent: usize, interner: &StringInterner) {
    match &expr.kind {
        ExprKind::BinOp(BinOp::Orelse | BinOp::Andalso, _, _) | ExprKind::Ann(_, _) => {
            buf.push('(');
            pretty_expr(buf, expr, indent, interner);
            buf.push(')');
        }
        _ => pretty_expr(buf, expr, indent, interner),
    }
}

fn needs_parens_as_atom(expr: &Expr) -> bool {
    !matches!(
        &expr.kind,
        ExprKind::IntLit(_)
            | ExprKind::FloatLit(_)
            | ExprKind::StringLit(_)
            | ExprKind::CharLit(_)
            | ExprKind::BoolLit(_)
            | ExprKind::Unit
            | ExprKind::Var(_)
            | ExprKind::Constructor(_)
            | ExprKind::Tuple(_)
            | ExprKind::List(_)
            | ExprKind::Paren(_)
    )
}

// ── Operator precedence for parenthesization ─────────────────────────

fn binop_prec(op: &BinOp) -> u8 {
    match op {
        BinOp::Orelse => 0,
        BinOp::Andalso => 1,
        BinOp::Eq
        | BinOp::Ne
        | BinOp::LtInt
        | BinOp::GtInt
        | BinOp::LeInt
        | BinOp::GeInt
        | BinOp::LtFloat
        | BinOp::GtFloat
        | BinOp::LeFloat
        | BinOp::GeFloat => 2,
        BinOp::AddInt | BinOp::SubInt | BinOp::AddFloat | BinOp::SubFloat | BinOp::ConcatStr => 4,
        BinOp::MulInt | BinOp::DivInt | BinOp::ModInt | BinOp::MulFloat | BinOp::DivFloat => 5,
    }
}

fn is_right_assoc(op: &BinOp) -> bool {
    matches!(op, BinOp::Orelse | BinOp::Andalso)
}

fn binop_needs_parens_lhs(op: &BinOp, lhs: &Expr) -> bool {
    if let ExprKind::BinOp(lhs_op, _, _) = &lhs.kind {
        let lp = binop_prec(lhs_op);
        let rp = binop_prec(op);
        if is_right_assoc(op) {
            lp <= rp
        } else {
            lp < rp
        }
    } else {
        false
    }
}

fn binop_needs_parens_rhs(op: &BinOp, rhs: &Expr) -> bool {
    if let ExprKind::BinOp(rhs_op, _, _) = &rhs.kind {
        let lp = binop_prec(rhs_op);
        let rp = binop_prec(op);
        if is_right_assoc(op) {
            lp < rp
        } else {
            lp <= rp
        }
    } else {
        false
    }
}

fn binop_str(op: &BinOp) -> &'static str {
    match op {
        BinOp::AddInt => "+",
        BinOp::SubInt => "-",
        BinOp::MulInt => "*",
        BinOp::DivInt => "/",
        BinOp::ModInt => "mod",
        BinOp::AddFloat => "+.",
        BinOp::SubFloat => "-.",
        BinOp::MulFloat => "*.",
        BinOp::DivFloat => "/.",
        BinOp::ConcatStr => "^",
        BinOp::LtInt => "<",
        BinOp::GtInt => ">",
        BinOp::LeInt => "<=",
        BinOp::GeInt => ">=",
        BinOp::LtFloat => "<.",
        BinOp::GtFloat => ">.",
        BinOp::LeFloat => "<=.",
        BinOp::GeFloat => ">=.",
        BinOp::Eq => "=",
        BinOp::Ne => "<>",
        BinOp::Andalso => "andalso",
        BinOp::Orelse => "orelse",
    }
}

// ── Patterns ─────────────────────────────────────────────────────────

fn pretty_pat(buf: &mut String, pat: &Pat, interner: &StringInterner) {
    match &pat.kind {
        PatKind::Wildcard => buf.push('_'),
        PatKind::Var(sym) => buf.push_str(interner.resolve(*sym)),
        PatKind::IntLit(n) => {
            if *n < 0 {
                write!(buf, "~{}", -n).unwrap();
            } else {
                write!(buf, "{n}").unwrap();
            }
        }
        PatKind::FloatLit(f) => {
            if *f < 0.0 {
                buf.push('~');
                pretty_float(buf, -f);
            } else {
                pretty_float(buf, *f);
            }
        }
        PatKind::StringLit(s) => write_escaped_string(buf, s),
        PatKind::CharLit(c) => write_escaped_char(buf, *c),
        PatKind::BoolLit(true) => buf.push_str("true"),
        PatKind::BoolLit(false) => buf.push_str("false"),
        PatKind::Unit => buf.push_str("()"),
        PatKind::Tuple(elems) => {
            buf.push('(');
            for (i, p) in elems.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                pretty_pat(buf, p, interner);
            }
            buf.push(')');
        }
        PatKind::Constructor(sym, None) => buf.push_str(interner.resolve(*sym)),
        PatKind::Constructor(sym, Some(payload)) => {
            buf.push_str(interner.resolve(*sym));
            buf.push(' ');
            pretty_atom_pat(buf, payload, interner);
        }
        PatKind::Cons(hd, tl) => {
            pretty_atom_pat(buf, hd, interner);
            buf.push_str(" :: ");
            pretty_pat(buf, tl, interner);
        }
        PatKind::List(elems) => {
            buf.push('[');
            for (i, p) in elems.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                pretty_pat(buf, p, interner);
            }
            buf.push(']');
        }
        PatKind::Ann(p, ty) => {
            pretty_pat(buf, p, interner);
            buf.push_str(" : ");
            pretty_type(buf, ty, interner);
        }
        PatKind::As(sym, p) => {
            buf.push_str(interner.resolve(*sym));
            buf.push_str(" as ");
            pretty_pat(buf, p, interner);
        }
        PatKind::Paren(p) => {
            buf.push('(');
            pretty_pat(buf, p, interner);
            buf.push(')');
        }
    }
}

fn pretty_atom_pat(buf: &mut String, pat: &Pat, interner: &StringInterner) {
    match &pat.kind {
        PatKind::Constructor(_, Some(_))
        | PatKind::Cons(_, _)
        | PatKind::Ann(_, _)
        | PatKind::As(_, _) => {
            buf.push('(');
            pretty_pat(buf, pat, interner);
            buf.push(')');
        }
        _ => pretty_pat(buf, pat, interner),
    }
}

// ── Type expressions ─────────────────────────────────────────────────

fn pretty_type(buf: &mut String, ty: &TypeExpr, interner: &StringInterner) {
    match &ty.kind {
        TypeExprKind::Named(sym) => buf.push_str(interner.resolve(*sym)),
        TypeExprKind::Var(sym) => buf.push_str(interner.resolve(*sym)),
        TypeExprKind::App(sym, args) => {
            let name = interner.resolve(*sym);
            if args.len() == 1 {
                pretty_atom_type(buf, &args[0], interner);
                write!(buf, " {name}").unwrap();
            } else {
                buf.push('(');
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        buf.push_str(", ");
                    }
                    pretty_type(buf, arg, interner);
                }
                write!(buf, ") {name}").unwrap();
            }
        }
        TypeExprKind::Arrow(lhs, rhs) => {
            pretty_arrow_lhs(buf, lhs, interner);
            buf.push_str(" -> ");
            pretty_type(buf, rhs, interner);
        }
        TypeExprKind::Tuple(elems) => {
            for (i, t) in elems.iter().enumerate() {
                if i > 0 {
                    buf.push_str(" * ");
                }
                pretty_atom_type(buf, t, interner);
            }
        }
        TypeExprKind::Paren(t) => {
            buf.push('(');
            pretty_type(buf, t, interner);
            buf.push(')');
        }
    }
}

fn pretty_atom_type(buf: &mut String, ty: &TypeExpr, interner: &StringInterner) {
    match &ty.kind {
        TypeExprKind::Arrow(_, _) | TypeExprKind::Tuple(_) => {
            buf.push('(');
            pretty_type(buf, ty, interner);
            buf.push(')');
        }
        _ => pretty_type(buf, ty, interner),
    }
}

fn pretty_arrow_lhs(buf: &mut String, ty: &TypeExpr, interner: &StringInterner) {
    match &ty.kind {
        TypeExprKind::Arrow(_, _) => {
            buf.push('(');
            pretty_type(buf, ty, interner);
            buf.push(')');
        }
        _ => pretty_type(buf, ty, interner),
    }
}

// ── Utilities ────────────────────────────────────────────────────────

fn write_indent(buf: &mut String, n: usize) {
    buf.extend(std::iter::repeat_n(' ', n));
}

fn write_escaped(buf: &mut String, c: char) {
    match c {
        '\n' => buf.push_str("\\n"),
        '\t' => buf.push_str("\\t"),
        '\\' => buf.push_str("\\\\"),
        '"' => buf.push_str("\\\""),
        c => buf.push(c),
    }
}

fn write_escaped_string(buf: &mut String, s: &str) {
    buf.push('"');
    for c in s.chars() {
        write_escaped(buf, c);
    }
    buf.push('"');
}

fn write_escaped_char(buf: &mut String, c: char) {
    buf.push_str("#\"");
    write_escaped(buf, c);
    buf.push('"');
}

fn pretty_float(buf: &mut String, f: f64) {
    if f.is_nan() {
        buf.push_str("0.0"); // NaN has no literal form; use 0.0 as placeholder
        return;
    }
    if f.is_infinite() {
        // No literal form for infinity
        if f.is_sign_negative() {
            buf.push_str("~1.0e308");
        } else {
            buf.push_str("1.0e308");
        }
        return;
    }
    let start = buf.len();
    write!(buf, "{f}").unwrap();
    let written = &buf[start..];
    if !written.contains('.') && !written.contains('e') && !written.contains('E') {
        buf.push_str(".0");
    }
}
