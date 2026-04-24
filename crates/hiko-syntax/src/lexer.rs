use crate::span::Span;
use crate::token::{Token, TokenKind};

#[derive(Debug, Clone)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub struct Lexer<'src> {
    source: &'src str,
    bytes: &'src [u8],
    pos: usize,
    file_id: u32,
}

impl<'src> Lexer<'src> {
    pub fn new(source: &'src str, file_id: u32) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: 0,
            file_id,
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::with_capacity(self.bytes.len() / 4);
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace_and_comments()?;

        if self.pos >= self.bytes.len() {
            return Ok(self.make_token(TokenKind::Eof, self.pos, self.pos));
        }

        let start = self.pos;
        let ch = self.bytes[start];

        // String literal
        if ch == b'"' {
            return self.lex_string();
        }

        // Char literal: #"c"
        if ch == b'#' && self.peek_at(1) == Some(b'"') {
            return self.lex_char();
        }

        // Number (int or float)
        if ch.is_ascii_digit() {
            return self.lex_number();
        }

        // Type variable: 'a, 'b, etc.
        if ch == b'\''
            && self
                .peek_at(1)
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        {
            return self.lex_tyvar();
        }

        // Identifier or keyword
        if ch.is_ascii_alphabetic() || ch == b'_' {
            return self.lex_ident();
        }

        // Operators and delimiters
        self.lex_operator_or_delimiter()
    }

    fn lex_string(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        self.pos += 1; // skip opening "
        let mut value = String::new();

        loop {
            if self.pos >= self.bytes.len() {
                return Err(self.err("unterminated string literal", start));
            }
            match self.bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Ok(self.make_token(TokenKind::StringLit(value), start, self.pos));
                }
                b'\\' => {
                    value.push(self.parse_escape(start)?);
                }
                _ => {
                    let rest = &self.source[self.pos..];
                    if let Some(c) = rest.chars().next() {
                        value.push(c);
                        self.pos += c.len_utf8();
                    }
                }
            }
        }
    }

    fn lex_char(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        self.pos += 2; // skip #"

        if self.pos >= self.bytes.len() {
            return Err(self.err("unterminated character literal", start));
        }

        let c = if self.bytes[self.pos] == b'\\' {
            self.parse_escape(start)?
        } else {
            let rest = &self.source[self.pos..];
            let c = rest
                .chars()
                .next()
                .ok_or_else(|| self.err("empty character literal", start))?;
            self.pos += c.len_utf8();
            c
        };

        if self.pos >= self.bytes.len() || self.bytes[self.pos] != b'"' {
            return Err(self.err("unterminated character literal, expected closing \"", start));
        }
        self.pos += 1; // skip closing "

        Ok(self.make_token(TokenKind::CharLit(c), start, self.pos))
    }

    fn parse_escape(&mut self, literal_start: usize) -> Result<char, LexError> {
        self.pos += 1; // skip backslash
        if self.pos >= self.bytes.len() {
            return Err(self.err("unterminated escape sequence", literal_start));
        }
        let c = match self.bytes[self.pos] {
            b'n' => '\n',
            b't' => '\t',
            b'r' => '\r',
            b'0' => '\0',
            b'\\' => '\\',
            b'"' => '"',
            b'x' => {
                // \xHH: two hex digits
                if self.pos + 2 >= self.bytes.len() {
                    return Err(self.err("incomplete \\x escape", literal_start));
                }
                let hi = self.bytes[self.pos + 1];
                let lo = self.bytes[self.pos + 2];
                let val = hex_digit(hi)
                    .and_then(|h| hex_digit(lo).map(|l| h * 16 + l))
                    .ok_or_else(|| self.err("invalid hex digit in \\x escape", literal_start))?;
                self.pos += 2; // skip the two hex digits (main +1 below)
                val as char
            }
            other => {
                return Err(LexError {
                    message: format!("unknown escape sequence: \\{}", other as char),
                    span: self.span(self.pos - 1, self.pos + 1),
                });
            }
        };
        self.pos += 1;
        Ok(c)
    }

    fn lex_number(&mut self) -> Result<Token, LexError> {
        let start = self.pos;

        // Check for word literal: 0w... or 0wx...
        if self.bytes[start] == b'0' && self.peek_at(1) == Some(b'w') {
            self.pos += 2; // skip '0w'
            if self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b'x' | b'X') {
                // Hex word literal: 0wxFF
                self.pos += 1; // skip 'x'/'X'
                let hex_start = self.pos;
                while self.pos < self.bytes.len() && hex_digit(self.bytes[self.pos]).is_some() {
                    self.pos += 1;
                }
                if self.pos == hex_start {
                    return Err(self.err("expected hex digits after 0wx", start));
                }
                let hex_text = &self.source[hex_start..self.pos];
                let value = u64::from_str_radix(hex_text, 16).map_err(|_| {
                    self.err(&format!("word literal overflow: 0wx{hex_text}"), start)
                })?;
                return Ok(self.make_token(TokenKind::WordLit(value), start, self.pos));
            } else {
                // Decimal word literal: 0w255
                let dec_start = self.pos;
                self.consume_digits();
                if self.pos == dec_start {
                    return Err(self.err("expected digits after 0w", start));
                }
                let dec_text = &self.source[dec_start..self.pos];
                let value: u64 = dec_text.parse().map_err(|_| {
                    self.err(&format!("word literal overflow: 0w{dec_text}"), start)
                })?;
                return Ok(self.make_token(TokenKind::WordLit(value), start, self.pos));
            }
        }

        self.consume_digits();

        let mut is_float = false;

        // Check for decimal point followed by digit
        if self.pos < self.bytes.len()
            && self.bytes[self.pos] == b'.'
            && self
                .bytes
                .get(self.pos + 1)
                .is_some_and(|c| c.is_ascii_digit())
        {
            is_float = true;
            self.pos += 1; // skip '.'
            self.consume_digits();
        }

        // Check for exponent
        if self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b'e' | b'E') {
            is_float = true;
            self.pos += 1;
            if self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b'+' | b'-') {
                self.pos += 1;
            }
            if self.pos >= self.bytes.len() || !self.bytes[self.pos].is_ascii_digit() {
                return Err(self.err("expected digits after exponent", start));
            }
            self.consume_digits();
        }

        let text = &self.source[start..self.pos];
        if is_float {
            let value: f64 = text
                .parse()
                .map_err(|_| self.err(&format!("invalid float literal: {text}"), start))?;
            Ok(self.make_token(TokenKind::FloatLit(value), start, self.pos))
        } else {
            let value: i64 = text
                .parse()
                .map_err(|_| self.err(&format!("invalid integer literal: {text}"), start))?;
            Ok(self.make_token(TokenKind::IntLit(value), start, self.pos))
        }
    }

    fn lex_tyvar(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        self.pos += 1; // skip '
        self.consume_ident_chars();
        let name = self.source[start..self.pos].to_string();
        Ok(self.make_token(TokenKind::TyVar(name), start, self.pos))
    }

    fn lex_ident(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        self.consume_ident_chars();
        let text = &self.source[start..self.pos];

        if text == "_" {
            return Ok(self.make_token(TokenKind::Underscore, start, self.pos));
        }

        if let Some(kw) = TokenKind::keyword_from_str(text) {
            return Ok(self.make_token(kw, start, self.pos));
        }

        let first = text.as_bytes()[0];
        if first.is_ascii_uppercase() {
            Ok(self.make_token(TokenKind::UpperIdent(text.to_string()), start, self.pos))
        } else {
            Ok(self.make_token(TokenKind::Ident(text.to_string()), start, self.pos))
        }
    }

    fn lex_operator_or_delimiter(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        let ch = self.bytes[start];

        let kind = match ch {
            b'(' => {
                self.pos += 1;
                TokenKind::LParen
            }
            b')' => {
                self.pos += 1;
                TokenKind::RParen
            }
            b'[' => {
                self.pos += 1;
                TokenKind::LBracket
            }
            b']' => {
                self.pos += 1;
                TokenKind::RBracket
            }
            b',' => {
                self.pos += 1;
                TokenKind::Comma
            }
            b'.' => {
                self.pos += 1;
                TokenKind::Dot
            }
            b';' => {
                self.pos += 1;
                TokenKind::Semicolon
            }
            b'~' => {
                self.pos += 1;
                TokenKind::Tilde
            }
            b'#' => {
                self.pos += 1;
                TokenKind::Hash
            }
            b'^' => {
                self.pos += 1;
                TokenKind::Caret
            }
            b'|' => {
                self.pos += 1;
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                    TokenKind::PipeGt
                } else {
                    TokenKind::Bar
                }
            }

            b':' => {
                self.pos += 1;
                if self.peek() == Some(b':') {
                    self.pos += 1;
                    TokenKind::ColonColon
                } else if self.peek() == Some(b'>') {
                    self.pos += 1;
                    TokenKind::ColonGt
                } else {
                    TokenKind::Colon
                }
            }

            b'=' => {
                self.pos += 1;
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                    TokenKind::Arrow
                } else {
                    TokenKind::Eq
                }
            }

            b'-' => {
                self.pos += 1;
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                    TokenKind::ThinArrow
                } else {
                    TokenKind::Minus
                }
            }

            b'+' => {
                self.pos += 1;
                TokenKind::Plus
            }

            b'*' => {
                self.pos += 1;
                TokenKind::Star
            }

            b'/' => {
                self.pos += 1;
                TokenKind::Slash
            }

            b'<' => {
                self.pos += 1;
                match self.peek() {
                    Some(b'>') => {
                        self.pos += 1;
                        TokenKind::Ne
                    }
                    Some(b'=') => {
                        self.pos += 1;
                        TokenKind::Le
                    }
                    _ => TokenKind::Lt,
                }
            }

            b'>' => {
                self.pos += 1;
                match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        TokenKind::Ge
                    }
                    _ => TokenKind::Gt,
                }
            }

            _ => {
                self.pos += 1;
                return Err(LexError {
                    message: format!("unexpected character: '{}'", ch as char),
                    span: self.span(start, self.pos),
                });
            }
        };

        Ok(self.make_token(kind, start, self.pos))
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<(), LexError> {
        loop {
            // Skip whitespace
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }

            // Skip nested comments (* ... *)
            if self.pos + 1 < self.bytes.len()
                && self.bytes[self.pos] == b'('
                && self.bytes[self.pos + 1] == b'*'
            {
                self.skip_comment()?;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn skip_comment(&mut self) -> Result<(), LexError> {
        let start = self.pos;
        self.pos += 2; // skip (*
        let mut depth = 1u32;

        while self.pos < self.bytes.len() && depth > 0 {
            if self.pos + 1 < self.bytes.len()
                && self.bytes[self.pos] == b'('
                && self.bytes[self.pos + 1] == b'*'
            {
                depth += 1;
                self.pos += 2;
            } else if self.pos + 1 < self.bytes.len()
                && self.bytes[self.pos] == b'*'
                && self.bytes[self.pos + 1] == b')'
            {
                depth -= 1;
                self.pos += 2;
            } else {
                self.pos += 1;
            }
        }

        if depth > 0 {
            return Err(self.err("unterminated comment", start));
        }
        Ok(())
    }

    fn consume_digits(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
    }

    fn consume_ident_chars(&mut self) {
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_alphanumeric() || self.bytes[self.pos] == b'_')
        {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.file_id, start as u32, end as u32)
    }

    fn err(&self, message: &str, start: usize) -> LexError {
        LexError {
            message: message.to_string(),
            span: self.span(start, self.pos),
        }
    }

    fn make_token(&self, kind: TokenKind, start: usize, end: usize) -> Token {
        Token {
            kind,
            span: self.span(start, end),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(input: &str) -> Vec<TokenKind> {
        let mut lexer = Lexer::new(input, 0);
        lexer
            .tokenize()
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    fn lex_err(input: &str) -> String {
        let mut lexer = Lexer::new(input, 0);
        lexer.tokenize().unwrap_err().message
    }

    #[test]
    fn test_int_literals() {
        assert_eq!(lex("42"), vec![TokenKind::IntLit(42), TokenKind::Eof]);
        assert_eq!(lex("0"), vec![TokenKind::IntLit(0), TokenKind::Eof]);
        assert_eq!(lex("12345"), vec![TokenKind::IntLit(12345), TokenKind::Eof]);
    }

    #[test]
    fn test_float_literals() {
        assert_eq!(
            lex("3.14"),
            vec![TokenKind::FloatLit(314.0 / 100.0), TokenKind::Eof]
        );
        assert_eq!(
            lex("1.0e10"),
            vec![TokenKind::FloatLit(1.0e10), TokenKind::Eof]
        );
        assert_eq!(
            lex("2.5E-3"),
            vec![TokenKind::FloatLit(2.5e-3), TokenKind::Eof]
        );
        assert_eq!(lex("0.0"), vec![TokenKind::FloatLit(0.0), TokenKind::Eof]);
    }

    #[test]
    fn test_string_literals() {
        assert_eq!(
            lex(r#""hello""#),
            vec![TokenKind::StringLit("hello".to_string()), TokenKind::Eof]
        );
        assert_eq!(
            lex(r#""a\nb""#),
            vec![TokenKind::StringLit("a\nb".to_string()), TokenKind::Eof]
        );
        assert_eq!(
            lex(r#""a\\b""#),
            vec![TokenKind::StringLit("a\\b".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn test_char_literals() {
        assert_eq!(
            lex(r#"#"a""#),
            vec![TokenKind::CharLit('a'), TokenKind::Eof]
        );
        assert_eq!(
            lex(r#"#"\n""#),
            vec![TokenKind::CharLit('\n'), TokenKind::Eof]
        );
    }

    #[test]
    fn test_keywords() {
        assert_eq!(lex("val"), vec![TokenKind::Val, TokenKind::Eof]);
        assert_eq!(lex("fun"), vec![TokenKind::Fun, TokenKind::Eof]);
        assert_eq!(lex("fn"), vec![TokenKind::Fn, TokenKind::Eof]);
        assert_eq!(lex("let"), vec![TokenKind::Let, TokenKind::Eof]);
        assert_eq!(lex("case"), vec![TokenKind::Case, TokenKind::Eof]);
        assert_eq!(lex("datatype"), vec![TokenKind::Datatype, TokenKind::Eof]);
        assert_eq!(lex("andalso"), vec![TokenKind::Andalso, TokenKind::Eof]);
        assert_eq!(lex("orelse"), vec![TokenKind::Orelse, TokenKind::Eof]);
    }

    #[test]
    fn test_identifiers() {
        assert_eq!(
            lex("foo"),
            vec![TokenKind::Ident("foo".to_string()), TokenKind::Eof]
        );
        assert_eq!(
            lex("x1"),
            vec![TokenKind::Ident("x1".to_string()), TokenKind::Eof]
        );
        assert_eq!(
            lex("_bar"),
            vec![TokenKind::Ident("_bar".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn test_upper_idents() {
        assert_eq!(
            lex("Some"),
            vec![TokenKind::UpperIdent("Some".to_string()), TokenKind::Eof]
        );
        assert_eq!(
            lex("None"),
            vec![TokenKind::UpperIdent("None".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn test_tyvars() {
        assert_eq!(
            lex("'a"),
            vec![TokenKind::TyVar("'a".to_string()), TokenKind::Eof]
        );
        assert_eq!(
            lex("'abc"),
            vec![TokenKind::TyVar("'abc".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn test_operators() {
        assert_eq!(
            lex("+ - * /"),
            vec![
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_comparison_operators() {
        assert_eq!(
            lex("< <= > >="),
            vec![
                TokenKind::Lt,
                TokenKind::Le,
                TokenKind::Gt,
                TokenKind::Ge,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_word_literals() {
        assert_eq!(lex("0w42"), vec![TokenKind::WordLit(42), TokenKind::Eof]);
        assert_eq!(lex("0wxFF"), vec![TokenKind::WordLit(0xFF), TokenKind::Eof]);
        assert_eq!(
            lex("0wxDEADBEEF"),
            vec![TokenKind::WordLit(0xDEADBEEF), TokenKind::Eof]
        );
        assert_eq!(lex("0w0"), vec![TokenKind::WordLit(0), TokenKind::Eof]);
    }

    #[test]
    fn test_equality_operators() {
        assert_eq!(
            lex("= <>"),
            vec![TokenKind::Eq, TokenKind::Ne, TokenKind::Eof]
        );
    }

    #[test]
    fn test_arrows_and_cons() {
        assert_eq!(
            lex("=> -> :: |>"),
            vec![
                TokenKind::Arrow,
                TokenKind::ThinArrow,
                TokenKind::ColonColon,
                TokenKind::PipeGt,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_delimiters() {
        assert_eq!(
            lex("( ) [ ] , : ; | _ #"),
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Comma,
                TokenKind::Colon,
                TokenKind::Semicolon,
                TokenKind::Bar,
                TokenKind::Underscore,
                TokenKind::Hash,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_comments() {
        assert_eq!(
            lex("(* comment *) 42"),
            vec![TokenKind::IntLit(42), TokenKind::Eof]
        );
    }

    #[test]
    fn test_nested_comments() {
        assert_eq!(
            lex("(* outer (* inner *) still outer *) 1"),
            vec![TokenKind::IntLit(1), TokenKind::Eof],
        );
    }

    #[test]
    fn test_unterminated_comment() {
        assert_eq!(lex_err("(* oops"), "unterminated comment");
    }

    #[test]
    fn test_unterminated_string() {
        assert_eq!(lex_err(r#""oops"#), "unterminated string literal");
    }

    #[test]
    fn test_unexpected_char() {
        assert_eq!(lex_err("@"), "unexpected character: '@'");
    }

    #[test]
    fn test_negation_is_operator() {
        assert_eq!(
            lex("~42"),
            vec![TokenKind::Tilde, TokenKind::IntLit(42), TokenKind::Eof,]
        );
    }

    #[test]
    fn test_full_expression() {
        let tokens = lex("fun fib n = if n < 2 then n else fib (n - 1) + fib (n - 2)");
        assert_eq!(tokens[0], TokenKind::Fun);
        assert_eq!(tokens[1], TokenKind::Ident("fib".to_string()));
        assert_eq!(tokens[2], TokenKind::Ident("n".to_string()));
        assert_eq!(tokens[3], TokenKind::Eq);
        assert_eq!(tokens[4], TokenKind::If);
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(lex(""), vec![TokenKind::Eof]);
    }

    #[test]
    fn test_comments_only() {
        assert_eq!(lex("(* just a comment *)"), vec![TokenKind::Eof]);
    }

    #[test]
    fn test_datatype_declaration() {
        let tokens = lex("datatype 'a option = None | Some of 'a");
        assert_eq!(tokens[0], TokenKind::Datatype);
        assert_eq!(tokens[1], TokenKind::TyVar("'a".to_string()));
        assert_eq!(tokens[2], TokenKind::Ident("option".to_string()));
        assert_eq!(tokens[3], TokenKind::Eq);
        assert_eq!(tokens[4], TokenKind::UpperIdent("None".to_string()));
        assert_eq!(tokens[5], TokenKind::Bar);
        assert_eq!(tokens[6], TokenKind::UpperIdent("Some".to_string()));
        assert_eq!(tokens[7], TokenKind::Of);
        assert_eq!(tokens[8], TokenKind::TyVar("'a".to_string()));
    }
}
