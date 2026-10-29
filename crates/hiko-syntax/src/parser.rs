use crate::ast::*;
use crate::intern::{StringInterner, Symbol};
use crate::span::Span;
use crate::token::{Token, TokenKind};

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

/// Maximum depth of the AST a program may produce, counting every
/// declaration, expression, pattern and type node on the path from a
/// top-level declaration. Every pass after the parser recurses over the
/// tree, so this bounds their stack use too. The parser's own recursion is
/// held to the same limit.
const MAX_NESTING_DEPTH: u32 = 256;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Number of `nested` parse steps currently open.
    depth: u32,
    /// Deepest level reached since the innermost open operator chain
    /// started, including the levels that finished chains add below their
    /// operands. See `Chain`.
    peak: u32,
    /// Set after an `fn`, `if`, `case` or `handle` expression. These extend
    /// as far right as they can, so nothing may follow one as an operand:
    /// no operator and no argument, until the enclosing `parse_expr` ends.
    closed: bool,
    interner: StringInterner,
}

/// An operator chain being parsed, such as `a + b + c` or `f x y`.
///
/// Chains are parsed in a loop, but each link puts the operands one AST level
/// deeper: in `a + b + c` the operand `a` sits two levels below the chain. To
/// keep `(... (x + 1 + ... + 1) ...)` from building a tree hundreds of times
/// deeper than the parser recursed, each chain records how deep its operands
/// went (`Parser::peak`) and adds the levels its links put above them.
struct Chain {
    /// Left-associative chains put the earliest operands deepest,
    /// right-associative ones the latest.
    left_assoc: bool,
    links: u32,
    /// Deepest operand so far, as levels below the chain, minus the link
    /// count for a left-associative chain (see `end_operand`).
    height: i64,
    outer_peak: u32,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
            peak: 0,
            closed: false,
            interner: StringInterner::new(),
        }
    }

    pub fn with_interner(tokens: Vec<Token>, interner: StringInterner) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
            peak: 0,
            closed: false,
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

    /// Whether the current token is of the same kind as `kind`, ignoring payloads.
    fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(kind)
    }

    fn eat(&mut self, kind: &TokenKind) -> Option<Span> {
        if self.at(kind) {
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

    fn expect_exposed_import_name(&mut self) -> Result<(Symbol, Span), ParseError> {
        if matches!(self.peek(), TokenKind::Ident(_) | TokenKind::UpperIdent(_)) {
            let span = self.span();
            Ok((self.take_symbol(), span))
        } else {
            Err(self.err("expected exposed import name"))
        }
    }

    fn expect_import_package_name(&mut self) -> Result<(Symbol, Span), ParseError> {
        match self.peek() {
            TokenKind::UpperIdent(_) => {
                let span = self.span();
                Ok((self.take_symbol(), span))
            }
            TokenKind::Ident(name) if name.starts_with("__") => {
                let span = self.span();
                Ok((self.take_symbol(), span))
            }
            _ => Err(self.err("expected package name")),
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

    /// Run a recursive parse step under the nesting limit, so deeply nested
    /// input fails with a parse error instead of overflowing the stack.
    fn nested<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(self.err("nesting limit exceeded"));
        }
        self.depth += 1;
        self.peak = self.peak.max(self.depth);
        let result = parse(self);
        self.depth -= 1;
        result
    }

    fn begin_chain(&mut self, left_assoc: bool) -> Chain {
        Chain {
            left_assoc,
            links: 0,
            height: 0,
            outer_peak: std::mem::replace(&mut self.peak, self.depth),
        }
    }

    /// Record the operand just parsed. With `n` links in all, operand `k`
    /// ends up at least `n - k` levels below a left-associative chain and
    /// `k` levels below a right-associative one (one less than exact for
    /// all but the extreme operand, which is enough to bound the tree).
    fn end_operand(&mut self, chain: &mut Chain) {
        let below = i64::from(self.peak - self.depth);
        let links = i64::from(chain.links);
        let height = if chain.left_assoc {
            below - links
        } else {
            below + links
        };
        chain.height = chain.height.max(height);
        self.peak = self.depth;
    }

    /// Count one more link. A chain of `MAX_NESTING_DEPTH` links is over the
    /// limit whatever its operands are, so stop before building more.
    fn chain_link(&self, chain: &mut Chain) -> Result<(), ParseError> {
        chain.links += 1;
        if chain.links >= MAX_NESTING_DEPTH {
            return Err(self.err("nesting limit exceeded"));
        }
        Ok(())
    }

    fn end_chain(&mut self, chain: Chain) -> Result<(), ParseError> {
        let height = if chain.left_assoc {
            chain.height + i64::from(chain.links)
        } else {
            chain.height
        };
        let peak = i64::from(self.depth) + height;
        if peak > i64::from(MAX_NESTING_DEPTH) {
            return Err(self.err("nesting limit exceeded"));
        }
        // `peak` is at least `self.depth`: every operand height is >= 0.
        self.peak = chain.outer_peak.max(peak as u32);
        Ok(())
    }

    fn can_start_app_arg(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::IntLit(_)
                | TokenKind::FloatLit(_)
                | TokenKind::WordLit(_)
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
                | TokenKind::WordLit(_)
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
            let decl = match self.peek() {
                TokenKind::Signature => self.parse_signature_decl()?,
                TokenKind::Structure => self.parse_structure_decl()?,
                _ => self.parse_decl()?,
            };
            decls.push(decl);
        }
        for decl in &decls {
            check_decl_nesting(decl, 1)?;
        }
        Ok(Program {
            decls,
            interner: std::mem::take(&mut self.interner),
        })
    }

    // ── Declarations ─────────────────────────────────────────────────

    /// Parse a declaration in any position. `signature` and `structure` are
    /// handled by `parse_program` because modules are a single top-level
    /// layer; desugaring has no representation for nested ones.
    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        match self.peek() {
            TokenKind::Val => self.parse_val_decl(),
            TokenKind::Fun => self.parse_fun_decl(),
            TokenKind::Datatype => self.parse_datatype_decl(),
            TokenKind::Type => self.parse_type_alias_decl(),
            TokenKind::Local => self.nested(Self::parse_local_decl),
            TokenKind::Import => self.parse_import_decl(),
            TokenKind::Use => self.parse_use_decl(),
            TokenKind::Signature | TokenKind::Structure => Err(self.err(
                "signature and structure declarations are only allowed at top level",
            )),
            TokenKind::Effect => self.parse_effect_decl(),
            _ => {
                Err(self
                    .err("expected declaration (val, fun, datatype, type, local, import, use, signature, structure, or effect)"))
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
        let mut clauses: Vec<FunClause> = Vec::new();

        loop {
            let clause_start = self.span();
            let mut pats = Vec::new();
            while self.can_start_atom_pat() {
                pats.push(self.parse_atom_pat()?);
            }
            if pats.is_empty() {
                return Err(self.err("expected at least one parameter pattern"));
            }
            // Desugaring matches all clauses against one tuple of arguments.
            if let Some(first) = clauses.first()
                && first.pats.len() != pats.len()
            {
                return Err(ParseError {
                    message: format!(
                        "clauses of '{}' take different numbers of arguments: {} and {}",
                        self.interner.resolve(name),
                        first.pats.len(),
                        pats.len()
                    ),
                    span: clause_start.merge(pats.last().unwrap().span),
                });
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
            if !(path.starts_with("./") || path.starts_with("../")) {
                return Err(
                    self.err("use expects an explicit relative path beginning with './' or '../'")
                );
            }
            let span = start.merge(end);
            Ok(Decl {
                kind: DeclKind::Use(path),
                span,
            })
        } else {
            Err(self.err("expected string literal after 'use'"))
        }
    }

    fn parse_import_decl(&mut self) -> Result<Decl, ParseError> {
        let start = self.span();
        self.advance(); // consume `import`
        let (package, _) = self.expect_import_package_name()?;
        self.expect(&TokenKind::Dot, ".")?;
        let (module, mut end) = self.expect_upper_ident()?;
        if matches!(self.peek(), TokenKind::Dot) {
            return Err(self.err("import expects exactly two segments: Package.Module"));
        }
        let name = self.interner.intern(&format!(
            "{}.{}",
            self.interner.resolve(package),
            self.interner.resolve(module)
        ));

        // Check for optional (Name1, Name2, ...) exposure list
        if matches!(self.peek(), TokenKind::LParen) {
            self.advance(); // consume `(`
            let mut names = Vec::new();
            if matches!(self.peek(), TokenKind::RParen) {
                return Err(self.err("import exposure list cannot be empty"));
            }
            let (first, _) = self.expect_exposed_import_name()?;
            names.push(first);
            while matches!(self.peek(), TokenKind::Comma) {
                self.advance(); // consume `,`
                let (n, _) = self.expect_exposed_import_name()?;
                names.push(n);
            }
            end = self.expect(&TokenKind::RParen, ")")?;
            return Ok(Decl {
                kind: DeclKind::ImportWithNames(name, names),
                span: start.merge(end),
            });
        }

        Ok(Decl {
            kind: DeclKind::Import(name),
            span: start.merge(end),
        })
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
            TokenKind::Type => {
                self.advance();
                let tyvars = self.parse_tyvar_params()?;
                let (name, end) = self.expect_name()?;
                Ok(SignatureSpec::Type {
                    tyvars,
                    name,
                    span: start.merge(end),
                })
            }
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
            _ => Err(self.err("expected signature spec (type or val)")),
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
        let expr = self.nested(Self::parse_expr_inner);
        self.closed = false;
        expr
    }

    fn parse_expr_inner(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_pipe()?;
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

    fn parse_pipe(&mut self) -> Result<Expr, ParseError> {
        self.parse_left_chain(Self::parse_orelse, |token| match token {
            TokenKind::PipeGt => Some(BinOp::Pipe),
            _ => None,
        })
    }

    fn parse_orelse(&mut self) -> Result<Expr, ParseError> {
        self.parse_right_chain(Self::parse_andalso, &TokenKind::Orelse, |lhs, rhs| {
            ExprKind::BinOp(BinOp::Orelse, lhs, rhs)
        })
    }

    fn parse_andalso(&mut self) -> Result<Expr, ParseError> {
        self.parse_right_chain(Self::parse_comp, &TokenKind::Andalso, |lhs, rhs| {
            ExprKind::BinOp(BinOp::Andalso, lhs, rhs)
        })
    }

    fn parse_comp(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_cons()?;
        let op = match self.peek() {
            TokenKind::Eq => Some(BinOp::Eq),
            TokenKind::Ne => Some(BinOp::Ne),
            TokenKind::Lt => Some(BinOp::Lt),
            TokenKind::Gt => Some(BinOp::Gt),
            TokenKind::Le => Some(BinOp::Le),
            TokenKind::Ge => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op.filter(|_| !self.closed) {
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
        self.parse_right_chain(Self::parse_add, &TokenKind::ColonColon, ExprKind::Cons)
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        self.parse_left_chain(Self::parse_mul, |token| match token {
            TokenKind::Plus => Some(BinOp::Add),
            TokenKind::Minus => Some(BinOp::Sub),
            TokenKind::Caret => Some(BinOp::ConcatStr),
            _ => None,
        })
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        self.parse_left_chain(Self::parse_unary, |token| match token {
            TokenKind::Star => Some(BinOp::Mul),
            TokenKind::Slash => Some(BinOp::Div),
            TokenKind::Mod => Some(BinOp::Mod),
            _ => None,
        })
    }

    /// Parse `operand (op operand)*` into a left-associative tree, where `op`
    /// says which tokens are operators of this precedence level.
    fn parse_left_chain(
        &mut self,
        operand: fn(&mut Self) -> Result<Expr, ParseError>,
        op: fn(&TokenKind) -> Option<BinOp>,
    ) -> Result<Expr, ParseError> {
        let mut chain = self.begin_chain(true);
        let mut lhs = operand(self)?;
        self.end_operand(&mut chain);
        while !self.closed
            && let Some(op) = op(self.peek())
        {
            self.chain_link(&mut chain)?;
            self.advance();
            let rhs = operand(self)?;
            self.end_operand(&mut chain);
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        self.end_chain(chain)?;
        Ok(lhs)
    }

    /// Parse `operand (sep operand)*` into a right-associative tree whose
    /// nodes are built by `node`.
    fn parse_right_chain(
        &mut self,
        operand: fn(&mut Self) -> Result<Expr, ParseError>,
        sep: &TokenKind,
        node: fn(Box<Expr>, Box<Expr>) -> ExprKind,
    ) -> Result<Expr, ParseError> {
        let mut chain = self.begin_chain(false);
        let mut elems = vec![operand(self)?];
        self.end_operand(&mut chain);
        while !self.closed && self.at(sep) {
            self.chain_link(&mut chain)?;
            self.advance();
            elems.push(operand(self)?);
            self.end_operand(&mut chain);
        }
        self.end_chain(chain)?;
        let mut result = elems.pop().unwrap();
        while let Some(lhs) = elems.pop() {
            let span = lhs.span.merge(result.span);
            result = Expr {
                kind: node(Box::new(lhs), Box::new(result)),
                span,
            };
        }
        Ok(result)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let wrap: fn(Box<Expr>) -> ExprKind = match self.peek() {
            TokenKind::Tilde => ExprKind::UnaryNeg,
            TokenKind::Not => ExprKind::Not,
            _ => return self.parse_app(),
        };
        let start = self.span();
        self.advance();
        let operand = self.nested(Self::parse_unary)?;
        let span = start.merge(operand.span);
        Ok(Expr {
            kind: wrap(Box::new(operand)),
            span,
        })
    }

    fn parse_app(&mut self) -> Result<Expr, ParseError> {
        let mut chain = self.begin_chain(true);
        let mut lhs = self.parse_atom_expr()?;
        self.end_operand(&mut chain);
        while !self.closed && self.can_start_app_arg() {
            self.chain_link(&mut chain)?;
            let arg = self.parse_app_arg()?;
            self.end_operand(&mut chain);
            let span = lhs.span.merge(arg.span);
            lhs = Expr {
                kind: ExprKind::App(Box::new(lhs), Box::new(arg)),
                span,
            };
        }
        self.end_chain(chain)?;
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
            TokenKind::WordLit(w) => {
                let w = *w;
                self.advance();
                Ok(Expr {
                    kind: ExprKind::WordLit(w),
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
            TokenKind::If => self.parse_open_ended(Self::parse_if_expr),
            TokenKind::Let => self.parse_let_expr(),
            TokenKind::Case => self.parse_open_ended(Self::parse_case_expr),
            TokenKind::Fn => self.parse_open_ended(Self::parse_fn_expr),
            TokenKind::Handle => self.parse_open_ended(Self::parse_handle_expr),
            TokenKind::Perform => self.parse_perform_expr(),
            TokenKind::Resume => self.parse_resume_expr(),
            _ => Err(self.err("expected expression")),
        }
    }

    /// Parse an expression that ends in a sub-expression (`fn ... => e`,
    /// `if ... else e`, ...). That tail takes every operator and argument
    /// it can, so anything an enclosing chain could still see was left by a
    /// tail that stopped early, as in `fn x => x : int (2)` or
    /// `if a then b else c < d < e`. Those are errors, as in SML, not an
    /// application of the `fn` or a comparison of the `if`.
    fn parse_open_ended(
        &mut self,
        parse: fn(&mut Self) -> Result<Expr, ParseError>,
    ) -> Result<Expr, ParseError> {
        let expr = parse(self)?;
        self.closed = true;
        Ok(expr)
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
        self.nested(Self::parse_pat_inner_dispatch)
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
        // Collect `ident as ident as ... as pat` iteratively
        let mut names: Vec<(Symbol, Span)> = Vec::new();
        let mut chain = self.begin_chain(false);
        while matches!(self.peek(), TokenKind::Ident(_))
            && self.pos + 1 < self.tokens.len()
            && matches!(self.tokens[self.pos + 1].kind, TokenKind::As)
        {
            self.chain_link(&mut chain)?;
            let start = self.span();
            let name = self.take_symbol(); // consume ident
            self.advance(); // consume `as`
            names.push((name, start));
        }
        let mut result = self.parse_cons_pat()?;
        self.end_operand(&mut chain);
        self.end_chain(chain)?;
        while let Some((name, start)) = names.pop() {
            let span = start.merge(result.span);
            result = Pat {
                kind: PatKind::As(name, Box::new(result)),
                span,
            };
        }
        Ok(result)
    }

    fn parse_cons_pat(&mut self) -> Result<Pat, ParseError> {
        let mut chain = self.begin_chain(false);
        let mut elems = vec![self.parse_app_pat()?];
        self.end_operand(&mut chain);
        while self.at(&TokenKind::ColonColon) {
            self.chain_link(&mut chain)?;
            self.advance();
            elems.push(self.parse_app_pat()?);
            self.end_operand(&mut chain);
        }
        self.end_chain(chain)?;
        let mut result = elems.pop().unwrap();
        while let Some(lhs) = elems.pop() {
            let span = lhs.span.merge(result.span);
            result = Pat {
                kind: PatKind::Cons(Box::new(lhs), Box::new(result)),
                span,
            };
        }
        Ok(result)
    }

    fn parse_app_pat(&mut self) -> Result<Pat, ParseError> {
        if self.can_start_qualified_path() {
            let (name, is_constructor, span) = self.parse_qualified_symbol()?;
            if !is_constructor {
                return Err(self.err("pattern qualified name must end in a constructor"));
            }
            if self.can_start_atom_pat() {
                let payload = self.parse_atom_pat()?;
                let payload_span = payload.span;
                Ok(Pat {
                    kind: PatKind::Constructor(name, Some(Box::new(payload))),
                    span: span.merge(payload_span),
                })
            } else {
                Ok(Pat {
                    kind: PatKind::Constructor(name, None),
                    span,
                })
            }
        } else if matches!(self.peek(), TokenKind::UpperIdent(_)) {
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
        if self.can_start_qualified_path() {
            let (name, is_constructor, span) = self.parse_qualified_symbol()?;
            if !is_constructor {
                return Err(self.err("pattern qualified name must end in a constructor"));
            }
            return Ok(Pat {
                kind: PatKind::Constructor(name, None),
                span,
            });
        }

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
            TokenKind::WordLit(w) => {
                let w = *w;
                self.advance();
                Ok(Pat {
                    kind: PatKind::WordLit(w),
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
                // Desugared to one `::` per element, so each comma is a link.
                let mut chain = self.begin_chain(false);
                let mut elems = vec![self.parse_pat()?];
                self.end_operand(&mut chain);
                while self.at(&TokenKind::Comma) {
                    self.chain_link(&mut chain)?;
                    self.advance();
                    elems.push(self.parse_pat()?);
                    self.end_operand(&mut chain);
                }
                self.end_chain(chain)?;
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
        self.nested(Self::parse_arrow_type)
    }

    fn parse_arrow_type(&mut self) -> Result<TypeExpr, ParseError> {
        let mut chain = self.begin_chain(false);
        let mut elems = vec![self.parse_tuple_type()?];
        self.end_operand(&mut chain);
        while self.at(&TokenKind::ThinArrow) {
            self.chain_link(&mut chain)?;
            self.advance();
            elems.push(self.parse_tuple_type()?);
            self.end_operand(&mut chain);
        }
        self.end_chain(chain)?;
        let mut result = elems.pop().unwrap();
        while let Some(lhs) = elems.pop() {
            let span = lhs.span.merge(result.span);
            result = TypeExpr {
                kind: TypeExprKind::Arrow(Box::new(lhs), Box::new(result)),
                span,
            };
        }
        Ok(result)
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
        let mut chain = self.begin_chain(true);
        let mut ty = self.parse_atom_type()?;
        self.end_operand(&mut chain);
        while matches!(self.peek(), TokenKind::Ident(_) | TokenKind::UpperIdent(_))
            || self.can_start_qualified_path()
        {
            self.chain_link(&mut chain)?;
            let (name, end) = if self.can_start_qualified_path() {
                let (name, _, span) = self.parse_qualified_symbol()?;
                (name, span)
            } else {
                let end = self.span();
                let name = self.take_symbol();
                (name, end)
            };
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
        self.end_chain(chain)?;
        // A bare `Tuple` here is a `(t1, t2, ...)` argument list that was never
        // applied to a type constructor; `t1 * t2` tuples are built above us.
        if matches!(ty.kind, TypeExprKind::Tuple(_)) {
            return Err(ParseError {
                message: "expected type constructor after type argument list".into(),
                span: ty.span,
            });
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
            _ if self.can_start_qualified_path() => {
                let (name, _, span) = self.parse_qualified_symbol()?;
                Ok(TypeExpr {
                    kind: TypeExprKind::Named(name),
                    span,
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

// ── Nesting check ────────────────────────────────────────────────────
//
// Operator chains and application are parsed in loops, so the parser's
// recursion depth says little about the depth of the tree it builds:
// `1 + 1 + ... + 1` is one parse level but one AST level per `+`. These
// functions walk the finished tree and reject any node deeper than
// `MAX_NESTING_DEPTH`. They stop at the limit, so their own recursion is
// bounded by it.

fn enter(depth: u32, span: Span) -> Result<u32, ParseError> {
    if depth > MAX_NESTING_DEPTH {
        return Err(ParseError {
            message: "nesting limit exceeded".into(),
            span,
        });
    }
    Ok(depth + 1)
}

fn check_decl_nesting(decl: &Decl, depth: u32) -> Result<(), ParseError> {
    let inner = enter(depth, decl.span)?;
    match &decl.kind {
        DeclKind::Val(pat, expr) => {
            check_pat_nesting(pat, inner)?;
            check_expr_nesting(expr, inner)
        }
        DeclKind::ValRec(_, expr) => check_expr_nesting(expr, inner),
        DeclKind::Fun(bindings) => {
            for clause in bindings.iter().flat_map(|binding| &binding.clauses) {
                for pat in &clause.pats {
                    check_pat_nesting(pat, inner)?;
                }
                check_expr_nesting(&clause.body, inner)?;
            }
            Ok(())
        }
        DeclKind::Datatype(dt) => dt
            .constructors
            .iter()
            .filter_map(|con| con.payload.as_ref())
            .try_for_each(|ty| check_type_nesting(ty, inner)),
        DeclKind::TypeAlias(ta) => check_type_nesting(&ta.ty, inner),
        DeclKind::Local(locals, body) => locals
            .iter()
            .chain(body)
            .try_for_each(|decl| check_decl_nesting(decl, inner)),
        DeclKind::Signature(sig) => sig.specs.iter().try_for_each(|spec| match spec {
            SignatureSpec::Val { ty, .. } => check_type_nesting(ty, inner),
            SignatureSpec::Type { .. } => Ok(()),
        }),
        DeclKind::Structure { decls, .. } => decls
            .iter()
            .try_for_each(|decl| check_decl_nesting(decl, inner)),
        DeclKind::ExportVal { ty, .. } => check_type_nesting(ty, inner),
        DeclKind::Effect(_, ty) => ty.iter().try_for_each(|ty| check_type_nesting(ty, inner)),
        DeclKind::Import(_)
        | DeclKind::ImportWithNames(_, _)
        | DeclKind::Use(_)
        | DeclKind::AbstractType(_) => Ok(()),
    }
}

fn check_expr_nesting(expr: &Expr, depth: u32) -> Result<(), ParseError> {
    let inner = enter(depth, expr.span)?;
    match &expr.kind {
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::WordLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::CharLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Unit
        | ExprKind::Var(_)
        | ExprKind::Constructor(_) => Ok(()),
        ExprKind::Tuple(elems) | ExprKind::List(elems) => elems
            .iter()
            .try_for_each(|elem| check_expr_nesting(elem, inner)),
        ExprKind::Cons(lhs, rhs)
        | ExprKind::BinOp(_, lhs, rhs)
        | ExprKind::App(lhs, rhs)
        | ExprKind::Resume(lhs, rhs) => {
            check_expr_nesting(lhs, inner)?;
            check_expr_nesting(rhs, inner)
        }
        ExprKind::UnaryNeg(e) | ExprKind::Not(e) | ExprKind::Paren(e) | ExprKind::Perform(_, e) => {
            check_expr_nesting(e, inner)
        }
        ExprKind::Fn(pat, body) => {
            check_pat_nesting(pat, inner)?;
            check_expr_nesting(body, inner)
        }
        ExprKind::If(cond, then_br, else_br) => {
            check_expr_nesting(cond, inner)?;
            check_expr_nesting(then_br, inner)?;
            check_expr_nesting(else_br, inner)
        }
        ExprKind::Let(decls, body) => {
            for decl in decls {
                check_decl_nesting(decl, inner)?;
            }
            check_expr_nesting(body, inner)
        }
        ExprKind::Case(scrutinee, arms) => {
            check_expr_nesting(scrutinee, inner)?;
            for (pat, body) in arms {
                check_pat_nesting(pat, inner)?;
                check_expr_nesting(body, inner)?;
            }
            Ok(())
        }
        ExprKind::Ann(e, ty) => {
            check_expr_nesting(e, inner)?;
            check_type_nesting(ty, inner)
        }
        ExprKind::Handle {
            body,
            return_body,
            handlers,
            ..
        } => {
            check_expr_nesting(body, inner)?;
            check_expr_nesting(return_body, inner)?;
            handlers
                .iter()
                .try_for_each(|handler| check_expr_nesting(&handler.body, inner))
        }
    }
}

fn check_pat_nesting(pat: &Pat, depth: u32) -> Result<(), ParseError> {
    let inner = enter(depth, pat.span)?;
    match &pat.kind {
        PatKind::Wildcard
        | PatKind::Var(_)
        | PatKind::IntLit(_)
        | PatKind::FloatLit(_)
        | PatKind::WordLit(_)
        | PatKind::StringLit(_)
        | PatKind::CharLit(_)
        | PatKind::BoolLit(_)
        | PatKind::Unit
        | PatKind::Constructor(_, None) => Ok(()),
        PatKind::Tuple(pats) => pats
            .iter()
            .try_for_each(|pat| check_pat_nesting(pat, inner)),
        // Desugared to `p1 :: (p2 :: ... :: [])`: element i sits i levels deeper.
        PatKind::List(pats) => (inner..)
            .zip(pats)
            .try_for_each(|(depth, pat)| check_pat_nesting(pat, depth)),
        PatKind::Cons(hd, tl) => {
            check_pat_nesting(hd, inner)?;
            check_pat_nesting(tl, inner)
        }
        PatKind::Constructor(_, Some(p)) | PatKind::As(_, p) | PatKind::Paren(p) => {
            check_pat_nesting(p, inner)
        }
        PatKind::Ann(p, ty) => {
            check_pat_nesting(p, inner)?;
            check_type_nesting(ty, inner)
        }
    }
}

fn check_type_nesting(ty: &TypeExpr, depth: u32) -> Result<(), ParseError> {
    let inner = enter(depth, ty.span)?;
    match &ty.kind {
        TypeExprKind::Named(_) | TypeExprKind::Var(_) => Ok(()),
        TypeExprKind::App(_, args) | TypeExprKind::Tuple(args) => args
            .iter()
            .try_for_each(|arg| check_type_nesting(arg, inner)),
        TypeExprKind::Arrow(lhs, rhs) => {
            check_type_nesting(lhs, inner)?;
            check_type_nesting(rhs, inner)
        }
        TypeExprKind::Paren(inner_ty) => check_type_nesting(inner_ty, inner),
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
    fn test_fun_clauses_must_agree_on_arity() {
        assert_eq!(
            parse_err("fun f 0 = 1 | f n _ = n"),
            "clauses of 'f' take different numbers of arguments: 1 and 2"
        );
        assert_eq!(
            parse_err("fun f 0 0 = 1 | f n = n"),
            "clauses of 'f' take different numbers of arguments: 2 and 1"
        );
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
        let prog = parse("datatype shape = Circle of float | Rect of float * float");
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
        let prog = parse("type point = float * float");
        assert!(matches!(&prog.decls[0].kind, DeclKind::TypeAlias(_)));
    }

    #[test]
    fn test_use_decl() {
        let prog = parse(r#"use "./foo.hml""#);
        assert!(matches!(&prog.decls[0].kind, DeclKind::Use(path) if path == "./foo.hml"));
    }

    #[test]
    fn test_import_decl() {
        let prog = parse("import Std.List");
        assert!(
            matches!(&prog.decls[0].kind, DeclKind::Import(name) if prog.interner.resolve(*name) == "Std.List")
        );
    }

    #[test]
    fn test_import_decl_accepts_reserved_internal_package() {
        let prog = parse("import __Builtin.Filesystem");
        assert!(
            matches!(&prog.decls[0].kind, DeclKind::Import(name) if prog.interner.resolve(*name) == "__Builtin.Filesystem")
        );
    }

    #[test]
    fn test_import_decl_with_exposed_names() {
        let prog = parse("import Std.Result (Ok, Err, map)");
        match &prog.decls[0].kind {
            DeclKind::ImportWithNames(name, names) => {
                assert_eq!(prog.interner.resolve(*name), "Std.Result");
                let names: Vec<&str> = names.iter().map(|s| prog.interner.resolve(*s)).collect();
                assert_eq!(names, vec!["Ok", "Err", "map"]);
            }
            other => panic!("expected import-with-names, got {other:?}"),
        }
    }

    #[test]
    fn test_import_decl_with_empty_exposure_list_rejected() {
        let err = parse_err("import Std.Result ()");
        assert!(err.contains("cannot be empty"));
    }

    #[test]
    fn test_import_decl_rejects_deep_names() {
        let err = parse_err("import Std.List.Extras");
        assert!(err.contains("exactly two segments"));
    }

    #[test]
    fn test_qualified_constructor_pattern() {
        let prog = parse("val x = case y of Json.JObject fields => fields | _ => []");
        let DeclKind::Val(_, expr) = &prog.decls[0].kind else {
            panic!("expected value declaration");
        };
        let ExprKind::Case(_, branches) = &expr.kind else {
            panic!("expected case expression");
        };
        let (pat, _) = &branches[0];
        match &pat.kind {
            PatKind::Constructor(sym, Some(payload)) => {
                assert_eq!(prog.interner.resolve(*sym), "Json.JObject");
                assert!(matches!(payload.kind, PatKind::Var(_)));
            }
            other => panic!("expected qualified constructor pattern, got {other:?}"),
        }
    }

    #[test]
    fn test_qualified_constructor_expr() {
        let prog = parse("val x = Option.Some 42");
        let DeclKind::Val(_, expr) = &prog.decls[0].kind else {
            panic!("expected value declaration");
        };
        let ExprKind::App(f, arg) = &expr.kind else {
            panic!("expected constructor application");
        };
        match (&f.kind, &arg.kind) {
            (ExprKind::Constructor(sym), ExprKind::IntLit(42)) => {
                assert_eq!(prog.interner.resolve(*sym), "Option.Some");
            }
            other => panic!("expected qualified constructor expr, got {other:?}"),
        }
    }

    #[test]
    fn test_use_decl_requires_explicit_relative_path() {
        let err = parse_err(r#"use "foo.hml""#);
        assert!(err.contains("explicit relative path"));
    }

    #[test]
    fn test_signature_decl() {
        let prog = parse("signature LIST = sig val fold : int -> int end");
        if let DeclKind::Signature(sig) = &prog.decls[0].kind {
            assert_eq!(prog.interner.resolve(sig.name), "LIST");
            assert_eq!(sig.specs.len(), 1);
        } else {
            panic!("expected signature");
        }
    }

    #[test]
    fn test_signature_decl_with_type_spec() {
        let prog = parse("signature BOX = sig type 'a t val empty : 'a t end");
        if let DeclKind::Signature(sig) = &prog.decls[0].kind {
            assert_eq!(prog.interner.resolve(sig.name), "BOX");
            assert_eq!(sig.specs.len(), 2);
            match &sig.specs[0] {
                SignatureSpec::Type { tyvars, name, .. } => {
                    assert_eq!(tyvars.len(), 1);
                    assert_eq!(prog.interner.resolve(*name), "t");
                }
                _ => panic!("expected type spec"),
            }
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
            assert_eq!(
                signature.map(|sym| prog.interner.resolve(sym)),
                Some("LIST")
            );
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
            assert_eq!(
                signature.map(|sym| prog.interner.resolve(sym)),
                Some("LIST")
            );
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
    fn test_qualified_type_expr() {
        let prog = parse("val x = (y : Queue.t)");
        if let DeclKind::Val(_, expr) = &prog.decls[0].kind {
            match &expr.kind {
                ExprKind::Paren(inner) => match &inner.kind {
                    ExprKind::Ann(_, ty) => match &ty.kind {
                        TypeExprKind::Named(sym) => {
                            assert_eq!(prog.interner.resolve(*sym), "Queue.t");
                        }
                        _ => panic!("expected qualified named type"),
                    },
                    _ => panic!("expected annotation inside parens"),
                },
                _ => panic!("expected annotation"),
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
            if let ExprKind::BinOp(BinOp::Add, _, ref rhs) = expr.kind {
                assert!(matches!(&rhs.kind, ExprKind::BinOp(BinOp::Mul, _, _)));
            } else {
                panic!("expected Add at top");
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
        if let DeclKind::Val(ref pat, _) = prog.decls[0].kind
            && let PatKind::Paren(ref inner) = pat.kind
        {
            assert!(matches!(&inner.kind, PatKind::Cons(_, _)));
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
        let prog = parse("val x = (42 : int)");
        assert!(matches!(&prog.decls[0].kind, DeclKind::Val(_, _)));
    }

    #[test]
    fn test_arrow_type() {
        let prog = parse("type f = int -> bool");
        if let DeclKind::TypeAlias(ref ta) = prog.decls[0].kind {
            assert!(matches!(&ta.ty.kind, TypeExprKind::Arrow(_, _)));
        }
    }

    #[test]
    fn test_type_app() {
        let prog = parse("type xs = int list");
        if let DeclKind::TypeAlias(ref ta) = prog.decls[0].kind {
            if let TypeExprKind::App(sym, _) = &ta.ty.kind {
                assert_eq!(prog.interner.resolve(*sym), "list");
            } else {
                panic!("expected App");
            }
        }
    }

    #[test]
    fn test_type_arg_list_requires_constructor() {
        assert_eq!(
            parse_err("val x : (int, string) = (1, \"a\")"),
            "expected type constructor after type argument list"
        );
        assert_eq!(
            parse_err("type t = (int, string) -> int"),
            "expected type constructor after type argument list"
        );
        let prog = parse("type t = (int, string) either * (int * string)");
        let DeclKind::TypeAlias(ref ta) = prog.decls[0].kind else {
            panic!("expected type alias");
        };
        let TypeExprKind::Tuple(elems) = &ta.ty.kind else {
            panic!("expected tuple type");
        };
        assert!(matches!(&elems[0].kind, TypeExprKind::App(_, args) if args.len() == 2));
        assert!(matches!(&elems[1].kind, TypeExprKind::Paren(_)));
    }

    #[test]
    fn test_local_decl() {
        let prog = parse("local val x = 1 in val y = x end");
        assert!(matches!(&prog.decls[0].kind, DeclKind::Local(_, _)));
    }

    #[test]
    fn test_negative_pattern() {
        let prog = parse("fun f ~1 = true | f _ = false");
        if let DeclKind::Fun(ref bindings) = prog.decls[0].kind
            && let PatKind::IntLit(n) = &bindings[0].clauses[0].pats[0].kind
        {
            assert_eq!(*n, -1);
        }
    }

    #[test]
    fn test_as_pattern() {
        let prog = parse("val (x as Some _) = y");
        if let DeclKind::Val(ref pat, _) = prog.decls[0].kind
            && let PatKind::Paren(ref inner) = pat.kind
        {
            if let PatKind::As(sym, _) = &inner.kind {
                assert_eq!(prog.interner.resolve(*sym), "x");
            } else {
                panic!("expected As");
            }
        }
    }

    #[test]
    fn test_float_operators() {
        let prog = parse("val x = 1.0 + 2.0 * 3.0");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::BinOp(BinOp::Add, _, ref rhs) = expr.kind {
                assert!(matches!(&rhs.kind, ExprKind::BinOp(BinOp::Mul, _, _)));
            } else {
                panic!("expected Add at top");
            }
        }
    }

    #[test]
    fn test_word_literal() {
        let prog = parse("val x = 0w42");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            assert!(matches!(&expr.kind, ExprKind::WordLit(42)));
        } else {
            panic!("expected val with WordLit");
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
    fn test_pipe_left_assoc() {
        let prog = parse("val x = a |> b |> c");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::BinOp(BinOp::Pipe, ref lhs, _) = expr.kind {
                assert!(matches!(&lhs.kind, ExprKind::BinOp(BinOp::Pipe, _, _)));
            } else {
                panic!("expected Pipe at top");
            }
        }
    }

    #[test]
    fn test_pipe_has_lower_precedence_than_orelse() {
        let prog = parse("val x = a orelse b |> f");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::BinOp(BinOp::Pipe, ref lhs, _) = expr.kind {
                assert!(matches!(&lhs.kind, ExprKind::BinOp(BinOp::Orelse, _, _)));
            } else {
                panic!("expected Pipe at top");
            }
        }
    }

    #[test]
    fn test_pipe_has_lower_precedence_than_application() {
        let prog = parse("val x = a |> f b");
        if let DeclKind::Val(_, ref expr) = prog.decls[0].kind {
            if let ExprKind::BinOp(BinOp::Pipe, _, ref rhs) = expr.kind {
                assert!(matches!(&rhs.kind, ExprKind::App(_, _)));
            } else {
                panic!("expected Pipe at top");
            }
        }
    }

    #[test]
    fn test_open_ended_forms_are_not_applied() {
        for src in [
            "val f = fn x => x : int (2)",
            "val f = if a then b else c : int 2",
            "val f = case a of _ => b : int [1]",
            "val f = handle a with return x => x : int ~1",
            "val f = fn x => x : int + 1",
            "val f = if a then b else c < d < e",
            "val f = 1 + fn x => x : int - 2",
            "val f = g ~if a then b else c : int 1",
        ] {
            assert_eq!(
                parse_err(src),
                "expected declaration (val, fun, datatype, type, local, import, use, signature, structure, or effect)",
                "{src}"
            );
        }
        parse("val f = (fn x => x : int) (2)");
        parse("val f = let in g end 2");
    }

    #[test]
    fn test_import_span_ends_at_its_last_token() {
        let src = "import Std.List (map, filter)\nval x = 1";
        let prog = parse(src);
        let span = prog.decls[0].span;
        assert_eq!(
            &src[span.start as usize..span.end as usize],
            "import Std.List (map, filter)"
        );
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

    #[test]
    fn test_modules_rejected_below_top_level() {
        let msg = "signature and structure declarations are only allowed at top level";
        for src in [
            "structure S = struct structure T = struct val x = 1 end end",
            "structure S = struct signature T = sig end end",
            "local structure T = struct val x = 1 end in end",
            "local in signature T = sig end end",
            "val z = let structure T = struct val x = 1 end in 1 end",
        ] {
            assert_eq!(parse_err(src), msg, "{src}");
        }
    }

    /// Run `f` on a thread with the 8 MiB stack a main thread gets: reaching
    /// the nesting limit in an unoptimized build needs more than the 2 MiB
    /// default test-thread stack.
    fn with_main_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap();
    }

    /// Exact AST depth, counting the top-level declaration as 1.
    fn ast_depth(prog: &Program) -> u32 {
        fn decl(d: &Decl) -> u32 {
            1 + match &d.kind {
                DeclKind::Val(p, e) => pat(p).max(expr(e)),
                DeclKind::Fun(bs) => bs
                    .iter()
                    .flat_map(|b| &b.clauses)
                    .map(|c| c.pats.iter().map(pat).max().unwrap_or(0).max(expr(&c.body)))
                    .max()
                    .unwrap_or(0),
                DeclKind::TypeAlias(ta) => ty(&ta.ty),
                _ => 0,
            }
        }
        fn expr(e: &Expr) -> u32 {
            1 + match &e.kind {
                ExprKind::Tuple(es) | ExprKind::List(es) => es.iter().map(expr).max().unwrap_or(0),
                ExprKind::Cons(a, b) | ExprKind::BinOp(_, a, b) | ExprKind::App(a, b) => {
                    expr(a).max(expr(b))
                }
                ExprKind::UnaryNeg(a) | ExprKind::Not(a) | ExprKind::Paren(a) => expr(a),
                ExprKind::Fn(p, b) => pat(p).max(expr(b)),
                _ => 0,
            }
        }
        fn pat(p: &Pat) -> u32 {
            1 + match &p.kind {
                PatKind::Tuple(ps) => ps.iter().map(pat).max().unwrap_or(0),
                PatKind::List(ps) => ps
                    .iter()
                    .zip(0..)
                    .map(|(p, i)| pat(p) + i)
                    .max()
                    .unwrap_or(0),
                PatKind::Cons(a, b) => pat(a).max(pat(b)),
                PatKind::As(_, a) | PatKind::Paren(a) | PatKind::Constructor(_, Some(a)) => pat(a),
                _ => 0,
            }
        }
        fn ty(t: &TypeExpr) -> u32 {
            1 + match &t.kind {
                TypeExprKind::App(_, ts) | TypeExprKind::Tuple(ts) => {
                    ts.iter().map(ty).max().unwrap_or(0)
                }
                TypeExprKind::Arrow(a, b) => ty(a).max(ty(b)),
                TypeExprKind::Paren(a) => ty(a),
                _ => 0,
            }
        }
        prog.decls.iter().map(decl).max().unwrap_or(0)
    }

    #[test]
    fn test_ast_depth_limit_is_exact() {
        // Each source has AST depth `extra + n + c` for the given constant.
        // At the limit it must parse; one level more must not, whether the
        // depth comes from parentheses or from an operator chain.
        type Shape = (&'static str, fn(usize, usize) -> String);
        let shapes: [Shape; 8] = [
            ("left chain, deep first operand", |m, k| {
                format!(
                    "val x = {}1{}{}",
                    "(".repeat(k),
                    ")".repeat(k),
                    " + 1".repeat(m)
                )
            }),
            ("right chain, deep last operand", |m, k| {
                format!(
                    "val x = {}{}[]{}",
                    "1 :: ".repeat(m),
                    "(".repeat(k),
                    ")".repeat(k)
                )
            }),
            ("pipe", |m, k| {
                format!(
                    "val x = {}1{}{}",
                    "(".repeat(k),
                    ")".repeat(k),
                    " |> f".repeat(m)
                )
            }),
            ("application", |m, k| {
                format!(
                    "val x = {}f{}{}",
                    "(".repeat(k),
                    ")".repeat(k),
                    " 1".repeat(m)
                )
            }),
            ("orelse", |m, k| {
                format!(
                    "val x = {}{}b{}",
                    "b orelse ".repeat(m),
                    "(".repeat(k),
                    ")".repeat(k)
                )
            }),
            ("as pattern", |m, k| {
                let names: String = (0..m).map(|i| format!("x{i} as ")).collect();
                format!("val {names}{}y{} = 1", "(".repeat(k), ")".repeat(k))
            }),
            ("list pattern", |m, k| {
                format!(
                    "val [{}{}y{}] = 1",
                    "_, ".repeat(m),
                    "(".repeat(k),
                    ")".repeat(k)
                )
            }),
            ("type application", |m, k| {
                format!(
                    "type t = {}int{}{}",
                    "(".repeat(k),
                    ")".repeat(k),
                    " list".repeat(m)
                )
            }),
        ];
        with_main_stack(move || {
            for (name, shape) in shapes {
                for m in [0, 1, 100, 250] {
                    // Find the deepest `k` that parses and check its depth.
                    let k = (0..300)
                        .take_while(|&k| {
                            let src = shape(m, k);
                            let tokens = Lexer::new(&src, 0).tokenize().expect("lex error");
                            Parser::new(tokens).parse_program().is_ok()
                        })
                        .last()
                        .unwrap_or_else(|| panic!("{name}, m={m}: nothing parses"));
                    let depth = ast_depth(&parse(&shape(m, k)));
                    assert_eq!(depth, MAX_NESTING_DEPTH, "{name}, m={m}, k={k}");
                    assert_eq!(parse_err(&shape(m, k + 1)), "nesting limit exceeded");
                }
            }
        });
    }

    #[test]
    fn test_deep_nesting_is_a_parse_error() {
        with_main_stack(|| {
            let n = 100_000;
            for src in [
                format!("val x = {}1", "~ ".repeat(n)),
                format!("val x = {}true", "not ".repeat(n)),
                format!("{}{}", "local ".repeat(n), "in end ".repeat(n)),
                format!("val x = {}1{}", "(".repeat(n), ")".repeat(n)),
                // Chains are parsed in a loop but nest the AST.
                format!("val x = 1{}", " + 1".repeat(n)),
                format!("val x = {}[]", "1 :: ".repeat(n)),
                format!("val x = 1{}", " |> f".repeat(n)),
                format!("val x = \"\"{}", " ^ \"\"".repeat(n)),
                format!("val x = f{}", " 1".repeat(n)),
                format!("val x = {}true", "true andalso ".repeat(n)),
                format!("val {}y = 1", "x as ".repeat(n)),
                format!("val ({}[]) = 1", "x :: ".repeat(n)),
                format!("val [{}y] = 1", "x, ".repeat(n)),
                format!("type t = int{}", " -> int".repeat(n)),
                format!("type t = int{}", " list".repeat(n)),
                // Each chain is under the limit, but nesting them is not;
                // this must fail without building (and dropping) a tree
                // tens of thousands of levels deep.
                format!(
                    "val x = {}1{}",
                    "(".repeat(250),
                    format!("{})", " + 1".repeat(250)).repeat(250)
                ),
                format!(
                    "val x = {}[]{}",
                    format!("{}(", "1 :: ".repeat(250)).repeat(250),
                    ")".repeat(250)
                ),
            ] {
                assert_eq!(parse_err(&src), "nesting limit exceeded");
            }
            parse(&format!("val x = {}1", "~ ".repeat(100)));
            parse(&format!(
                "{}{}",
                "local ".repeat(100),
                "in end ".repeat(100)
            ));
        });
    }
}
