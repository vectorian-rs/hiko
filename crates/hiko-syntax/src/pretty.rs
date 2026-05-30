use crate::ast::*;
use crate::intern::StringInterner;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub text: String,
    pub start: u32,
    pub end: u32,
    pub start_line: usize,
    pub end_line: usize,
}

struct CommentCtx<'a> {
    comments: &'a [Comment],
    line_offsets: &'a [usize],
}

pub fn pretty_program(prog: &Program) -> String {
    let interner = &prog.interner;
    let mut buf = String::new();
    pretty_decl_list(&mut buf, &prog.decls, 0, interner, None, 0, u32::MAX);
    buf
}

pub fn pretty_program_with_comments(
    prog: &Program,
    comments: &[Comment],
    line_offsets: &[usize],
) -> String {
    let interner = &prog.interner;
    let ctx = CommentCtx {
        comments,
        line_offsets,
    };
    let mut buf = String::new();
    pretty_decl_list(&mut buf, &prog.decls, 0, interner, Some(&ctx), 0, u32::MAX);
    buf
}

// ── Declarations ─────────────────────────────────────────────────────

fn pretty_decl_list(
    buf: &mut String,
    decls: &[Decl],
    indent: usize,
    interner: &StringInterner,
    ctx: Option<&CommentCtx<'_>>,
    container_start: u32,
    container_end: u32,
) {
    let mut cursor = container_start;
    for (i, decl) in decls.iter().enumerate() {
        let leading = ctx
            .map(|ctx| {
                leading_comments(ctx, cursor, decl.span.start, container_end).collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if i > 0 {
            if leading.is_empty() {
                if is_import_decl(&decls[i - 1]) && is_import_decl(decl) {
                    buf.push('\n');
                } else {
                    ensure_blank_line(buf);
                }
            } else if !buf.ends_with('\n') {
                buf.push('\n');
            }
        }

        if let Some(ctx) = ctx {
            if let Some(first) = leading.first() {
                if i > 0 && first.start_line > line_for(ctx, cursor) + 1 {
                    ensure_blank_line(buf);
                }
            }
        }
        write_comment_sequence(buf, &leading, indent);

        pretty_decl(buf, decl, indent, interner, ctx);

        let mut trailing_end = decl.span.end;
        if let Some(ctx) = ctx {
            for comment in
                trailing_comments(ctx, decl.span.end, next_decl_start(decls, i), container_end)
            {
                buf.push_str("  ");
                buf.push_str(comment.text.trim());
                trailing_end = trailing_end.max(comment.end);
            }
        }
        cursor = trailing_end;
    }

    if let Some(ctx) = ctx {
        let trailing =
            leading_comments(ctx, cursor, container_end, container_end).collect::<Vec<_>>();
        if let Some(first) = trailing.first() {
            if !buf.is_empty() && first.start_line > line_for(ctx, cursor) + 1 {
                ensure_blank_line(buf);
            }
        }
        write_comment_sequence(buf, &trailing, indent);
        trim_trailing_newlines(buf);
    }
}

fn write_comment_sequence(buf: &mut String, comments: &[&Comment], indent: usize) {
    let mut prev_end_line = None;
    for comment in comments {
        if let Some(prev_end_line) = prev_end_line {
            if comment.start_line > prev_end_line + 1 {
                ensure_blank_line(buf);
            } else if !buf.is_empty() && !buf.ends_with('\n') {
                buf.push('\n');
            }
        }
        write_indent(buf, indent);
        buf.push_str(comment.text.trim());
        buf.push('\n');
        prev_end_line = Some(comment.end_line);
    }
}

fn next_decl_start(decls: &[Decl], idx: usize) -> u32 {
    decls
        .get(idx + 1)
        .map(|decl| decl.span.start)
        .unwrap_or(u32::MAX)
}

fn leading_comments<'a>(
    ctx: &'a CommentCtx<'_>,
    after: u32,
    before: u32,
    container_end: u32,
) -> impl Iterator<Item = &'a Comment> {
    ctx.comments.iter().filter(move |comment| {
        comment.start >= after && comment.end <= before && comment.end <= container_end
    })
}

fn trailing_comments<'a>(
    ctx: &'a CommentCtx<'_>,
    decl_end: u32,
    before: u32,
    container_end: u32,
) -> impl Iterator<Item = &'a Comment> {
    let decl_line = line_for(ctx, decl_end);
    ctx.comments.iter().filter(move |comment| {
        comment.start >= decl_end
            && comment.start < before
            && comment.end <= container_end
            && comment.start_line == decl_line
    })
}

fn line_for(ctx: &CommentCtx<'_>, byte: u32) -> usize {
    let byte = byte as usize;
    match ctx.line_offsets.binary_search(&byte) {
        Ok(line) => line,
        Err(0) => 0,
        Err(line) => line - 1,
    }
}

fn ensure_blank_line(buf: &mut String) {
    while buf.ends_with(' ') || buf.ends_with('\t') {
        buf.pop();
    }
    if buf.is_empty() || buf.ends_with("\n\n") {
        return;
    }
    if !buf.ends_with('\n') {
        buf.push('\n');
    }
    buf.push('\n');
}

fn trim_trailing_newlines(buf: &mut String) {
    while buf.ends_with('\n') {
        buf.pop();
    }
}

fn is_import_decl(decl: &Decl) -> bool {
    matches!(
        decl.kind,
        DeclKind::Import(_) | DeclKind::ImportWithNames(_, _)
    )
}

fn pretty_decl(
    buf: &mut String,
    decl: &Decl,
    indent: usize,
    interner: &StringInterner,
    ctx: Option<&CommentCtx<'_>>,
) {
    match &decl.kind {
        DeclKind::Val(pat, expr) => {
            write_indent(buf, indent);
            buf.push_str("val ");
            pretty_pat(buf, pat, interner);
            if is_multiline_expr(expr) {
                buf.push_str(" =\n");
                write_indent(buf, indent + 2);
                pretty_expr(buf, expr, indent + 2, interner);
            } else {
                buf.push_str(" = ");
                pretty_expr(buf, expr, indent, interner);
            }
        }
        DeclKind::ValRec(name, expr) => {
            write_indent(buf, indent);
            write!(buf, "val rec {} = ", interner.resolve(*name)).unwrap();
            pretty_expr(buf, expr, indent, interner);
        }
        DeclKind::Fun(bindings) => {
            for (i, binding) in bindings.iter().enumerate() {
                if i > 0 {
                    buf.push('\n');
                }
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
            if dt.constructors.len() == 1 {
                buf.push_str(" = ");
                pretty_constructor(buf, &dt.constructors[0], interner);
            } else {
                buf.push_str(" =\n");
                for (i, con) in dt.constructors.iter().enumerate() {
                    if i == 0 {
                        write_indent(buf, indent + 4);
                    } else {
                        write_indent(buf, indent + 2);
                        buf.push_str("| ");
                    }
                    pretty_constructor(buf, con, interner);
                    if i + 1 < dt.constructors.len() {
                        buf.push('\n');
                    }
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
            pretty_decl_list(
                buf,
                locals,
                indent + 2,
                interner,
                ctx,
                decl.span.start,
                decl.span.end,
            );
            if !locals.is_empty() {
                buf.push('\n');
            }
            write_indent(buf, indent);
            buf.push_str("in\n");
            pretty_decl_list(
                buf,
                body,
                indent + 2,
                interner,
                ctx,
                decl.span.start,
                decl.span.end,
            );
            if !body.is_empty() {
                buf.push('\n');
            }
            write_indent(buf, indent);
            buf.push_str("end");
        }
        DeclKind::Import(name) => {
            write_indent(buf, indent);
            write!(buf, "import {}", interner.resolve(*name)).unwrap();
        }
        DeclKind::ImportWithNames(name, names) => {
            write_indent(buf, indent);
            write!(buf, "import {} (", interner.resolve(*name)).unwrap();
            for (i, sym) in names.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                buf.push_str(interner.resolve(*sym));
            }
            buf.push_str(")");
        }
        DeclKind::Use(path) => {
            write_indent(buf, indent);
            buf.push_str("use ");
            write_escaped_string(buf, path);
        }
        DeclKind::Signature(sig) => {
            write_indent(buf, indent);
            writeln!(buf, "signature {} = sig", interner.resolve(sig.name)).unwrap();
            for spec in &sig.specs {
                write_indent(buf, indent + 2);
                match spec {
                    SignatureSpec::Type { tyvars, name, .. } => {
                        buf.push_str("type ");
                        pretty_tyvars(buf, tyvars, interner);
                        buf.push_str(interner.resolve(*name));
                    }
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
            pretty_decl_list(
                buf,
                decls,
                indent + 2,
                interner,
                ctx,
                decl.span.start,
                decl.span.end,
            );
            if !decls.is_empty() {
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
        DeclKind::AbstractType(dt) => {
            write_indent(buf, indent);
            buf.push_str("(* abstract type ");
            pretty_tyvars(buf, &dt.tyvars, interner);
            buf.push_str(interner.resolve(dt.name));
            if let Some(implementation) = dt.implementation {
                write!(buf, " = {}", interner.resolve(implementation)).unwrap();
            }
            buf.push_str(" *)");
        }
        DeclKind::ExportVal {
            public_name,
            internal_name,
            ty,
        } => {
            write_indent(buf, indent);
            write!(
                buf,
                "(* export {} = {} : ",
                interner.resolve(*public_name),
                interner.resolve(*internal_name)
            )
            .unwrap();
            pretty_type(buf, ty, interner);
            buf.push_str(" *)");
        }
    }
}

fn pretty_constructor(buf: &mut String, con: &ConDecl, interner: &StringInterner) {
    buf.push_str(interner.resolve(con.name));
    if let Some(ref ty) = con.payload {
        buf.push_str(" of ");
        pretty_type(buf, ty, interner);
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
        if is_multiline_expr(&clause.body) {
            buf.push_str(" =\n");
            write_indent(buf, indent + 2);
            pretty_expr(buf, &clause.body, indent + 2, interner);
        } else {
            buf.push_str(" = ");
            pretty_expr(buf, &clause.body, indent + 2, interner);
        }
    }
}

fn is_multiline_expr(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Case(_, _) | ExprKind::Let(_, _) | ExprKind::Handle { .. }
    )
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
        ExprKind::WordLit(w) => write!(buf, "0w{w}").unwrap(),
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
                pretty_decl(buf, d, indent + 2, interner, None);
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
                if i == 0 {
                    write_indent(buf, indent + 4);
                } else {
                    write_indent(buf, indent + 2);
                    buf.push_str("| ");
                }
                pretty_pat(buf, pat, interner);
                if is_multiline_expr(body) {
                    buf.push_str(" =>\n");
                    write_indent(buf, indent + 4);
                    pretty_expr(buf, body, indent + 4, interner);
                } else {
                    buf.push_str(" => ");
                    pretty_expr(buf, body, indent + 4, interner);
                }
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
            | ExprKind::WordLit(_)
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
        BinOp::Pipe => 0,
        BinOp::Orelse => 1,
        BinOp::Andalso => 2,
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Gt
        | BinOp::Le
        | BinOp::Ge
        | BinOp::LtInt
        | BinOp::GtInt
        | BinOp::LeInt
        | BinOp::GeInt
        | BinOp::LtWord
        | BinOp::GtWord
        | BinOp::LeWord
        | BinOp::GeWord => 3,
        BinOp::Add
        | BinOp::Sub
        | BinOp::AddInt
        | BinOp::SubInt
        | BinOp::AddWord
        | BinOp::SubWord
        | BinOp::ConcatStr => 4,
        BinOp::Mul
        | BinOp::Div
        | BinOp::Mod
        | BinOp::MulInt
        | BinOp::DivInt
        | BinOp::ModInt
        | BinOp::MulWord
        | BinOp::DivWord
        | BinOp::ModWord => 5,
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
        BinOp::Pipe => "|>",
        BinOp::Add | BinOp::AddInt | BinOp::AddWord => "+",
        BinOp::Sub | BinOp::SubInt | BinOp::SubWord => "-",
        BinOp::Mul | BinOp::MulInt | BinOp::MulWord => "*",
        BinOp::Div | BinOp::DivInt | BinOp::DivWord => "/",
        BinOp::Mod | BinOp::ModInt | BinOp::ModWord => "mod",
        BinOp::ConcatStr => "^",
        BinOp::Lt | BinOp::LtInt | BinOp::LtWord => "<",
        BinOp::Gt | BinOp::GtInt | BinOp::GtWord => ">",
        BinOp::Le | BinOp::LeInt | BinOp::LeWord => "<=",
        BinOp::Ge | BinOp::GeInt | BinOp::GeWord => ">=",
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
        PatKind::WordLit(w) => write!(buf, "0w{w}").unwrap(),
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
