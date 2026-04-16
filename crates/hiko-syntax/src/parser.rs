use crate::ast::*;
use crate::intern::{StringInterner, Symbol};
use crate::span::Span;
use crate::token::{Token, TokenKind};

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: u32,
    interner: StringInterner,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
            interner: StringInterner::new(),
        }
    }

    pub fn with_interner(tokens: Vec<Token>, interner: StringInterner) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
            interner,
        }
    }

    // ── Utility ──────────────────────────────────────────────────────

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn advance(&mut self) {
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, kind: &TokenKind) -> Option<Span> {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(kind) {
            let span = self.span();
            self.advance();
            Some(span)
        } else {
            None
        }
    }

    fn expect(&mut self, kind: &TokenKind, desc: &str) -> Result<Span, ParseError> {
        self.eat(kind)
            .ok_or_else(|| self.err(&format!("expected {desc}")))
    }

    /// Take an identifier/tyvar payload from the current token, intern it, and advance.
    fn take_symbol(&mut self) -> Symbol {
        let s = match &mut self.tokens[self.pos].kind {
            TokenKind::Ident(s) | TokenKind::UpperIdent(s) | TokenKind::TyVar(s) => {
                std::mem::take(s)
            }
            _ => String::new(),
        };
        self.advance();
        self.interner.intern(&s)
    }

    /// Take a string literal payload from the current token and advance.
    fn take_string_lit(&mut self) -> String {
        let s = match &mut self.tokens[self.pos].kind {
            TokenKind::StringLit(s) => std::mem::take(s),
            _ => String::new(),
        };
        self.advance();
        s
    }

    fn expect_ident(&mut self) -> Result<(Symbol, Span), ParseError> {
        if matches!(self.peek(), TokenKind::Ident(_)) {
            let span = self.span();
            Ok((self.take_symbol(), span))
        } else {
            Err(self.err("expected identifier"))
        }
    }

    fn expect_ident_or_wildcard(&mut self) -> Result<(Symbol, Span), ParseError> {
        if matches!(self.peek(), TokenKind::Ident(_)) {
            let span = self.span();
            Ok((self.take_symbol(), span))
        } else if matches!(self.peek(), TokenKind::Underscore) {
            let span = self.span();
            self.advance();
            Ok((self.interner.intern("_"), span))
        } else {
            Err(self.err("expected identifier or _"))
        }
    }

    fn expect_upper_ident(&mut self) -> Result<(Symbol, Span), ParseError> {
        if matches!(self.peek(), TokenKind::UpperIdent(_)) {
            let span = self.span();
            Ok((self.take_symbol(), span))
        } else {
            Err(self.err("expected constructor name"))
        }
    }

    fn expect_name(&mut self) -> Result<(Symbol, Span), ParseError> {
        if matches!(self.peek(), TokenKind::Ident(_) | TokenKind::UpperIdent(_)) {
            let span = self.span();
            Ok((self.take_symbol(), span))
        } else {
            Err(self.err("expected name"))
        }
    }

    fn expect_tyvar(&mut self) -> Result<Symbol, ParseError> {
        if matches!(self.peek(), TokenKind::TyVar(_)) {
            Ok(self.take_symbol())
        } else {
            Err(self.err("expected type variable"))
        }
    }

    fn err(&self, message: &str) -> ParseError {
        ParseError {
            message: message.to_string(),
            span: self.span(),
        }
    }

    fn guard_depth(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > 256 {
            Err(self.err("expression nesting limit exceeded"))
        } else {
            Ok(())
        }
    }

    fn unguard_depth(&mut self) {
        self.depth -= 1;
    }

    fn can_start_app_arg(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::IntLit(_)
                | TokenKind::FloatLit(_)
                | TokenKind::StringLit(_)
                | TokenKind::CharLit(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Ident(_)
                | TokenKind::UpperIdent(_)
                | TokenKind::Tilde
                | TokenKind::Not
                | TokenKind::LParen
                | TokenKind::LBracket
        )
    }

    fn can_start_qualified_path(&self) -> bool {
        matches!(self.peek(), TokenKind::UpperIdent(_))
            && self.pos + 1 < self.tokens.len()
            && matches!(self.tokens[self.pos + 1].kind, TokenKind::Dot)
    }

    fn parse_qualified_symbol(&mut self) -> Result<(Symbol, bool, Span), ParseError> {
        let start = self.span();
        let mut parts = Vec::new();

        if !matches!(self.peek(), TokenKind::UpperIdent(_)) {
            return Err(self.err("expected qualified name"));
        }
        parts.push(self.take_symbol());

        while self.eat(&TokenKind::Dot).is_some() {
            match self.peek() {
                TokenKind::UpperIdent(_) | TokenKind::Ident(_) => parts.push(self.take_symbol()),
                _ => return Err(self.err("expected name after '.'")),
            }
        }

        let end = self.tokens[self.pos - 1].span;
        let last_name = self.interner.resolve(*parts.last().unwrap()).to_string();
        let is_constructor = last_name
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_uppercase());
        let joined = parts
            .iter()
            .map(|sym| self.interner.resolve(*sym))
            .collect::<Vec<_>>()
            .join(".");
        let sym = self.interner.intern(&joined);
        Ok((sym, is_constructor, start.merge(end)))
    }

    fn can_start_atom_pat(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Underscore
                | TokenKind::IntLit(_)
                | TokenKind::FloatLit(_)
                | TokenKind::StringLit(_)
                | TokenKind::CharLit(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Ident(_)
                | TokenKind::UpperIdent(_)
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::Tilde
        )
    }

    // ── Program ──────────────────────────────────────────────────────

    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut decls = Vec::new();
        while !matches!(self.peek(), TokenKind::Eof) {
            decls.push(self.parse_decl()?);
        }
        Ok(Program {
            decls,
            interner: std::mem::take(&mut self.interner),
        })
    }

    // ── Declarations ─────────────────────────────────────────────────

    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        match self.peek() {
            TokenKind::Val => self.parse_val_decl(),
            TokenKind::Fun => self.parse_fun_decl(),
            TokenKind::Datatype => self.parse_datatype_decl(),
            TokenKind::Type => self.parse_type_alias_decl(),
            TokenKind::Local => self.parse_local_decl(),
            TokenKind::Use => self.parse_use_decl(),
            TokenKind::Signature => self.parse_signature_decl(),
            TokenKind::Structure => self.parse_structure_decl(),
            TokenKind::Effect => self.parse_effect_decl(),
            _ => {
                Err(self
                    .err("expected declaration (val, fun, datatype, type, local, use, signature, structure, or effect)"))
            }
        }
    }

    fn parse_val_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `val`

        if self.eat(&TokenKind::Rec).is_some() {
            let (name, _) = self.expect_ident()?;
            self.expect(&TokenKind::Eq, "=")?;
            if !matches!(self.peek(), TokenKind::Fn) {
                return Err(self.err("val rec requires fn on the right-hand side"));
            }
            let body = self.parse_expr()?;
            let span = start.merge(body.span);
            return Ok(Decl {
                kind: DeclKind::ValRec(name, body),
                span,
            });
        }

        let pat = self.parse_pat()?;
        self.expect(&TokenKind::Eq, "=")?;
        let body = self.parse_expr()?;
        let span = start.merge(body.span);
        Ok(Decl {
            kind: DeclKind::Val(pat, body),
            span,
        })
    }

    fn parse_fun_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `fun`

        let mut bindings = Vec::new();
        loop {
            bindings.push(self.parse_fun_binding()?);
            if self.eat(&TokenKind::And).is_none() {
                break;
            }
        }
        let span = start.merge(bindings.last().unwrap().span);
        Ok(Decl {
            kind: DeclKind::Fun(bindings),
            span,
        })
    }

    fn parse_fun_binding(&mut self) -> Result<FunBinding, ParseError> {
        let (name, name_span) = self.expect_ident()?;
        let mut clauses = Vec::new();

        loop {
            let clause_start = self.span();
            let mut pats = Vec::new();
            while self.can_start_atom_pat() {
                pats.push(self.parse_atom_pat()?);
            }
            if pats.is_empty() {
                return Err(self.err("expected at least one parameter pattern"));
            }
            self.expect(&TokenKind::Eq, "=")?;
            let body = self.parse_expr()?;
            let clause_span = clause_start.merge(body.span);
            clauses.push(FunClause {
                pats,
                body,
                span: clause_span,
            });

            // Check for another clause: | name ...
            if matches!(self.peek(), TokenKind::Bar) {
                self.advance(); // consume |
                // Expect the same function name
                let (next_name, _) = self.expect_ident()?;
                if next_name != name {
                    return Err(self.err(&format!(
                        "clausal function name mismatch: expected '{}', found '{}'",
                        self.interner.resolve(name),
                        self.interner.resolve(next_name),
                    )));
                }
            } else {
                break;
            }
        }

        let span = name_span.merge(clauses.last().unwrap().span);
        Ok(FunBinding {
            name,
            clauses,
            span,
        })
    }

    fn parse_datatype_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `datatype`

        let tyvars = self.parse_tyvar_params()?;
        let (name, _) = self.expect_name()?;
        self.expect(&TokenKind::Eq, "=")?;

        self.eat(&TokenKind::Bar); // optional leading |
        let mut constructors = Vec::new();
        loop {
            let con_start = self.span();
            let (con_name, _) = self.expect_upper_ident()?;
            let payload = if self.eat(&TokenKind::Of).is_some() {
                Some(self.parse_type_expr()?)
            } else {
                None
            };
            let con_span = if let Some(ref ty) = payload {
                con_start.merge(ty.span)
            } else {
                con_start
            };
            constructors.push(ConDecl {
                name: con_name,
                payload,
                span: con_span,
            });
            if self.eat(&TokenKind::Bar).is_none() {
                break;
            }
        }

        let span = start.merge(constructors.last().unwrap().span);
        Ok(Decl {
            kind: DeclKind::Datatype(DatatypeDecl {
                tyvars,
                name,
                constructors,
                span,
            }),
            span,
        })
    }

    fn parse_type_alias_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `type`
        let tyvars = self.parse_tyvar_params()?;
        let (name, _) = self.expect_name()?;
        self.expect(&TokenKind::Eq, "=")?;
        let ty = self.parse_type_expr()?;
        let span = start.merge(ty.span);
        Ok(Decl {
            kind: DeclKind::TypeAlias(TypeAliasDecl {
                tyvars,
                name,
                ty,
                span,
            }),
            span,
        })
    }

    fn parse_local_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `local`
        let mut locals = Vec::new();
        while !matches!(self.peek(), TokenKind::In | TokenKind::Eof) {
            locals.push(self.parse_decl()?);
        }
        self.expect(&TokenKind::In, "in")?;
        let mut body = Vec::new();
        while !matches!(self.peek(), TokenKind::End | TokenKind::Eof) {
            body.push(self.parse_decl()?);
        }
        let end_span = self.expect(&TokenKind::End, "end")?;
        let span = start.merge(end_span);
        Ok(Decl {
            kind: DeclKind::Local(locals, body),
            span,
        })
    }

    fn parse_use_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `use`
        if matches!(self.peek(), TokenKind::StringLit(_)) {
            let end = self.span();
            let path = self.take_string_lit();
            let span = start.merge(end);
            Ok(Decl {
                kind: DeclKind::Use(path),
                span,
            })
        } else {
            Err(self.err("expected string literal after 'use'"))
        }
    }

    fn parse_signature_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `signature`
        let (name, _) = self.expect_upper_ident()?;
        self.expect(&TokenKind::Eq, "=")?;
        self.expect(&TokenKind::Sig, "sig")?;
        let mut specs = Vec::new();
        while !matches!(self.peek(), TokenKind::End | TokenKind::Eof) {
            specs.push(self.parse_signature_spec()?);
        }
        let end = self.expect(&TokenKind::End, "end")?;
        Ok(Decl {
            kind: DeclKind::Signature(SignatureDecl {
                name,
                specs,
                span: start.merge(end),
            }),
            span: start.merge(end),
        })
    }

    fn parse_signature_spec(&mut self) -> Result<SignatureSpec, ParseError> {
        let start = self.span();
        match self.peek() {
            TokenKind::Val => {
                self.advance();
                let (name, _) = self.expect_ident()?;
                self.expect(&TokenKind::Colon, ":")?;
                let ty = self.parse_type_expr()?;
                Ok(SignatureSpec::Val {
                    name,
                    span: start.merge(ty.span),
                    ty,
                })
            }
            _ => Err(self.err("expected signature spec (currently only val)")),
        }
    }

    fn parse_structure_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `structure`
        let (name, _) = self.expect_upper_ident()?;
        let (signature, opaque) = if self.eat(&TokenKind::ColonGt).is_some() {
            (Some(self.expect_upper_ident()?.0), true)
        } else if self.eat(&TokenKind::Colon).is_some() {
            (Some(self.expect_upper_ident()?.0), false)
        } else {
            (None, false)
        };
        self.expect(&TokenKind::Eq, "=")?;
        self.expect(&TokenKind::Struct, "struct")?;
        let mut decls = Vec::new();
        while !matches!(self.peek(), TokenKind::End | TokenKind::Eof) {
            decls.push(self.parse_decl()?);
        }
        let end = self.expect(&TokenKind::End, "end")?;
        Ok(Decl {
            kind: DeclKind::Structure {
                name,
                signature,
                opaque,
                decls,
            },
            span: start.merge(end),
        })
    }

    fn parse_effect_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `effect`
        let (name, end) = self.expect_upper_ident()?;
        let payload = if self.eat(&TokenKind::Of).is_some() {
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let span = start.merge(payload.as_ref().map_or(end, |t| t.span));
        Ok(Decl {
            kind: DeclKind::Effect(name, payload),
            span,
        })
    }

    fn parse_tyvar_params(&mut self) -> Result<Vec<Symbol>, ParseError> {
        if matches!(self.peek(), TokenKind::TyVar(_)) {
            return Ok(vec![self.take_symbol()]);
        }
        if self.eat(&TokenKind::LParen).is_none() {
            return Ok(vec![]);
        }
        let mut vars = vec![self.expect_tyvar()?];
        while self.eat(&TokenKind::Comma).is_some() {
            vars.push(self.expect_tyvar()?);
        }
        self.expect(&TokenKind::RParen, ")")?;
        Ok(vars)
    }

    // ── Expressions (lowest to highest precedence) ───────────────────

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.guard_depth()?;
        let result = self.parse_expr_inner();
        self.unguard_depth();
        result
    }

    fn parse_expr_inner(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_orelse()?;
        if self.eat(&TokenKind::Colon).is_some() {
            let ty = self.parse_type_expr()?;
            let span = expr.span.merge(ty.span);
            Ok(Expr {
                kind: ExprKind::Ann(Box::new(expr), ty),
                span,
            })
        } else {
            Ok(expr)
        }
    }

    fn parse_orelse(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_andalso()?;
        if self.eat(&TokenKind::Orelse).is_some() {
            let rhs = self.parse_orelse()?; // right-associative
            let span = lhs.span.merge(rhs.span);
            Ok(Expr {
                kind: ExprKind::BinOp(BinOp::Orelse, Box::new(lhs), Box::new(rhs)),
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_andalso(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_comp()?;
        if self.eat(&TokenKind::Andalso).is_some() {
            let rhs = self.parse_andalso()?; // right-associative
            let span = lhs.span.merge(rhs.span);
            Ok(Expr {
                kind: ExprKind::BinOp(BinOp::Andalso, Box::new(lhs), Box::new(rhs)),
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_comp(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_cons()?;
        let op = match self.peek() {
            TokenKind::Eq => Some(BinOp::Eq),
            TokenKind::Ne => Some(BinOp::Ne),
            TokenKind::Lt => Some(BinOp::LtInt),
            TokenKind::Gt => Some(BinOp::GtInt),
            TokenKind::Le => Some(BinOp::LeInt),
            TokenKind::Ge => Some(BinOp::GeInt),
            TokenKind::LtDot => Some(BinOp::LtFloat),
            TokenKind::GtDot => Some(BinOp::GtFloat),
            TokenKind::LeDot => Some(BinOp::LeFloat),
            TokenKind::GeDot => Some(BinOp::GeFloat),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let rhs = self.parse_cons()?;
            let span = lhs.span.merge(rhs.span);
            Ok(Expr {
                kind: ExprKind::BinOp(op, Box::new(lhs), Box::new(rhs)),
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_cons(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_add()?;
        if self.eat(&TokenKind::ColonColon).is_some() {
            let rhs = self.parse_cons()?; // right-associative
            let span = lhs.span.merge(rhs.span);
            Ok(Expr {
                kind: ExprKind::Cons(Box::new(lhs), Box::new(rhs)),
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => Some(BinOp::AddInt),
                TokenKind::Minus => Some(BinOp::SubInt),
                TokenKind::PlusDot => Some(BinOp::AddFloat),
                TokenKind::MinusDot => Some(BinOp::SubFloat),
                TokenKind::Caret => Some(BinOp::ConcatStr),
                _ => None,
            };
            if let Some(op) = op {
                self.advance();
                let rhs = self.parse_mul()?;
                let span = lhs.span.merge(rhs.span);
                lhs = Expr {
                    kind: ExprKind::BinOp(op, Box::new(lhs), Box::new(rhs)),
                    span,
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => Some(BinOp::MulInt),
                TokenKind::Slash => Some(BinOp::DivInt),
                TokenKind::Mod => Some(BinOp::ModInt),
                TokenKind::StarDot => Some(BinOp::MulFloat),
                TokenKind::SlashDot => Some(BinOp::DivFloat),
                _ => None,
            };
            if let Some(op) = op {
                self.advance();
                let rhs = self.parse_unary()?;
                let span = lhs.span.merge(rhs.span);
                lhs = Expr {
                    kind: ExprKind::BinOp(op, Box::new(lhs), Box::new(rhs)),
                    span,
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), TokenKind::Tilde) {
            let start = self.span();
            self.advance();
            let operand = self.parse_unary()?;
            let span = start.merge(operand.span);
            return Ok(Expr {
                kind: ExprKind::UnaryNeg(Box::new(operand)),
                span,
            });
        }
        if matches!(self.peek(), TokenKind::Not) {
            let start = self.span();
            self.advance();
            let operand = self.parse_unary()?;
            let span = start.merge(operand.span);
            return Ok(Expr {
                kind: ExprKind::Not(Box::new(operand)),
                span,
            });
        }
        self.parse_app()
    }

    fn parse_app(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_atom_expr()?;
        while self.can_start_app_arg() {
            let arg = self.parse_app_arg()?;
            let span = lhs.span.merge(arg.span);
            lhs = Expr {
                kind: ExprKind::App(Box::new(lhs), Box::new(arg)),
                span,
            };
        }
        Ok(lhs)
    }

    /// Parse an application argument: an atom, or a unary prefix (~, not) applied to an atom.
    fn parse_app_arg(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        if matches!(self.peek(), TokenKind::Tilde) {
            self.advance();
            let operand = self.parse_atom_expr()?;
            let span = start.merge(operand.span);
            return Ok(Expr {
                kind: ExprKind::UnaryNeg(Box::new(operand)),
                span,
            });
        }
        if matches!(self.peek(), TokenKind::Not) {
            self.advance();
            let operand = self.parse_atom_expr()?;
            let span = start.merge(operand.span);
            return Ok(Expr {
                kind: ExprKind::Not(Box::new(operand)),
                span,
            });
        }
        self.parse_atom_expr()
    }

    fn parse_atom_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        match self.peek() {
            TokenKind::IntLit(n) => {
                let n = *n;
                self.advance();
                Ok(Expr {
                    kind: ExprKind::IntLit(n),
                    span: start,
                })
            }
            TokenKind::FloatLit(f) => {
                let f = *f;
                self.advance();
                Ok(Expr {
                    kind: ExprKind::FloatLit(f),
                    span: start,
                })
            }
            TokenKind::StringLit(_) => {
                let s = self.take_string_lit();
                Ok(Expr {
                    kind: ExprKind::StringLit(s),
                    span: start,
                })
            }
            TokenKind::CharLit(c) => {
                let c = *c;
                self.advance();
                Ok(Expr {
                    kind: ExprKind::CharLit(c),
                    span: start,
                })
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::BoolLit(true),
                    span: start,
                })
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::BoolLit(false),
                    span: start,
                })
            }
            TokenKind::Ident(_) => {
                let name = self.take_symbol();
                Ok(Expr {
                    kind: ExprKind::Var(name),
                    span: start,
                })
            }
            TokenKind::UpperIdent(_) => {
                if self.can_start_qualified_path() {
                    let (name, is_constructor, span) = self.parse_qualified_symbol()?;
                    Ok(Expr {
                        kind: if is_constructor {
                            ExprKind::Constructor(name)
                        } else {
                            ExprKind::Var(name)
                        },
                        span,
                    })
                } else {
                    let name = self.take_symbol();
                    Ok(Expr {
                        kind: ExprKind::Constructor(name),
                        span: start,
                    })
                }
            }
            TokenKind::LParen => {
                self.advance();
                if self.eat(&TokenKind::RParen).is_some() {
                    return Ok(Expr {
                        kind: ExprKind::Unit,
                        span: start.merge(self.tokens[self.pos - 1].span),
                    });
                }
                let first = self.parse_expr()?;
                if self.eat(&TokenKind::Comma).is_some() {
                    let mut elems = vec![first];
                    elems.push(self.parse_expr()?);
                    while self.eat(&TokenKind::Comma).is_some() {
                        elems.push(self.parse_expr()?);
                    }
                    let end = self.expect(&TokenKind::RParen, ")")?;
                    let span = start.merge(end);
                    Ok(Expr {
                        kind: ExprKind::Tuple(elems),
                        span,
                    })
                } else {
                    let end = self.expect(&TokenKind::RParen, ")")?;
                    let span = start.merge(end);
                    Ok(Expr {
                        kind: ExprKind::Paren(Box::new(first)),
                        span,
                    })
                }
            }
            TokenKind::LBracket => {
                self.advance();
                if self.eat(&TokenKind::RBracket).is_some() {
                    return Ok(Expr {
                        kind: ExprKind::List(vec![]),
                        span: start.merge(self.tokens[self.pos - 1].span),
                    });
                }
                let mut elems = vec![self.parse_expr()?];
                while self.eat(&TokenKind::Comma).is_some() {
                    elems.push(self.parse_expr()?);
                }
                let end = self.expect(&TokenKind::RBracket, "]")?;
                let span = start.merge(end);
                Ok(Expr {
                    kind: ExprKind::List(elems),
                    span,
                })
            }
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Let => self.parse_let_expr(),
            TokenKind::Case => self.parse_case_expr(),
            TokenKind::Fn => self.parse_fn_expr(),
            TokenKind::Handle => self.parse_handle_expr(),
            TokenKind::Perform => self.parse_perform_expr(),
            TokenKind::Resume => self.parse_resume_expr(),
            _ => Err(self.err("expected expression")),
        }
    }

    fn parse_handle_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.advance(); // consume `handle`
        let body = self.parse_expr()?;
        self.expect(&TokenKind::With, "with")?;
        self.expect(&TokenKind::Return, "return")?;
        let (return_var, _) = self.expect_ident_or_wildcard()?;
        self.expect(&TokenKind::Arrow, "=>")?;
        let return_body = self.parse_expr()?;
        let mut handlers = Vec::new();
        while self.eat(&TokenKind::Bar).is_some() {
            let hstart = self.span();
            let (effect_name, _) = self.expect_upper_ident()?;
            let (payload_var, _) = self.expect_ident_or_wildcard()?;
            let (cont_var, _) = self.expect_ident_or_wildcard()?;
            self.expect(&TokenKind::Arrow, "=>")?;
            let hbody = self.parse_expr()?;
            let hspan = hstart.merge(hbody.span);
            handlers.push(EffectHandler {
                effect_name,
                payload_var,
                cont_var,
                body: hbody,
                span: hspan,
            });
        }
        let end_span = handlers.last().map(|h| h.span).unwrap_or(return_body.span);
        let span = start.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Handle {
                body: Box::new(body),
                return_var,
                return_body: Box::new(return_body),
                handlers,
            },
            span,
        })
    }

    fn parse_perform_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.advance(); // consume `perform`
        let (name, _) = self.expect_upper_ident()?;
        let arg = self.parse_atom_expr()?;
        let span = start.merge(arg.span);
        Ok(Expr {
            kind: ExprKind::Perform(name, Box::new(arg)),
            span,
        })
    }

    fn parse_resume_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.advance(); // consume `resume`
        let cont = self.parse_atom_expr()?;
        let arg = self.parse_atom_expr()?;
        let span = start.merge(arg.span);
        Ok(Expr {
            kind: ExprKind::Resume(Box::new(cont), Box::new(arg)),
            span,
        })
    }

    fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.advance(); // consume `if`
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::Then, "then")?;
        let then_br = self.parse_expr()?;
        self.expect(&TokenKind::Else, "else")?;
        let else_br = self.parse_expr()?;
        let span = start.merge(else_br.span);
        Ok(Expr {
            kind: ExprKind::If(Box::new(cond), Box::new(then_br), Box::new(else_br)),
            span,
        })
    }

    fn parse_let_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.advance(); // consume `let`
        let mut decls = Vec::new();
        while !matches!(self.peek(), TokenKind::In | TokenKind::Eof) {
            decls.push(self.parse_decl()?);
        }
        self.expect(&TokenKind::In, "in")?;
        let body = self.parse_expr()?;
        let end = self.expect(&TokenKind::End, "end")?;
        let span = start.merge(end);
        Ok(Expr {
            kind: ExprKind::Let(decls, Box::new(body)),
            span,
        })
    }

    fn parse_case_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.advance(); // consume `case`
        let scrutinee = self.parse_expr()?;
        self.expect(&TokenKind::Of, "of")?;
        self.eat(&TokenKind::Bar); // optional leading |
        let mut branches = Vec::new();
        loop {
            let pat = self.parse_pat()?;
            self.expect(&TokenKind::Arrow, "=>")?;
            let body = self.parse_expr()?;
            branches.push((pat, body));
            if self.eat(&TokenKind::Bar).is_none() {
                break;
            }
        }
        let last_span = branches.last().unwrap().1.span;
        let span = start.merge(last_span);
        Ok(Expr {
            kind: ExprKind::Case(Box::new(scrutinee), branches),
            span,
        })
    }

    fn parse_fn_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.advance(); // consume `fn`
        let pat = self.parse_pat()?;
        self.expect(&TokenKind::Arrow, "=>")?;
        let body = self.parse_expr()?;
        let span = start.merge(body.span);
        Ok(Expr {
            kind: ExprKind::Fn(pat, Box::new(body)),
            span,
        })
    }

    // ── Patterns ─────────────────────────────────────────────────────

    fn parse_pat(&mut self) -> Result<Pat, ParseError> {
        self.guard_depth()?;
        let result = self.parse_pat_inner_dispatch();
        self.unguard_depth();
        result
    }

    fn parse_pat_inner_dispatch(&mut self) -> Result<Pat, ParseError> {
        let pat = self.parse_as_pat()?;
        if self.eat(&TokenKind::Colon).is_some() {
            let ty = self.parse_type_expr()?;
            let span = pat.span.merge(ty.span);
            Ok(Pat {
                kind: PatKind::Ann(Box::new(pat), ty),
                span,
            })
        } else {
            Ok(pat)
        }
    }

    fn parse_as_pat(&mut self) -> Result<Pat, ParseError> {
        // Check for `ident as pat`
        if matches!(self.peek(), TokenKind::Ident(_))
            && self.pos + 1 < self.tokens.len()
            && matches!(self.tokens[self.pos + 1].kind, TokenKind::As)
        {
            let start = self.span();
            let name = self.take_symbol(); // consume ident
            self.advance(); // consume `as`
            let inner = self.parse_as_pat()?;
            let span = start.merge(inner.span);
            return Ok(Pat {
                kind: PatKind::As(name, Box::new(inner)),
                span,
            });
        }
        self.parse_cons_pat()
    }

    fn parse_cons_pat(&mut self) -> Result<Pat, ParseError> {
        let lhs = self.parse_app_pat()?;
        if self.eat(&TokenKind::ColonColon).is_some() {
            let rhs = self.parse_cons_pat()?; // right-associative
            let span = lhs.span.merge(rhs.span);
            Ok(Pat {
                kind: PatKind::Cons(Box::new(lhs), Box::new(rhs)),
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_app_pat(&mut self) -> Result<Pat, ParseError> {
        if matches!(self.peek(), TokenKind::UpperIdent(_)) {
            let start = self.span();
            let name = self.take_symbol();
            if self.can_start_atom_pat() {
                let payload = self.parse_atom_pat()?;
                let span = start.merge(payload.span);
                Ok(Pat {
                    kind: PatKind::Constructor(name, Some(Box::new(payload))),
                    span,
                })
            } else {
                Ok(Pat {
                    kind: PatKind::Constructor(name, None),
                    span: start,
                })
            }
        } else {
            self.parse_atom_pat()
        }
    }

    fn parse_atom_pat(&mut self) -> Result<Pat, ParseError> {
        let start = self.span();
        match self.peek() {
            TokenKind::Underscore => {
                self.advance();
                Ok(Pat {
                    kind: PatKind::Wildcard,
                    span: start,
                })
            }
            TokenKind::Ident(_) => {
                let name = self.take_symbol();
                Ok(Pat {
                    kind: PatKind::Var(name),
                    span: start,
                })
            }
            TokenKind::UpperIdent(_) => {
                let name = self.take_symbol();
                Ok(Pat {
                    kind: PatKind::Constructor(name, None),
                    span: start,
                })
            }
            TokenKind::IntLit(n) => {
                let n = *n;
                self.advance();
                Ok(Pat {
                    kind: PatKind::IntLit(n),
                    span: start,
                })
            }
            TokenKind::FloatLit(f) => {
                let f = *f;
                self.advance();
                Ok(Pat {
                    kind: PatKind::FloatLit(f),
                    span: start,
                })
            }
            TokenKind::StringLit(_) => {
                let s = self.take_string_lit();
                Ok(Pat {
                    kind: PatKind::StringLit(s),
                    span: start,
                })
            }
            TokenKind::CharLit(c) => {
                let c = *c;
                self.advance();
                Ok(Pat {
                    kind: PatKind::CharLit(c),
                    span: start,
                })
            }
            TokenKind::True => {
                self.advance();
                Ok(Pat {
                    kind: PatKind::BoolLit(true),
                    span: start,
                })
            }
            TokenKind::False => {
                self.advance();
                Ok(Pat {
                    kind: PatKind::BoolLit(false),
                    span: start,
                })
            }
            TokenKind::Tilde => {
                self.advance();
                match self.peek() {
                    TokenKind::IntLit(n) => {
                        let n = *n;
                        let end = self.span();
                        self.advance();
                        Ok(Pat {
                            kind: PatKind::IntLit(-n),
                            span: start.merge(end),
                        })
                    }
                    TokenKind::FloatLit(f) => {
                        let f = *f;
                        let end = self.span();
                        self.advance();
                        Ok(Pat {
                            kind: PatKind::FloatLit(-f),
                            span: start.merge(end),
                        })
                    }
                    _ => Err(self.err("expected number after ~ in pattern")),
                }
            }
            TokenKind::LParen => {
                self.advance();
                if self.eat(&TokenKind::RParen).is_some() {
                    return Ok(Pat {
                        kind: PatKind::Unit,
                        span: start.merge(self.tokens[self.pos - 1].span),
                    });
                }
                let first = self.parse_pat()?;
                if self.eat(&TokenKind::Comma).is_some() {
                    let mut elems = vec![first];
                    elems.push(self.parse_pat()?);
                    while self.eat(&TokenKind::Comma).is_some() {
                        elems.push(self.parse_pat()?);
                    }
                    let end = self.expect(&TokenKind::RParen, ")")?;
                    let span = start.merge(end);
                    Ok(Pat {
                        kind: PatKind::Tuple(elems),
                        span,
                    })
                } else {
                    let end = self.expect(&TokenKind::RParen, ")")?;
                    let span = start.merge(end);
                    Ok(Pat {
                        kind: PatKind::Paren(Box::new(first)),
                        span,
                    })
                }
            }
            TokenKind::LBracket => {
                self.advance();
                if self.eat(&TokenKind::RBracket).is_some() {
                    return Ok(Pat {
                        kind: PatKind::List(vec![]),
                        span: start.merge(self.tokens[self.pos - 1].span),
                    });
                }
                let mut elems = vec![self.parse_pat()?];
                while self.eat(&TokenKind::Comma).is_some() {
                    elems.push(self.parse_pat()?);
                }
                let end = self.expect(&TokenKind::RBracket, "]")?;
                let span = start.merge(end);
                Ok(Pat {
                    kind: PatKind::List(elems),
                    span,
                })
            }
            _ => Err(self.err("expected pattern")),
        }
    }

    // ── Type expressions ─────────────────────────────────────────────

    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        self.guard_depth()?;
        let result = self.parse_arrow_type();
        self.unguard_depth();
        result
    }

    fn parse_arrow_type(&mut self) -> Result<TypeExpr, ParseError> {
        let lhs = self.parse_tuple_type()?;
        if self.eat(&TokenKind::ThinArrow).is_some() {
            let rhs = self.parse_arrow_type()?; // right-associative
            let span = lhs.span.merge(rhs.span);
            Ok(TypeExpr {
                kind: TypeExprKind::Arrow(Box::new(lhs), Box::new(rhs)),
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_tuple_type(&mut self) -> Result<TypeExpr, ParseError> {
        let first = self.parse_app_type()?;
        if matches!(self.peek(), TokenKind::Star) {
            let mut elems = vec![first];
            while self.eat(&TokenKind::Star).is_some() {
                elems.push(self.parse_app_type()?);
            }
            let span = elems[0].span.merge(elems.last().unwrap().span);
            Ok(TypeExpr {
                kind: TypeExprKind::Tuple(elems),
                span,
            })
        } else {
            Ok(first)
        }
    }

    fn parse_app_type(&mut self) -> Result<TypeExpr, ParseError> {
        let mut ty = self.parse_atom_type()?;
        while matches!(self.peek(), TokenKind::Ident(_) | TokenKind::UpperIdent(_)) {
            let end = self.span();
            let name = self.take_symbol();
            let (args, base_span) = match ty.kind {
                TypeExprKind::Tuple(elems) => (elems, ty.span),
                _ => {
                    let span = ty.span;
                    (vec![ty], span)
                }
            };
            let span = base_span.merge(end);
            ty = TypeExpr {
                kind: TypeExprKind::App(name, args),
                span,
            };
        }
        Ok(ty)
    }

    fn parse_atom_type(&mut self) -> Result<TypeExpr, ParseError> {
        let start = self.span();
        match self.peek() {
            TokenKind::TyVar(_) => {
                let name = self.take_symbol();
                Ok(TypeExpr {
                    kind: TypeExprKind::Var(name),
                    span: start,
                })
            }
            TokenKind::Ident(_) | TokenKind::UpperIdent(_) => {
                let name = self.take_symbol();
                Ok(TypeExpr {
                    kind: TypeExprKind::Named(name),
                    span: start,
                })
            }
            TokenKind::LParen => {
                self.advance();
                let first = self.parse_type_expr()?;
                if self.eat(&TokenKind::Comma).is_some() {
                    // Multi-arg: (ty1, ty2, ...) stored as Tuple, converted to App in parse_app_type
                    let mut elems = vec![first];
                    elems.push(self.parse_type_expr()?);
                    while self.eat(&TokenKind::Comma).is_some() {
                        elems.push(self.parse_type_expr()?);
                    }
                    let end = self.expect(&TokenKind::RParen, ")")?;
                    let span = start.merge(end);
                    Ok(TypeExpr {
                        kind: TypeExprKind::Tuple(elems),
                        span,
                    })
                } else {
                    let end = self.expect(&TokenKind::RParen, ")")?;
                    let span = start.merge(end);
                    Ok(TypeExpr {
                        kind: TypeExprKind::Paren(Box::new(first)),
                        span,
                    })
                }
            }
            _ => Err(self.err("expected type")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(input: &str) -> Program {
        let mut lexer = Lexer::new(input, 0);
        let tokens = lexer.tokenize().expect("lex error");
        let mut parser = Parser::new(tokens);
        parser.parse_program().expect("parse error")
    }

    #[allow(dead_code)]
    fn parse_err(input: &str) -> String {
        let mut lexer = Lexer::new(input, 0);
        let tokens = lexer.tokenize().expect("lex error");
        let mut parser = Parser::new(tokens);
        parser.parse_program().unwrap_err().message
    }

    #[test]
    fn test_val_binding() {
        let prog = parse("val x = 42");
        assert_eq!(prog.decls.len(), 1);
        assert!(matches!(&prog.decls[0].kind, DeclKind::Val(_, _)));
    }

    #[test]
    fn test_val_rec() {
        let prog = parse("val rec f = fn x => x");
        if let DeclKind::ValRec(name, _) = &prog.decls[0].kind {
            assert_eq!(prog.interner.resolve(*name), "f");
        } else {
            panic!("expected ValRec");
        }
    }

    #[test]
    fn test_val_rec_requires_fn() {
        assert_eq!(
            parse_err("val rec x = 1"),
            "val rec requires fn on the right-hand side"
        );
    }

    #[test]
    fn test_app_with_unary_neg() {
        // f ~1 should parse as App(f, UnaryNeg(1))
        let prog = parse("val x = f ~1");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::App(_, ref arg) = expr.kind {
                assert!(matches!(&arg.kind, ExprKind::UnaryNeg(_)));
            } else {
                panic!("expected App");
            }
        }
    }

    #[test]
    fn test_app_with_not() {
        let prog = parse("val x = f not true");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::App(_, ref arg) = expr.kind {
                assert!(matches!(&arg.kind, ExprKind::Not(_)));
            } else {
                panic!("expected App");
            }
        }
    }

    #[test]
    fn test_constructor_with_unary_neg() {
        let prog = parse("val x = Some ~1");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::App(_, ref arg) = expr.kind {
                assert!(matches!(&arg.kind, ExprKind::UnaryNeg(_)));
            } else {
                panic!("expected App");
            }
        }
    }

    #[test]
    fn test_fun_simple() {
        let prog = parse("fun add x y = x + y");
        if let DeclKind::Fun(bindings) = &prog.decls[0].kind {
            assert_eq!(prog.interner.resolve(bindings[0].name), "add");
            assert_eq!(bindings[0].clauses[0].pats.len(), 2);
        } else {
            panic!("expected fun");
        }
    }

    #[test]
    fn test_fun_clausal() {
        let prog = parse("fun f 0 = 1 | f n = n");
        if let DeclKind::Fun(bindings) = &prog.decls[0].kind {
            assert_eq!(bindings[0].clauses.len(), 2);
        } else {
            panic!("expected fun");
        }
    }

    #[test]
    fn test_fun_mutual() {
        let prog = parse("fun f x = g x and g y = f y");
        if let DeclKind::Fun(bindings) = &prog.decls[0].kind {
            assert_eq!(bindings.len(), 2);
            assert_eq!(prog.interner.resolve(bindings[0].name), "f");
            assert_eq!(prog.interner.resolve(bindings[1].name), "g");
        } else {
            panic!("expected fun");
        }
    }

    #[test]
    fn test_datatype_simple() {
        let prog = parse("datatype shape = Circle of Float | Rect of Float * Float");
        if let DeclKind::Datatype(dt) = &prog.decls[0].kind {
            assert_eq!(prog.interner.resolve(dt.name), "shape");
            assert_eq!(dt.tyvars.len(), 0);
            assert_eq!(dt.constructors.len(), 2);
        } else {
            panic!("expected datatype");
        }
    }

    #[test]
    fn test_datatype_parameterized() {
        let prog = parse("datatype 'a option = None | Some of 'a");
        if let DeclKind::Datatype(dt) = &prog.decls[0].kind {
            let tyvars: Vec<&str> = dt
                .tyvars
                .iter()
                .map(|s| prog.interner.resolve(*s))
                .collect();
            assert_eq!(tyvars, vec!["'a"]);
            assert_eq!(dt.constructors.len(), 2);
        } else {
            panic!("expected datatype");
        }
    }

    #[test]
    fn test_datatype_multi_param() {
        let prog = parse("datatype ('a, 'b) either = Left of 'a | Right of 'b");
        if let DeclKind::Datatype(dt) = &prog.decls[0].kind {
            let tyvars: Vec<&str> = dt
                .tyvars
                .iter()
                .map(|s| prog.interner.resolve(*s))
                .collect();
            assert_eq!(tyvars, vec!["'a", "'b"]);
        } else {
            panic!("expected datatype");
        }
    }

    #[test]
    fn test_type_alias() {
        let prog = parse("type point = Float * Float");
        assert!(matches!(&prog.decls[0].kind, DeclKind::TypeAlias(_)));
    }

    #[test]
    fn test_use_decl() {
        let prog = parse(r#"use "foo.hml""#);
        assert!(matches!(&prog.decls[0].kind, DeclKind::Use(path) if path == "foo.hml"));
    }

    #[test]
    fn test_signature_decl() {
        let prog = parse("signature LIST = sig val fold : Int -> Int end");
        if let DeclKind::Signature(sig) = &prog.decls[0].kind {
            assert_eq!(prog.interner.resolve(sig.name), "LIST");
            assert_eq!(sig.specs.len(), 1);
        } else {
            panic!("expected signature");
        }
    }

    #[test]
    fn test_structure_decl() {
        let prog = parse("structure List = struct fun fold f acc xs = acc end");
        if let DeclKind::Structure {
            name,
            signature,
            opaque,
            decls,
        } = &prog.decls[0].kind
        {
            assert_eq!(prog.interner.resolve(*name), "List");
            assert!(signature.is_none());
            assert!(!opaque);
            assert_eq!(decls.len(), 1);
        } else {
            panic!("expected structure");
        }
    }

    #[test]
    fn test_structure_decl_with_signature() {
        let prog = parse("structure List : LIST = struct fun fold f acc xs = acc end");
        if let DeclKind::Structure {
            name,
            signature,
            opaque,
            decls,
        } = &prog.decls[0].kind
        {
            assert_eq!(prog.interner.resolve(*name), "List");
            assert_eq!(signature.map(|sym| prog.interner.resolve(sym)), Some("LIST"));
            assert!(!opaque);
            assert_eq!(decls.len(), 1);
        } else {
            panic!("expected structure");
        }
    }

    #[test]
    fn test_structure_decl_with_opaque_signature() {
        let prog = parse("structure List :> LIST = struct fun fold f acc xs = acc end");
        if let DeclKind::Structure {
            name,
            signature,
            opaque,
            decls,
        } = &prog.decls[0].kind
        {
            assert_eq!(prog.interner.resolve(*name), "List");
            assert_eq!(signature.map(|sym| prog.interner.resolve(sym)), Some("LIST"));
            assert!(*opaque);
            assert_eq!(decls.len(), 1);
        } else {
            panic!("expected structure");
        }
    }

    #[test]
    fn test_qualified_access_expr() {
        let prog = parse("val sum = List.fold add 0 xs");
        if let DeclKind::Val(_, expr) = &prog.decls[0].kind {
            match &expr.kind {
                ExprKind::App(func, _) => match &func.kind {
                    ExprKind::App(func, _) => match &func.kind {
                        ExprKind::App(func, _) => match &func.kind {
                            ExprKind::Var(sym) => {
                                assert_eq!(prog.interner.resolve(*sym), "List.fold");
                            }
                            _ => panic!("expected qualified var"),
                        },
                        _ => panic!("expected application chain"),
                    },
                    _ => panic!("expected application chain"),
                },
                _ => panic!("expected application"),
            }
        } else {
            panic!("expected val");
        }
    }

    #[test]
    fn test_let_expr() {
        let prog = parse("val x = let val a = 1 in a + 2 end");
        assert!(matches!(&prog.decls[0].kind, DeclKind::Val(_, _)));
    }

    #[test]
    fn test_case_expr() {
        let prog = parse("val x = case y of 0 => 1 | n => n");
        assert!(matches!(&prog.decls[0].kind, DeclKind::Val(_, _)));
    }

    #[test]
    fn test_fn_expr() {
        let prog = parse("val f = fn x => x + 1");
        assert!(matches!(&prog.decls[0].kind, DeclKind::Val(_, _)));
    }

    #[test]
    fn test_if_expr() {
        let prog = parse("val x = if true then 1 else 2");
        assert!(matches!(&prog.decls[0].kind, DeclKind::Val(_, _)));
    }

    #[test]
    fn test_tuple_expr() {
        let prog = parse("val x = (1, 2, 3)");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            assert!(matches!(&expr.kind, ExprKind::Tuple(elems) if elems.len() == 3));
        }
    }

    #[test]
    fn test_list_expr() {
        let prog = parse("val x = [1, 2, 3]");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            assert!(matches!(&expr.kind, ExprKind::List(elems) if elems.len() == 3));
        }
    }

    #[test]
    fn test_cons_expr() {
        let prog = parse("val x = 1 :: 2 :: []");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            assert!(matches!(&expr.kind, ExprKind::Cons(_, _)));
        }
    }

    #[test]
    fn test_operator_precedence() {
        // 1 + 2 * 3 should be 1 + (2 * 3)
        let prog = parse("val x = 1 + 2 * 3");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::BinOp(BinOp::AddInt, _, ref rhs) = expr.kind {
                assert!(matches!(&rhs.kind, ExprKind::BinOp(BinOp::MulInt, _, _)));
            } else {
                panic!("expected AddInt at top");
            }
        }
    }

    #[test]
    fn test_application() {
        let prog = parse("val x = f a b");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            // f a b = (f a) b
            if let ExprKind::App(ref func, _) = expr.kind {
                assert!(matches!(&func.kind, ExprKind::App(_, _)));
            } else {
                panic!("expected App");
            }
        }
    }

    #[test]
    fn test_pattern_cons() {
        let prog = parse("val (x :: xs) = [1, 2]");
        if let DeclKind::Val(ref pat, _) = prog.decls[0].kind {
            if let PatKind::Paren(ref inner) = pat.kind {
                assert!(matches!(&inner.kind, PatKind::Cons(_, _)));
            }
        }
    }

    #[test]
    fn test_pattern_constructor() {
        let prog = parse("fun f (Some x) = x | f None = 0");
        if let DeclKind::Fun(ref bindings) = prog.decls[0].kind {
            assert_eq!(bindings[0].clauses.len(), 2);
        }
    }

    #[test]
    fn test_type_annotation() {
        let prog = parse("val x = (42 : Int)");
        assert!(matches!(&prog.decls[0].kind, DeclKind::Val(_, _)));
    }

    #[test]
    fn test_arrow_type() {
        let prog = parse("type f = Int -> Bool");
        if let DeclKind::TypeAlias(ref ta) = prog.decls[0].kind {
            assert!(matches!(&ta.ty.kind, TypeExprKind::Arrow(_, _)));
        }
    }

    #[test]
    fn test_type_app() {
        let prog = parse("type xs = Int list");
        if let DeclKind::TypeAlias(ref ta) = prog.decls[0].kind {
            if let TypeExprKind::App(sym, _) = &ta.ty.kind {
                assert_eq!(prog.interner.resolve(*sym), "list");
            } else {
                panic!("expected App");
            }
        }
    }

    #[test]
    fn test_local_decl() {
        let prog = parse("local val x = 1 in val y = x end");
        assert!(matches!(&prog.decls[0].kind, DeclKind::Local(_, _)));
    }

    #[test]
    fn test_negative_pattern() {
        let prog = parse("fun f ~1 = true | f _ = false");
        if let DeclKind::Fun(ref bindings) = prog.decls[0].kind {
            if let PatKind::IntLit(n) = &bindings[0].clauses[0].pats[0].kind {
                assert_eq!(*n, -1);
            }
        }
    }

    #[test]
    fn test_as_pattern() {
        let prog = parse("val (x as Some _) = y");
        if let DeclKind::Val(ref pat, _) = prog.decls[0].kind {
            if let PatKind::Paren(ref inner) = pat.kind {
                if let PatKind::As(sym, _) = &inner.kind {
                    assert_eq!(prog.interner.resolve(*sym), "x");
                } else {
                    panic!("expected As");
                }
            }
        }
    }

    #[test]
    fn test_float_operators() {
        let prog = parse("val x = 1.0 +. 2.0 *. 3.0");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::BinOp(BinOp::AddFloat, _, ref rhs) = expr.kind {
                assert!(matches!(&rhs.kind, ExprKind::BinOp(BinOp::MulFloat, _, _)));
            } else {
                panic!("expected AddFloat");
            }
        }
    }

    #[test]
    fn test_unit_expr() {
        let prog = parse("val x = ()");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            assert!(matches!(&expr.kind, ExprKind::Unit));
        }
    }

    #[test]
    fn test_empty_list() {
        let prog = parse("val x = []");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            assert!(matches!(&expr.kind, ExprKind::List(elems) if elems.is_empty()));
        }
    }

    #[test]
    fn test_orelse_right_assoc() {
        // a orelse b orelse c should be a orelse (b orelse c)
        let prog = parse("val x = a orelse b orelse c");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::BinOp(BinOp::Orelse, _, ref rhs) = expr.kind {
                assert!(matches!(&rhs.kind, ExprKind::BinOp(BinOp::Orelse, _, _)));
            } else {
                panic!("expected Orelse at top");
            }
        }
    }

    #[test]
    fn test_andalso_right_assoc() {
        let prog = parse("val x = a andalso b andalso c");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::BinOp(BinOp::Andalso, _, ref rhs) = expr.kind {
                assert!(matches!(&rhs.kind, ExprKind::BinOp(BinOp::Andalso, _, _)));
            } else {
                panic!("expected Andalso at top");
            }
        }
    }

    #[test]
    fn test_complex_program() {
        // A realistic multi-declaration program
        let src = r#"
            datatype 'a option = None | Some of 'a

            fun map_option f opt =
              case opt of
                None   => None
              | Some x => Some (f x)

            val result = map_option (fn x => x + 1) (Some 42)
        "#;
        let prog = parse(src);
        assert_eq!(prog.decls.len(), 3);
    }
}
