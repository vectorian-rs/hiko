use crate::ast::*;
use crate::intern::StringInterner;
use crate::lexer::{LexError, Lexer};
use crate::parser::{ParseError, Parser};
use crate::span::Span;
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone)]
pub enum FormatError {
    Lex(LexError),
    Parse(ParseError),
}

impl From<LexError> for FormatError {
    fn from(error: LexError) -> Self {
        Self::Lex(error)
    }
}

impl From<ParseError> for FormatError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Anchor {
    span: Span,
    output_start: usize,
    output_end: usize,
}

#[derive(Debug)]
struct PrettyOutput {
    text: String,
    anchors: Vec<Anchor>,
}

#[derive(Debug, Clone)]
struct Comment {
    span: Span,
    text: String,
}

#[derive(Debug)]
struct Insertion {
    offset: usize,
    rank: u8,
    text: String,
}

pub fn format_source(source: &str, file_id: u32) -> Result<String, FormatError> {
    let comments = collect_comments(source, file_id);
    let tokens = Lexer::new(source, file_id).tokenize()?;
    let program = Parser::new(tokens).parse_program()?;
    let pretty = TrackedPrettyPrinter::new(&program.interner).pretty_program(&program);
    let mut formatted = if comments.is_empty() {
        pretty.text
    } else {
        insert_comments(&pretty.text, &pretty.anchors, &comments, source)
    };
    if !formatted.is_empty() && !formatted.ends_with('\n') {
        formatted.push('\n');
    }
    Ok(formatted)
}

fn collect_comments(source: &str, file_id: u32) -> Vec<Comment> {
    let mut comments = Vec::new();
    let bytes = source.as_bytes();
    let mut pos = 0usize;

    while pos < bytes.len() {
        if bytes[pos] == b'"' {
            pos = skip_string_literal(source, pos);
            continue;
        }
        if bytes[pos] == b'#' && bytes.get(pos + 1) == Some(&b'"') {
            pos = skip_char_literal(source, pos);
            continue;
        }
        if bytes[pos] == b'(' && bytes.get(pos + 1) == Some(&b'*') {
            let start = pos;
            pos += 2;
            let mut depth = 1u32;
            while pos < bytes.len() && depth > 0 {
                if bytes[pos] == b'(' && bytes.get(pos + 1) == Some(&b'*') {
                    depth += 1;
                    pos += 2;
                } else if bytes[pos] == b'*' && bytes.get(pos + 1) == Some(&b')') {
                    depth -= 1;
                    pos += 2;
                } else {
                    pos += 1;
                }
            }
            comments.push(Comment {
                span: Span::new(file_id, start as u32, pos as u32),
                text: source[start..pos].to_string(),
            });
            continue;
        }
        pos += 1;
    }

    comments
}

fn skip_string_literal(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut pos = start + 1;

    while pos < bytes.len() {
        match bytes[pos] {
            b'"' => return pos + 1,
            b'\\' => {
                pos += 1;
                if pos < bytes.len() {
                    let advance = source[pos..]
                        .chars()
                        .next()
                        .map(|ch| ch.len_utf8())
                        .unwrap_or(1);
                    pos += advance;
                }
            }
            _ => {
                let advance = source[pos..]
                    .chars()
                    .next()
                    .map(|ch| ch.len_utf8())
                    .unwrap_or(1);
                pos += advance;
            }
        }
    }

    bytes.len()
}

fn skip_char_literal(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut pos = start + 2;

    while pos < bytes.len() {
        match bytes[pos] {
            b'"' => return pos + 1,
            b'\\' => {
                pos += 1;
                if pos < bytes.len() {
                    let advance = source[pos..]
                        .chars()
                        .next()
                        .map(|ch| ch.len_utf8())
                        .unwrap_or(1);
                    pos += advance;
                }
            }
            _ => {
                let advance = source[pos..]
                    .chars()
                    .next()
                    .map(|ch| ch.len_utf8())
                    .unwrap_or(1);
                pos += advance;
            }
        }
    }

    bytes.len()
}

fn insert_comments(
    formatted: &str,
    anchors: &[Anchor],
    comments: &[Comment],
    source: &str,
) -> String {
    let mut leading_groups: BTreeMap<usize, Vec<&Comment>> = BTreeMap::new();
    let mut trailing_groups: BTreeMap<usize, Vec<&Comment>> = BTreeMap::new();
    let mut eof_comments = Vec::new();

    for comment in comments {
        let prev = find_prev_anchor(anchors, comment.span.start as usize);
        let next = find_next_anchor(anchors, comment.span.end as usize);
        let can_attach_trailing = prev.is_some_and(|anchor| {
            is_horizontal_trivia_only(
                source,
                anchor.span.end as usize,
                comment.span.start as usize,
                comments,
            )
        });
        let can_attach_leading = next.is_some_and(|anchor| {
            is_trivia_only(
                source,
                comment.span.end as usize,
                anchor.span.start as usize,
                comments,
            )
        });

        if can_attach_trailing {
            let anchor = prev.expect("trailing placement requires previous anchor");
            trailing_groups
                .entry(anchor.output_end)
                .or_default()
                .push(comment);
            continue;
        }

        if can_attach_leading {
            let anchor = next.expect("leading placement requires next anchor");
            leading_groups
                .entry(line_start_offset(formatted, anchor.output_start))
                .or_default()
                .push(comment);
            continue;
        }

        if let Some(anchor) = next {
            leading_groups
                .entry(line_start_offset(formatted, anchor.output_start))
                .or_default()
                .push(comment);
        } else if let Some(anchor) = prev {
            trailing_groups
                .entry(anchor.output_end)
                .or_default()
                .push(comment);
        } else {
            eof_comments.push(comment);
        }
    }

    let mut insertions = Vec::new();

    for (offset, group) in leading_groups {
        let indent = line_indent_at(formatted, offset);
        let mut text = String::new();
        for comment in group {
            text.push_str(&indent_comment(&comment.text, indent));
            text.push('\n');
        }
        insertions.push(Insertion {
            offset,
            rank: 1,
            text,
        });
    }

    for (offset, group) in trailing_groups {
        let line_start = line_start_offset(formatted, offset);
        let indent = line_indent_at(formatted, line_start);
        let mut text = String::new();
        for (idx, comment) in group.into_iter().enumerate() {
            if comment.text.contains('\n') {
                text.push('\n');
                text.push_str(&indent_comment(&comment.text, indent));
            } else if idx == 0 {
                text.push_str("  ");
                text.push_str(&comment.text);
            } else {
                text.push(' ');
                text.push_str(&comment.text);
            }
        }
        insertions.push(Insertion {
            offset,
            rank: 0,
            text,
        });
    }

    if !eof_comments.is_empty() {
        let mut text = String::new();
        if !formatted.is_empty() {
            text.push('\n');
        }
        for (idx, comment) in eof_comments.into_iter().enumerate() {
            if idx > 0 {
                text.push('\n');
            }
            text.push_str(&comment.text);
        }
        insertions.push(Insertion {
            offset: formatted.len(),
            rank: 2,
            text,
        });
    }

    let mut result = formatted.to_string();
    insertions.sort_by(|left, right| {
        right
            .offset
            .cmp(&left.offset)
            .then_with(|| left.rank.cmp(&right.rank))
    });
    for insertion in insertions {
        result.insert_str(insertion.offset, &insertion.text);
    }
    result
}

fn find_prev_anchor(anchors: &[Anchor], pos: usize) -> Option<Anchor> {
    anchors
        .iter()
        .copied()
        .filter(|anchor| anchor.span.end as usize <= pos)
        .max_by(|left, right| {
            left.span
                .end
                .cmp(&right.span.end)
                .then_with(|| left.span.start.cmp(&right.span.start))
        })
}

fn find_next_anchor(anchors: &[Anchor], pos: usize) -> Option<Anchor> {
    anchors
        .iter()
        .copied()
        .filter(|anchor| anchor.span.start as usize >= pos)
        .min_by(|left, right| {
            left.span
                .start
                .cmp(&right.span.start)
                .then_with(|| {
                    let left_len = left.span.end - left.span.start;
                    let right_len = right.span.end - right.span.start;
                    left_len.cmp(&right_len)
                })
                .then_with(|| left.span.end.cmp(&right.span.end))
        })
}

fn is_trivia_only(source: &str, start: usize, end: usize, comments: &[Comment]) -> bool {
    trivia_only_by(source, start, end, comments, |byte| {
        byte.is_ascii_whitespace()
    })
}

fn is_horizontal_trivia_only(source: &str, start: usize, end: usize, comments: &[Comment]) -> bool {
    trivia_only_by(source, start, end, comments, |byte| {
        matches!(byte, b' ' | b'\t' | b'\r')
    })
}

fn trivia_only_by<F>(source: &str, start: usize, end: usize, comments: &[Comment], allow: F) -> bool
where
    F: Fn(u8) -> bool,
{
    let bytes = source.as_bytes();
    let mut pos = start;
    while pos < end {
        if let Some(comment) = comments
            .iter()
            .find(|comment| comment.span.start as usize == pos)
        {
            pos = comment.span.end as usize;
            continue;
        }
        if !allow(bytes[pos]) {
            return false;
        }
        pos += 1;
    }
    true
}

fn line_start_offset(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |idx| idx + 1)
}

fn line_indent_at(text: &str, line_start: usize) -> usize {
    text[line_start..]
        .bytes()
        .take_while(|byte| *byte == b' ')
        .count()
}

fn indent_comment(comment: &str, indent: usize) -> String {
    let prefix = " ".repeat(indent);
    let mut out = String::new();
    for (idx, line) in comment.lines().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(&prefix);
        out.push_str(line);
    }
    out
}

struct TrackedPrettyPrinter<'a> {
    buf: String,
    anchors: Vec<Anchor>,
    interner: &'a StringInterner,
}

impl<'a> TrackedPrettyPrinter<'a> {
    fn new(interner: &'a StringInterner) -> Self {
        Self {
            buf: String::new(),
            anchors: Vec::new(),
            interner,
        }
    }

    fn pretty_program(mut self, prog: &Program) -> PrettyOutput {
        for (idx, decl) in prog.decls.iter().enumerate() {
            if idx > 0 {
                self.buf.push('\n');
            }
            self.pretty_decl(decl, 0);
        }
        PrettyOutput {
            text: self.buf,
            anchors: self.anchors,
        }
    }

    fn with_anchor<F>(&mut self, span: Span, f: F)
    where
        F: FnOnce(&mut Self),
    {
        let start = self.buf.len();
        f(self);
        let end = self.buf.len();
        if span != Span::dummy() && end >= start {
            self.anchors.push(Anchor {
                span,
                output_start: start,
                output_end: end,
            });
        }
    }

    fn pretty_decl(&mut self, decl: &Decl, indent: usize) {
        self.with_anchor(decl.span, |this| match &decl.kind {
            DeclKind::Val(pat, expr) => {
                write_indent(&mut this.buf, indent);
                this.buf.push_str("val ");
                this.pretty_pat(pat);
                this.buf.push_str(" = ");
                this.pretty_expr(expr, indent);
            }
            DeclKind::ValRec(name, expr) => {
                write_indent(&mut this.buf, indent);
                write!(this.buf, "val rec {} = ", this.interner.resolve(*name)).unwrap();
                this.pretty_expr(expr, indent);
            }
            DeclKind::Fun(bindings) => {
                for (idx, binding) in bindings.iter().enumerate() {
                    write_indent(&mut this.buf, indent);
                    if idx == 0 {
                        this.buf.push_str("fun ");
                    } else {
                        this.buf.push_str("and ");
                    }
                    this.pretty_fun_binding(binding, indent);
                }
            }
            DeclKind::Datatype(dt) => {
                write_indent(&mut this.buf, indent);
                this.buf.push_str("datatype ");
                this.pretty_tyvars(&dt.tyvars);
                this.buf.push_str(this.interner.resolve(dt.name));
                this.buf.push_str(" =");
                for (idx, con) in dt.constructors.iter().enumerate() {
                    if idx > 0 {
                        this.buf.push_str(" |");
                    }
                    write!(this.buf, " {}", this.interner.resolve(con.name)).unwrap();
                    if let Some(ty) = &con.payload {
                        this.buf.push_str(" of ");
                        this.pretty_type(ty);
                    }
                }
            }
            DeclKind::TypeAlias(alias) => {
                write_indent(&mut this.buf, indent);
                this.buf.push_str("type ");
                this.pretty_tyvars(&alias.tyvars);
                write!(this.buf, "{} = ", this.interner.resolve(alias.name)).unwrap();
                this.pretty_type(&alias.ty);
            }
            DeclKind::Local(locals, body) => {
                write_indent(&mut this.buf, indent);
                this.buf.push_str("local\n");
                for decl in locals {
                    this.pretty_decl(decl, indent + 2);
                    this.buf.push('\n');
                }
                write_indent(&mut this.buf, indent);
                this.buf.push_str("in\n");
                for decl in body {
                    this.pretty_decl(decl, indent + 2);
                    this.buf.push('\n');
                }
                write_indent(&mut this.buf, indent);
                this.buf.push_str("end");
            }
            DeclKind::Import(name) => {
                write_indent(&mut this.buf, indent);
                write!(this.buf, "import {}", this.interner.resolve(*name)).unwrap();
            }
            DeclKind::Use(path) => {
                write_indent(&mut this.buf, indent);
                this.buf.push_str("use ");
                write_escaped_string(&mut this.buf, path);
            }
            DeclKind::Signature(signature) => {
                write_indent(&mut this.buf, indent);
                writeln!(
                    this.buf,
                    "signature {} = sig",
                    this.interner.resolve(signature.name)
                )
                .unwrap();
                for spec in &signature.specs {
                    write_indent(&mut this.buf, indent + 2);
                    match spec {
                        SignatureSpec::Type { tyvars, name, .. } => {
                            this.buf.push_str("type ");
                            this.pretty_tyvars(tyvars);
                            this.buf.push_str(this.interner.resolve(*name));
                        }
                        SignatureSpec::Val { name, ty, .. } => {
                            write!(this.buf, "val {} : ", this.interner.resolve(*name)).unwrap();
                            this.pretty_type(ty);
                        }
                    }
                    this.buf.push('\n');
                }
                write_indent(&mut this.buf, indent);
                this.buf.push_str("end");
            }
            DeclKind::Structure {
                name,
                signature,
                opaque,
                decls,
            } => {
                write_indent(&mut this.buf, indent);
                write!(this.buf, "structure {}", this.interner.resolve(*name)).unwrap();
                if let Some(signature) = signature {
                    if *opaque {
                        write!(this.buf, " :> {}", this.interner.resolve(*signature)).unwrap();
                    } else {
                        write!(this.buf, " : {}", this.interner.resolve(*signature)).unwrap();
                    }
                }
                this.buf.push_str(" = struct\n");
                for decl in decls {
                    this.pretty_decl(decl, indent + 2);
                    this.buf.push('\n');
                }
                write_indent(&mut this.buf, indent);
                this.buf.push_str("end");
            }
            DeclKind::Effect(name, payload) => {
                write_indent(&mut this.buf, indent);
                write!(this.buf, "effect {}", this.interner.resolve(*name)).unwrap();
                if let Some(ty) = payload {
                    this.buf.push_str(" of ");
                    this.pretty_type(ty);
                }
            }
            DeclKind::AbstractType(dt) => {
                write_indent(&mut this.buf, indent);
                this.buf.push_str("(* abstract type ");
                this.pretty_tyvars(&dt.tyvars);
                this.buf.push_str(this.interner.resolve(dt.name));
                if let Some(implementation) = dt.implementation {
                    write!(this.buf, " = {}", this.interner.resolve(implementation)).unwrap();
                }
                this.buf.push_str(" *)");
            }
            DeclKind::ExportVal {
                public_name,
                internal_name,
                ty,
            } => {
                write_indent(&mut this.buf, indent);
                write!(
                    this.buf,
                    "(* export {} = {} : ",
                    this.interner.resolve(*public_name),
                    this.interner.resolve(*internal_name),
                )
                .unwrap();
                this.pretty_type(ty);
                this.buf.push_str(" *)");
            }
        });
    }

    fn pretty_fun_binding(&mut self, binding: &FunBinding, indent: usize) {
        for (idx, clause) in binding.clauses.iter().enumerate() {
            if idx > 0 {
                self.buf.push('\n');
                write_indent(&mut self.buf, indent + 2);
                self.buf.push_str("| ");
            }
            self.buf.push_str(self.interner.resolve(binding.name));
            for pat in &clause.pats {
                self.buf.push(' ');
                self.pretty_atom_pat(pat);
            }
            self.buf.push_str(" = ");
            self.pretty_expr(&clause.body, indent + 2);
        }
    }

    fn pretty_tyvars(&mut self, tyvars: &[crate::intern::Symbol]) {
        match tyvars.len() {
            0 => {}
            1 => write!(self.buf, "{} ", self.interner.resolve(tyvars[0])).unwrap(),
            _ => {
                self.buf.push('(');
                for (idx, tyvar) in tyvars.iter().enumerate() {
                    if idx > 0 {
                        self.buf.push_str(", ");
                    }
                    self.buf.push_str(self.interner.resolve(*tyvar));
                }
                self.buf.push_str(") ");
            }
        }
    }

    fn pretty_expr(&mut self, expr: &Expr, indent: usize) {
        self.with_anchor(expr.span, |this| match &expr.kind {
            ExprKind::IntLit(n) => write!(this.buf, "{n}").unwrap(),
            ExprKind::FloatLit(f) => pretty_float(&mut this.buf, *f),
            ExprKind::StringLit(s) => write_escaped_string(&mut this.buf, s),
            ExprKind::CharLit(c) => write_escaped_char(&mut this.buf, *c),
            ExprKind::BoolLit(true) => this.buf.push_str("true"),
            ExprKind::BoolLit(false) => this.buf.push_str("false"),
            ExprKind::Unit => this.buf.push_str("()"),
            ExprKind::Var(sym) => this.buf.push_str(this.interner.resolve(*sym)),
            ExprKind::Constructor(sym) => this.buf.push_str(this.interner.resolve(*sym)),
            ExprKind::Tuple(elems) => {
                this.buf.push('(');
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        this.buf.push_str(", ");
                    }
                    this.pretty_expr(elem, indent);
                }
                this.buf.push(')');
            }
            ExprKind::List(elems) => {
                this.buf.push('[');
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        this.buf.push_str(", ");
                    }
                    this.pretty_expr(elem, indent);
                }
                this.buf.push(']');
            }
            ExprKind::Cons(head, tail) => {
                this.pretty_cons_operand(head, indent);
                this.buf.push_str(" :: ");
                this.pretty_expr(tail, indent);
            }
            ExprKind::BinOp(op, lhs, rhs) => {
                let needs_parens_lhs = binop_needs_parens_lhs(op, lhs);
                let needs_parens_rhs = binop_needs_parens_rhs(op, rhs);
                if needs_parens_lhs {
                    this.buf.push('(');
                }
                this.pretty_expr(lhs, indent);
                if needs_parens_lhs {
                    this.buf.push(')');
                }
                write!(this.buf, " {} ", binop_str(op)).unwrap();
                if needs_parens_rhs {
                    this.buf.push('(');
                }
                this.pretty_expr(rhs, indent);
                if needs_parens_rhs {
                    this.buf.push(')');
                }
            }
            ExprKind::UnaryNeg(expr) => {
                this.buf.push('~');
                this.pretty_atom_expr(expr, indent);
            }
            ExprKind::Not(expr) => {
                this.buf.push_str("not ");
                this.pretty_atom_expr(expr, indent);
            }
            ExprKind::App(func, arg) => {
                this.pretty_app_func(func, indent);
                this.buf.push(' ');
                this.pretty_atom_expr(arg, indent);
            }
            ExprKind::Fn(pat, body) => {
                this.buf.push_str("fn ");
                this.pretty_pat(pat);
                this.buf.push_str(" => ");
                this.pretty_expr(body, indent);
            }
            ExprKind::If(cond, then_br, else_br) => {
                this.buf.push_str("if ");
                this.pretty_expr(cond, indent);
                this.buf.push_str(" then ");
                this.pretty_expr(then_br, indent);
                this.buf.push_str(" else ");
                this.pretty_expr(else_br, indent);
            }
            ExprKind::Let(decls, body) => {
                this.buf.push_str("let\n");
                for decl in decls {
                    this.pretty_decl(decl, indent + 2);
                    this.buf.push('\n');
                }
                write_indent(&mut this.buf, indent);
                this.buf.push_str("in\n");
                write_indent(&mut this.buf, indent + 2);
                this.pretty_expr(body, indent + 2);
                this.buf.push('\n');
                write_indent(&mut this.buf, indent);
                this.buf.push_str("end");
            }
            ExprKind::Case(scrutinee, branches) => {
                this.buf.push_str("case ");
                this.pretty_expr(scrutinee, indent);
                this.buf.push_str(" of\n");
                for (idx, (pat, body)) in branches.iter().enumerate() {
                    write_indent(&mut this.buf, indent + 2);
                    if idx > 0 {
                        this.buf.push_str("| ");
                    } else {
                        this.buf.push_str("  ");
                    }
                    this.pretty_pat(pat);
                    this.buf.push_str(" => ");
                    this.pretty_expr(body, indent + 4);
                    if idx + 1 < branches.len() {
                        this.buf.push('\n');
                    }
                }
            }
            ExprKind::Ann(expr, ty) => {
                this.pretty_expr(expr, indent);
                this.buf.push_str(" : ");
                this.pretty_type(ty);
            }
            ExprKind::Paren(expr) => {
                this.buf.push('(');
                this.pretty_expr(expr, indent);
                this.buf.push(')');
            }
            ExprKind::Perform(sym, arg) => {
                write!(this.buf, "perform {} ", this.interner.resolve(*sym)).unwrap();
                this.pretty_atom_expr(arg, indent);
            }
            ExprKind::Handle {
                body,
                return_var,
                return_body,
                handlers,
            } => {
                this.buf.push_str("handle\n");
                write_indent(&mut this.buf, indent + 2);
                this.pretty_expr(body, indent + 2);
                this.buf.push('\n');
                write_indent(&mut this.buf, indent);
                this.buf.push_str("with\n");
                write_indent(&mut this.buf, indent + 2);
                write!(
                    this.buf,
                    "return {} => ",
                    this.interner.resolve(*return_var)
                )
                .unwrap();
                this.pretty_expr(return_body, indent + 2);
                for handler in handlers {
                    this.buf.push('\n');
                    write_indent(&mut this.buf, indent);
                    write!(
                        this.buf,
                        "| {} {} {} => ",
                        this.interner.resolve(handler.effect_name),
                        this.interner.resolve(handler.payload_var),
                        this.interner.resolve(handler.cont_var),
                    )
                    .unwrap();
                    this.pretty_expr(&handler.body, indent + 2);
                }
            }
            ExprKind::Resume(cont, arg) => {
                this.buf.push_str("resume ");
                this.pretty_atom_expr(cont, indent);
                this.buf.push(' ');
                this.pretty_atom_expr(arg, indent);
            }
        });
    }

    fn pretty_atom_expr(&mut self, expr: &Expr, indent: usize) {
        if needs_parens_as_atom(expr) {
            self.buf.push('(');
            self.pretty_expr(expr, indent);
            self.buf.push(')');
        } else {
            self.pretty_expr(expr, indent);
        }
    }

    fn pretty_app_func(&mut self, expr: &Expr, indent: usize) {
        match &expr.kind {
            ExprKind::App(_, _)
            | ExprKind::Var(_)
            | ExprKind::Constructor(_)
            | ExprKind::Paren(_) => self.pretty_expr(expr, indent),
            _ => {
                self.buf.push('(');
                self.pretty_expr(expr, indent);
                self.buf.push(')');
            }
        }
    }

    fn pretty_cons_operand(&mut self, expr: &Expr, indent: usize) {
        match &expr.kind {
            ExprKind::BinOp(BinOp::Orelse | BinOp::Andalso, _, _) | ExprKind::Ann(_, _) => {
                self.buf.push('(');
                self.pretty_expr(expr, indent);
                self.buf.push(')');
            }
            _ => self.pretty_expr(expr, indent),
        }
    }

    fn pretty_pat(&mut self, pat: &Pat) {
        self.with_anchor(pat.span, |this| match &pat.kind {
            PatKind::Wildcard => this.buf.push('_'),
            PatKind::Var(sym) => this.buf.push_str(this.interner.resolve(*sym)),
            PatKind::IntLit(n) => {
                if *n < 0 {
                    write!(this.buf, "~{}", -n).unwrap();
                } else {
                    write!(this.buf, "{n}").unwrap();
                }
            }
            PatKind::FloatLit(f) => {
                if *f < 0.0 {
                    this.buf.push('~');
                    pretty_float(&mut this.buf, -*f);
                } else {
                    pretty_float(&mut this.buf, *f);
                }
            }
            PatKind::StringLit(s) => write_escaped_string(&mut this.buf, s),
            PatKind::CharLit(c) => write_escaped_char(&mut this.buf, *c),
            PatKind::BoolLit(true) => this.buf.push_str("true"),
            PatKind::BoolLit(false) => this.buf.push_str("false"),
            PatKind::Unit => this.buf.push_str("()"),
            PatKind::Tuple(elems) => {
                this.buf.push('(');
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        this.buf.push_str(", ");
                    }
                    this.pretty_pat(elem);
                }
                this.buf.push(')');
            }
            PatKind::Constructor(sym, None) => this.buf.push_str(this.interner.resolve(*sym)),
            PatKind::Constructor(sym, Some(payload)) => {
                this.buf.push_str(this.interner.resolve(*sym));
                this.buf.push(' ');
                this.pretty_atom_pat(payload);
            }
            PatKind::Cons(head, tail) => {
                this.pretty_atom_pat(head);
                this.buf.push_str(" :: ");
                this.pretty_pat(tail);
            }
            PatKind::List(elems) => {
                this.buf.push('[');
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        this.buf.push_str(", ");
                    }
                    this.pretty_pat(elem);
                }
                this.buf.push(']');
            }
            PatKind::Ann(pat, ty) => {
                this.pretty_pat(pat);
                this.buf.push_str(" : ");
                this.pretty_type(ty);
            }
            PatKind::As(sym, pat) => {
                this.buf.push_str(this.interner.resolve(*sym));
                this.buf.push_str(" as ");
                this.pretty_pat(pat);
            }
            PatKind::Paren(pat) => {
                this.buf.push('(');
                this.pretty_pat(pat);
                this.buf.push(')');
            }
        });
    }

    fn pretty_atom_pat(&mut self, pat: &Pat) {
        match &pat.kind {
            PatKind::Constructor(_, Some(_))
            | PatKind::Cons(_, _)
            | PatKind::Ann(_, _)
            | PatKind::As(_, _) => {
                self.buf.push('(');
                self.pretty_pat(pat);
                self.buf.push(')');
            }
            _ => self.pretty_pat(pat),
        }
    }

    fn pretty_type(&mut self, ty: &TypeExpr) {
        self.with_anchor(ty.span, |this| match &ty.kind {
            TypeExprKind::Named(sym) => this.buf.push_str(this.interner.resolve(*sym)),
            TypeExprKind::Var(sym) => this.buf.push_str(this.interner.resolve(*sym)),
            TypeExprKind::App(sym, args) => {
                let name = this.interner.resolve(*sym);
                if args.len() == 1 {
                    this.pretty_atom_type(&args[0]);
                    write!(this.buf, " {name}").unwrap();
                } else {
                    this.buf.push('(');
                    for (idx, arg) in args.iter().enumerate() {
                        if idx > 0 {
                            this.buf.push_str(", ");
                        }
                        this.pretty_type(arg);
                    }
                    write!(this.buf, ") {name}").unwrap();
                }
            }
            TypeExprKind::Arrow(lhs, rhs) => {
                this.pretty_arrow_lhs(lhs);
                this.buf.push_str(" -> ");
                this.pretty_type(rhs);
            }
            TypeExprKind::Tuple(elems) => {
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        this.buf.push_str(" * ");
                    }
                    this.pretty_atom_type(elem);
                }
            }
            TypeExprKind::Paren(ty) => {
                this.buf.push('(');
                this.pretty_type(ty);
                this.buf.push(')');
            }
        });
    }

    fn pretty_atom_type(&mut self, ty: &TypeExpr) {
        match &ty.kind {
            TypeExprKind::Arrow(_, _) | TypeExprKind::Tuple(_) => {
                self.buf.push('(');
                self.pretty_type(ty);
                self.buf.push(')');
            }
            _ => self.pretty_type(ty),
        }
    }

    fn pretty_arrow_lhs(&mut self, ty: &TypeExpr) {
        match &ty.kind {
            TypeExprKind::Arrow(_, _) => {
                self.buf.push('(');
                self.pretty_type(ty);
                self.buf.push(')');
            }
            _ => self.pretty_type(ty),
        }
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

fn binop_prec(op: &BinOp) -> u8 {
    match op {
        BinOp::Pipe => 0,
        BinOp::Orelse => 1,
        BinOp::Andalso => 2,
        BinOp::Eq
        | BinOp::Ne
        | BinOp::LtInt
        | BinOp::GtInt
        | BinOp::LeInt
        | BinOp::GeInt
        | BinOp::LtFloat
        | BinOp::GtFloat
        | BinOp::LeFloat
        | BinOp::GeFloat => 3,
        BinOp::AddInt | BinOp::SubInt | BinOp::AddFloat | BinOp::SubFloat | BinOp::ConcatStr => 4,
        BinOp::MulInt | BinOp::DivInt | BinOp::ModInt | BinOp::MulFloat | BinOp::DivFloat => 5,
    }
}

fn is_right_assoc(op: &BinOp) -> bool {
    matches!(op, BinOp::Orelse | BinOp::Andalso)
}

fn binop_needs_parens_lhs(op: &BinOp, lhs: &Expr) -> bool {
    if let ExprKind::BinOp(lhs_op, _, _) = &lhs.kind {
        let lhs_prec = binop_prec(lhs_op);
        let rhs_prec = binop_prec(op);
        if is_right_assoc(op) {
            lhs_prec <= rhs_prec
        } else {
            lhs_prec < rhs_prec
        }
    } else {
        false
    }
}

fn binop_needs_parens_rhs(op: &BinOp, rhs: &Expr) -> bool {
    if let ExprKind::BinOp(rhs_op, _, _) = &rhs.kind {
        let lhs_prec = binop_prec(rhs_op);
        let rhs_prec = binop_prec(op);
        if is_right_assoc(op) {
            lhs_prec < rhs_prec
        } else {
            lhs_prec <= rhs_prec
        }
    } else {
        false
    }
}

fn binop_str(op: &BinOp) -> &'static str {
    match op {
        BinOp::Pipe => "|>",
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

fn write_indent(buf: &mut String, indent: usize) {
    buf.extend(std::iter::repeat_n(' ', indent));
}

fn write_escaped(buf: &mut String, ch: char) {
    match ch {
        '\n' => buf.push_str("\\n"),
        '\t' => buf.push_str("\\t"),
        '\\' => buf.push_str("\\\\"),
        '"' => buf.push_str("\\\""),
        ch => buf.push(ch),
    }
}

fn write_escaped_string(buf: &mut String, s: &str) {
    buf.push('"');
    for ch in s.chars() {
        write_escaped(buf, ch);
    }
    buf.push('"');
}

fn write_escaped_char(buf: &mut String, ch: char) {
    buf.push_str("#\"");
    write_escaped(buf, ch);
    buf.push('"');
}

fn pretty_float(buf: &mut String, value: f64) {
    if value.is_nan() {
        buf.push_str("0.0");
        return;
    }
    if value.is_infinite() {
        if value.is_sign_negative() {
            buf.push_str("~1.0e308");
        } else {
            buf.push_str("1.0e308");
        }
        return;
    }
    let start = buf.len();
    write!(buf, "{value}").unwrap();
    let written = &buf[start..];
    if !written.contains('.') && !written.contains('e') && !written.contains('E') {
        buf.push_str(".0");
    }
}

#[cfg(test)]
mod tests {
    use super::format_source;

    fn fmt(source: &str) -> String {
        format_source(source, 0).expect("formatting should succeed")
    }

    #[test]
    fn preserves_header_and_trailing_comments() {
        let source = "(* header *)\nval _=println \"hi\" (* tail *)\n";
        assert_eq!(
            fmt(source),
            "(* header *)\nval _ = println \"hi\"  (* tail *)\n"
        );
    }

    #[test]
    fn keeps_comment_markers_inside_strings() {
        let source = "val s = \"(* not a comment *)\"\n(* real comment *)\nval _ = println s\n";
        assert_eq!(
            fmt(source),
            "val s = \"(* not a comment *)\"\n(* real comment *)\nval _ = println s\n"
        );
    }

    #[test]
    fn is_idempotent_with_nested_comments() {
        let source = "fun f x =\n  case x of\n      [] => 0\n    | y :: ys => (* branch *) y\n";
        let once = fmt(source);
        let twice = fmt(&once);
        assert_eq!(twice, once);
    }

    #[test]
    fn formats_comment_only_files() {
        let source = "(* hello *)\n(* world *)\n";
        assert_eq!(fmt(source), "(* hello *)\n(* world *)\n");
    }
}
