use crate::intern::Symbol;
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
    Var(Symbol),
    Constructor(Symbol),
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
    /// `perform EffectName arg`
    Perform(Symbol, Box<Expr>),
    /// `handle body with return x => e | Effect x k => e`
    Handle {
        body: Box<Expr>,
        return_var: Symbol,
        return_body: Box<Expr>,
        handlers: Vec<EffectHandler>,
    },
    /// `resume k value`
    Resume(Box<Expr>, Box<Expr>),
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
    ValRec(Symbol, Expr),
    /// `fun f x1 x2 ... = e` (simple, possibly mutual via `and`)
    Fun(Vec<FunBinding>),
    /// `datatype ('a, 'b) T = C1 of t | C2 | ...`
    Datatype(DatatypeDecl),
    /// `type ('a, 'b) T = t`
    TypeAlias(TypeAliasDecl),
    /// `local d1 in d2 end`
    Local(Vec<Decl>, Vec<Decl>),
    /// `use "path/to/file.hml"`
    Use(String),
    /// `structure Name = struct ... end`
    Structure(Symbol, Vec<Decl>),
    /// `effect Yield of Int`
    Effect(Symbol, Option<TypeExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunBinding {
    pub name: Symbol,
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
    pub tyvars: Vec<Symbol>,
    pub name: Symbol,
    pub constructors: Vec<ConDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConDecl {
    pub name: Symbol,
    pub payload: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasDecl {
    pub tyvars: Vec<Symbol>,
    pub name: Symbol,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectHandler {
    pub effect_name: Symbol,
    pub payload_var: Symbol,
    pub cont_var: Symbol,
    pub body: Expr,
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
    Var(Symbol),
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    CharLit(char),
    BoolLit(bool),
    Unit,
    Tuple(Vec<Pat>),
    Constructor(Symbol, Option<Box<Pat>>),
    Cons(Box<Pat>, Box<Pat>),
    List(Vec<Pat>),
    Ann(Box<Pat>, TypeExpr),
    As(Symbol, Box<Pat>),
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
    Named(Symbol),
    /// A type variable: `'a`
    Var(Symbol),
    /// Type application: `'a list`, `('a, 'b) either`
    App(Symbol, Vec<TypeExpr>),
    /// Arrow: `a -> b`
    Arrow(Box<TypeExpr>, Box<TypeExpr>),
    /// Tuple: `a * b * c`
    Tuple(Vec<TypeExpr>),
    /// Parenthesized
    Paren(Box<TypeExpr>),
}

// ── Program ──────────────────────────────────────────────────────────

pub struct Program {
    pub decls: Vec<Decl>,
    pub interner: crate::intern::StringInterner,
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Program")
            .field("decls", &self.decls)
            .finish()
    }
}
