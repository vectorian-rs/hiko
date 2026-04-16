use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    CharLit(char),

    // Identifiers
    Ident(String),      // lowercase-initial
    UpperIdent(String), // uppercase-initial (constructors)
    TyVar(String),      // 'a, 'b, etc.

    // Keywords
    Val,
    Fun,
    Fn,
    Let,
    In,
    End,
    If,
    Then,
    Else,
    Case,
    Of,
    Datatype,
    Type,
    Local,
    And,
    Rec,
    Use,
    Structure,
    Struct,
    True,
    False,
    Not,
    Andalso,
    Orelse,
    Mod,
    As,
    Effect,
    Handle,
    With,
    Perform,
    Resume,
    Return,

    // Operators (Int arithmetic)
    Plus,  // +
    Minus, // -
    Star,  // *
    Slash, // /

    // Operators (Float arithmetic)
    PlusDot,  // +.
    MinusDot, // -.
    StarDot,  // *.
    SlashDot, // /.

    // Operators (String)
    Caret, // ^

    // Operators (Int comparison)
    Lt, // <
    Gt, // >
    Le, // <=
    Ge, // >=

    // Operators (Float comparison)
    LtDot, // <.
    GtDot, // >.
    LeDot, // <=.
    GeDot, // >=.

    // Operators (equality, scalar only)
    Eq, // =
    Ne, // <>

    // Other operators
    Tilde,      // ~ (unary negation)
    ColonColon, // ::
    Arrow,      // =>
    ThinArrow,  // ->
    Bar,        // |

    // Delimiters
    LParen,     // (
    RParen,     // )
    LBracket,   // [
    RBracket,   // ]
    Comma,      // ,
    Dot,        // .
    Colon,      // :
    Semicolon,  // ;
    Underscore, // _
    Hash,       // #

    // Special
    Eof,
}

impl TokenKind {
    pub fn keyword_from_str(s: &str) -> Option<TokenKind> {
        match s {
            "val" => Some(TokenKind::Val),
            "fun" => Some(TokenKind::Fun),
            "fn" => Some(TokenKind::Fn),
            "let" => Some(TokenKind::Let),
            "in" => Some(TokenKind::In),
            "end" => Some(TokenKind::End),
            "if" => Some(TokenKind::If),
            "then" => Some(TokenKind::Then),
            "else" => Some(TokenKind::Else),
            "case" => Some(TokenKind::Case),
            "of" => Some(TokenKind::Of),
            "datatype" => Some(TokenKind::Datatype),
            "type" => Some(TokenKind::Type),
            "local" => Some(TokenKind::Local),
            "and" => Some(TokenKind::And),
            "rec" => Some(TokenKind::Rec),
            "use" => Some(TokenKind::Use),
            "structure" => Some(TokenKind::Structure),
            "struct" => Some(TokenKind::Struct),
            "true" => Some(TokenKind::True),
            "false" => Some(TokenKind::False),
            "not" => Some(TokenKind::Not),
            "andalso" => Some(TokenKind::Andalso),
            "orelse" => Some(TokenKind::Orelse),
            "mod" => Some(TokenKind::Mod),
            "as" => Some(TokenKind::As),
            "effect" => Some(TokenKind::Effect),
            "handle" => Some(TokenKind::Handle),
            "with" => Some(TokenKind::With),
            "perform" => Some(TokenKind::Perform),
            "resume" => Some(TokenKind::Resume),
            "return" => Some(TokenKind::Return),
            _ => None,
        }
    }
}
