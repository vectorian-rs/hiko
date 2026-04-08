use crate::span::Span;

// ── Expressions ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    CharLit(char),
    BoolLit(bool),
    Unit,
    Var(String),
    Constructor(String),
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    Cons(Box<Expr>, Box<Expr>),
    BinOp(BinOp, Box<Expr>, Box<Expr>),
    UnaryNeg(Box<Expr>),
    Not(Box<Expr>),
    App(Box<Expr>, Box<Expr>),
    Fn(Pat, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Let(Vec<Decl>, Box<Expr>),
    Case(Box<Expr>, Vec<(Pat, Expr)>),
    Ann(Box<Expr>, TypeExpr),
    Paren(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    // Int arithmetic
    AddInt,
    SubInt,
    MulInt,
    DivInt,
    ModInt,
    // Float arithmetic
    AddFloat,
    SubFloat,
    MulFloat,
    DivFloat,
    // String
    ConcatStr,
    // Int comparison
    LtInt,
    GtInt,
    LeInt,
    GeInt,
    // Float comparison
    LtFloat,
    GtFloat,
    LeFloat,
    GeFloat,
    // Equality (scalar only)
    Eq,
    Ne,
    // Short-circuit boolean
    Andalso,
    Orelse,
}

// ── Declarations ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub kind: DeclKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclKind {
    /// `val p = e`
    Val(Pat, Expr),
    /// `val rec f = fn x => e`
    ValRec(String, Expr),
    /// `fun f x1 x2 ... = e` (simple, possibly mutual via `and`)
    Fun(Vec<FunBinding>),
    /// `datatype ('a, 'b) T = C1 of t | C2 | ...`
    Datatype(DatatypeDecl),
    /// `type ('a, 'b) T = t`
    TypeAlias(TypeAliasDecl),
    /// `local d1 in d2 end`
    Local(Vec<Decl>, Vec<Decl>),
    /// `use "path/to/file.hk"`
    Use(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunBinding {
    pub name: String,
    pub clauses: Vec<FunClause>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunClause {
    pub pats: Vec<Pat>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DatatypeDecl {
    pub tyvars: Vec<String>,
    pub name: String,
    pub constructors: Vec<ConDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConDecl {
    pub name: String,
    pub payload: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasDecl {
    pub tyvars: Vec<String>,
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

// ── Patterns ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Pat {
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatKind {
    Wildcard,
    Var(String),
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    CharLit(char),
    BoolLit(bool),
    Unit,
    Tuple(Vec<Pat>),
    Constructor(String, Option<Box<Pat>>),
    Cons(Box<Pat>, Box<Pat>),
    List(Vec<Pat>),
    Ann(Box<Pat>, TypeExpr),
    As(String, Box<Pat>),
    Paren(Box<Pat>),
}

// ── Type expressions (surface syntax) ────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExprKind {
    /// A named type: `Int`, `Bool`, `option`, etc.
    Named(String),
    /// A type variable: `'a`
    Var(String),
    /// Type application: `'a list`, `('a, 'b) either`
    App(String, Vec<TypeExpr>),
    /// Arrow: `a -> b`
    Arrow(Box<TypeExpr>, Box<TypeExpr>),
    /// Tuple: `a * b * c`
    Tuple(Vec<TypeExpr>),
    /// Parenthesized
    Paren(Box<TypeExpr>),
}

// ── Program ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub decls: Vec<Decl>,
}
