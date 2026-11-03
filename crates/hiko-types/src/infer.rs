use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use hiko_syntax::ast::*;
use hiko_syntax::intern::StringInterner;
use hiko_syntax::span::Span;

use crate::exhaustive::{self, TypeInfo};
use crate::ty::{Scheme, Type};

// ── Errors ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

// ── Warnings ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Warning {
    pub message: String,
    pub span: Span,
}

// ── Inference context ────────────────────────────────────────────────

pub struct InferCtx {
    subst: HashMap<u32, Type>,
    next_var: u32,
    scopes: Vec<HashMap<String, Scheme>>,
    /// The level of each type variable, indexed by its id: the let-nesting
    /// depth of the outermost binding whose type may mention it.
    /// Generalization takes the variables above the current level (Rémy's
    /// levels, as in OCaml), so it never walks the environment.
    levels: Vec<u32>,
    /// The current level: [`TOP_LEVEL`] between top-level declarations, one
    /// more inside each let-bound right-hand side.
    level: u32,
    constructors: HashMap<String, Scheme>,
    type_arities: HashMap<String, usize>,
    /// Opaque types declared by extensions (see `add_opaque_type`).
    opaque_types: Vec<String>,
    /// Name -> (parameters, the row variable of the body's arrows, body).
    type_aliases: HashMap<String, (Vec<u32>, Option<u32>, Type)>,
    /// Abstract type introduced by signature ascription -> implementation type name
    abstract_impls: HashMap<String, String>,
    /// Constructor name → tag (for exhaustiveness checking)
    constructor_tags: HashMap<String, u16>,
    /// Import alias -> constructor name it stands for (for exhaustiveness)
    constructor_aliases: HashMap<String, String>,
    /// Type name → list of (constructor_name, arity) (for exhaustiveness)
    datatype_constructors: HashMap<String, Vec<(String, usize)>>,
    /// Accumulated warnings (redundant clauses, etc.)
    pub warnings: Vec<Warning>,
    /// Effect name -> (argument_type, result_type)
    effect_sigs: HashMap<String, (Type, Type)>,
    /// Resolved types for expressions (keyed by span, for typed codegen)
    pub expr_types: HashMap<hiko_syntax::span::Span, Type>,
    /// String interner for resolving symbols in the AST.
    pub interner: StringInterner,
    /// Type nodes visited so far. See [`DEFAULT_MAX_TYPE_WORK`].
    work: Cell<u64>,
    max_work: u64,
}

/// Default for the number of type nodes inference may visit while checking
/// one compilation. Type sizes can grow exponentially in program size
/// (`val x1 = (x0, x0) val x2 = (x1, x1) ...`), so without a bound a small
/// source file can stall the compiler.
pub const DEFAULT_MAX_TYPE_WORK: u64 = 10_000_000;

/// The level of type variables that are never generalized: an effect's
/// result type and the row variables of its payload, shared by every
/// `perform` and handler of the effect.
const NEVER_GENERALIZED: u32 = 0;
/// The level between top-level declarations.
const TOP_LEVEL: u32 = 1;

/// What one type annotation has bound so far: its type variables by name,
/// and the row variable its unmarked arrows share.
#[derive(Default)]
struct Annotation {
    tyvars: HashMap<String, u32>,
    row: Option<u32>,
}

impl Annotation {
    fn with_tyvars(tyvars: HashMap<String, u32>) -> Self {
        Self { tyvars, row: None }
    }
}

struct BuiltinTypeParser<'a> {
    chars: Vec<char>,
    pos: usize,
    vars: HashMap<&'a str, u32>,
    /// The row variable every arrow of the signature shares: no builtin
    /// performs an effect, so a call adds nothing to the caller's row.
    row: u32,
    /// Opaque type names an extension declared, usable like primitives.
    opaque: &'a [String],
}

impl<'a> BuiltinTypeParser<'a> {
    fn new(input: &'a str, vars: HashMap<&'a str, u32>, row: u32, opaque: &'a [String]) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
            vars,
            row,
            opaque,
        }
    }

    fn parse(&mut self) -> Result<Type, String> {
        let ty = self.parse_arrow()?;
        self.skip_ws();
        if self.pos != self.chars.len() {
            return Err(format!("unexpected trailing input at byte {}", self.pos));
        }
        Ok(ty)
    }

    fn parse_arrow(&mut self) -> Result<Type, String> {
        let lhs = self.parse_tuple()?;
        self.skip_ws();
        if self.consume("->") {
            let rhs = self.parse_arrow()?;
            Ok(Type::arrow(lhs, rhs, Type::open_row(self.row)))
        } else {
            Ok(lhs)
        }
    }

    fn parse_tuple(&mut self) -> Result<Type, String> {
        let first = self.parse_app()?;
        let mut elems = vec![first];
        loop {
            self.skip_ws();
            if !self.consume("*") {
                break;
            }
            elems.push(self.parse_app()?);
        }
        if elems.len() == 1 {
            Ok(elems.pop().unwrap())
        } else {
            Ok(Type::Tuple(elems))
        }
    }

    fn parse_app(&mut self) -> Result<Type, String> {
        let mut ty = self.parse_atom()?;
        loop {
            self.skip_ws();
            if self.consume_word("list") {
                ty = Type::list(ty);
            } else if self.consume_word("Option.option") {
                ty = Type::App("Option.option".into(), vec![ty]);
            } else {
                break;
            }
        }
        Ok(ty)
    }

    fn parse_atom(&mut self) -> Result<Type, String> {
        self.skip_ws();
        if self.consume("(") {
            let ty = self.parse_arrow()?;
            self.skip_ws();
            if !self.consume(")") {
                return Err("expected ')'".into());
            }
            return Ok(ty);
        }
        if self.consume("'") {
            let name = self.ident()?;
            let var = self
                .vars
                .get(name.as_str())
                .copied()
                .ok_or_else(|| format!("unknown type variable '{name}"))?;
            return Ok(Type::Var(var));
        }
        let ident = self.ident()?;
        if self.opaque.contains(&ident) {
            return Ok(Type::Con(ident));
        }
        Type::primitive(&ident).ok_or_else(|| format!("unknown type constructor '{ident}"))
    }

    fn ident(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.chars.len()
            && (self.chars[self.pos].is_ascii_alphanumeric()
                || self.chars[self.pos] == '_'
                || self.chars[self.pos] == '.')
        {
            self.pos += 1;
        }
        if self.pos == start {
            Err(format!("expected identifier at byte {}", self.pos))
        } else {
            Ok(self.chars[start..self.pos].iter().collect())
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn consume(&mut self, expected: &str) -> bool {
        self.skip_ws();
        let expected_chars: Vec<char> = expected.chars().collect();
        if self.chars[self.pos..].starts_with(&expected_chars) {
            self.pos += expected_chars.len();
            true
        } else {
            false
        }
    }

    fn consume_word(&mut self, expected: &str) -> bool {
        self.skip_ws();
        let start = self.pos;
        if !self.consume(expected) {
            return false;
        }
        if self.pos < self.chars.len()
            && (self.chars[self.pos].is_ascii_alphanumeric() || self.chars[self.pos] == '_')
        {
            self.pos = start;
            false
        } else {
            true
        }
    }
}

impl Default for InferCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl InferCtx {
    pub fn new() -> Self {
        let mut ctx = Self {
            subst: HashMap::new(),
            next_var: 0,
            scopes: vec![HashMap::new()],
            levels: Vec::new(),
            level: TOP_LEVEL,
            constructors: HashMap::new(),
            type_arities: HashMap::new(),
            opaque_types: Vec::new(),
            type_aliases: HashMap::new(),
            abstract_impls: HashMap::new(),
            constructor_tags: HashMap::new(),
            constructor_aliases: HashMap::new(),
            datatype_constructors: HashMap::new(),
            warnings: Vec::new(),
            effect_sigs: HashMap::new(),
            expr_types: HashMap::new(),
            interner: StringInterner::new(),
            work: Cell::new(0),
            max_work: DEFAULT_MAX_TYPE_WORK,
        };
        ctx.type_arities.insert("list".into(), 1);
        ctx.bind_builtin_type_signatures();
        ctx
    }

    fn bind_builtin_type_signatures(&mut self) {
        for (name, sig) in hiko_builtin_meta::builtin_type_signatures() {
            let scheme = self
                .parse_builtin_scheme(sig)
                .unwrap_or_else(|err| panic!("invalid builtin type signature for {name}: {err}"));
            self.bind(name.to_string(), scheme);
        }
    }

    /// Declare an opaque type named `name`, for values an extension creates
    /// and consumes, such as a client handle. It resolves like a primitive in
    /// source annotations and extension signatures.
    pub fn add_opaque_type(&mut self, name: &str) -> Result<(), String> {
        if Type::primitive(name).is_some() || self.type_arities.contains_key(name) {
            return Err(format!("extension type '{name}' is already defined"));
        }
        self.type_arities.insert(name.to_string(), 0);
        self.opaque_types.push(name.to_string());
        Ok(())
    }

    /// Bind an extension builtin's type. Call `add_opaque_type` first for
    /// any opaque type the signature names.
    pub fn bind_extension_builtin(
        &mut self,
        name: &str,
        sig: hiko_builtin_meta::BuiltinTypeSignature,
    ) -> Result<(), String> {
        if self.lookup(name).is_some() {
            return Err(format!("extension builtin '{name}' is already defined"));
        }
        let scheme = self
            .parse_builtin_scheme(sig)
            .map_err(|err| format!("invalid type for extension builtin '{name}': {err}"))?;
        self.bind(name.to_string(), scheme);
        Ok(())
    }

    fn parse_builtin_scheme(
        &mut self,
        sig: hiko_builtin_meta::BuiltinTypeSignature,
    ) -> Result<Scheme, String> {
        let mut vars = HashMap::new();
        let mut scheme_vars = Vec::new();
        for name in sig.vars {
            let var = self.fresh_id();
            vars.insert(*name, var);
            scheme_vars.push(var);
        }
        let row = self.fresh_id();
        scheme_vars.push(row);
        let mut parser = BuiltinTypeParser::new(sig.ty, vars, row, &self.opaque_types);
        let ty = parser.parse()?;
        Ok(Scheme {
            vars: scheme_vars,
            ty,
        })
    }

    // ── Fresh variables ──────────────────────────────────────────────

    fn fresh_id(&mut self) -> u32 {
        let v = self.next_var;
        self.next_var += 1;
        self.levels.push(self.level);
        v
    }

    /// Run `f` one level deeper: inference of a let-bound right-hand side,
    /// so that `generalize` afterwards takes the variables it created and
    /// did not share with the environment. The level is restored whatever
    /// `f` returns.
    fn at_inner_level<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.level += 1;
        let result = f(self);
        self.level -= 1;
        result
    }

    /// Bind the unbound variable `v` to `ty`, whose variables are unbound
    /// too. They now occur wherever `v` does, so none may stay above `v`'s
    /// level.
    fn bind_var(&mut self, v: u32, ty: Type) {
        self.lower_levels(&ty, self.levels[v as usize]);
        self.subst.insert(v, ty);
    }

    /// Unify two distinct unbound variables. The newer one (higher level,
    /// then higher id) is bound to the older, so the older stays the
    /// representative: binding the other way round would let
    /// `val u0 = y  val u1 = y ...` grow `y`'s type into a chain
    /// `'a -> 'u0 -> 'u1 -> ...` that every later `apply` walks.
    fn link_vars(&mut self, a: u32, b: u32) {
        let key = |v: u32| (self.levels[v as usize], v);
        let (newer, older) = if key(a) > key(b) { (a, b) } else { (b, a) };
        self.bind_var(newer, Type::Var(older));
    }

    /// Lower the level of every variable of `ty` (unbound variables only) to
    /// at most `level`.
    fn lower_levels(&mut self, ty: &Type, level: u32) {
        match ty {
            Type::Var(u) => {
                let l = &mut self.levels[*u as usize];
                *l = (*l).min(level);
            }
            Type::Arrow(a, b, row) => {
                self.lower_levels(a, level);
                self.lower_levels(b, level);
                self.lower_levels(row, level);
            }
            Type::Row(_, tail) => {
                if let Some(tail) = tail {
                    self.lower_levels(tail, level);
                }
            }
            Type::Tuple(ts) | Type::App(_, ts) => {
                for t in ts {
                    self.lower_levels(t, level);
                }
            }
            Type::Con(_) => {}
        }
    }

    fn fresh(&mut self) -> Type {
        Type::Var(self.fresh_id())
    }

    /// A fresh open row `{| e}`.
    fn fresh_row(&mut self) -> Type {
        Type::open_row(self.fresh_id())
    }

    /// `from -> to` with a fresh open row.
    fn arrow(&mut self, from: Type, to: Type) -> Type {
        let row = self.fresh_row();
        Type::arrow(from, to, row)
    }

    /// The row variable that every unmarked arrow in one annotation shares.
    fn annotation_row(&mut self, ann: &mut Annotation) -> u32 {
        match ann.row {
            Some(v) => v,
            None => {
                let v = self.fresh_id();
                ann.row = Some(v);
                v
            }
        }
    }

    // ── Work limit ───────────────────────────────────────────────────

    pub fn set_max_type_work(&mut self, max_work: u64) {
        self.max_work = max_work;
    }

    pub fn type_work(&self) -> u64 {
        self.work.get()
    }

    fn work_exhausted(&self) -> bool {
        self.work.get() > self.max_work
    }

    /// Count one visited type node. Returns false once the limit is passed;
    /// callers then stop walking and return a placeholder. `infer_decl`
    /// turns the exhausted budget into an error, so a placeholder never
    /// reaches a successful result.
    fn charge(&self) -> bool {
        let used = self.work.get() + 1;
        self.work.set(used);
        used <= self.max_work
    }

    // ── Substitution ─────────────────────────────────────────────────

    fn apply(&self, ty: &Type) -> Type {
        self.apply_with(ty, true)
    }

    /// `apply`, charging the work counter only if `charge`: the test-only
    /// checks must not change what a program may cost.
    fn apply_with(&self, ty: &Type, charge: bool) -> Type {
        if charge && !self.charge() {
            return Type::unit();
        }
        let apply = |t| self.apply_with(t, charge);
        match ty {
            Type::Var(v) => match self.subst.get(v) {
                Some(t) => apply(t),
                None => ty.clone(),
            },
            Type::Arrow(a, b, row) => Type::arrow(apply(a), apply(b), apply(row)),
            Type::Row(labels, tail) => match tail {
                None => ty.clone(),
                Some(tail) => extend_row(labels, apply(tail)),
            },
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(apply).collect()),
            Type::App(n, args) => Type::App(n.clone(), args.iter().map(apply).collect()),
            Type::Con(_) => ty.clone(),
        }
    }

    /// Replace type variables according to `mapping`, leaving others untouched.
    fn substitute_vars(&self, ty: &Type, mapping: &HashMap<u32, Type>) -> Type {
        if !self.charge() {
            return Type::unit();
        }
        match ty {
            Type::Var(v) => mapping.get(v).cloned().unwrap_or_else(|| ty.clone()),
            Type::Arrow(a, b, row) => Type::arrow(
                self.substitute_vars(a, mapping),
                self.substitute_vars(b, mapping),
                self.substitute_vars(row, mapping),
            ),
            Type::Row(labels, tail) => match tail {
                None => ty.clone(),
                Some(tail) => extend_row(labels, self.substitute_vars(tail, mapping)),
            },
            Type::Tuple(ts) => Type::Tuple(
                ts.iter()
                    .map(|t| self.substitute_vars(t, mapping))
                    .collect(),
            ),
            Type::App(n, args) => Type::App(
                n.clone(),
                args.iter()
                    .map(|t| self.substitute_vars(t, mapping))
                    .collect(),
            ),
            Type::Con(_) => ty.clone(),
        }
    }

    fn apply_scheme(&self, scheme: &Scheme) -> Scheme {
        Scheme {
            vars: scheme.vars.clone(),
            ty: self.apply(&scheme.ty),
        }
    }

    // ── Unification ──────────────────────────────────────────────────

    fn unify(&mut self, t1: &Type, t2: &Type, span: Span) -> Result<(), TypeError> {
        let t1 = self.apply(t1);
        let t2 = self.apply(t2);
        match (&t1, &t2) {
            (Type::Var(a), Type::Var(b)) if a == b => Ok(()),
            (Type::Var(a), Type::Var(b)) => {
                self.link_vars(*a, *b);
                Ok(())
            }
            (Type::Var(v), _) => {
                if self.occurs(*v, &t2) {
                    return Err(self.err("infinite type (occurs check failed)", span));
                }
                self.bind_var(*v, t2);
                Ok(())
            }
            (_, Type::Var(v)) => {
                if self.occurs(*v, &t1) {
                    return Err(self.err("infinite type (occurs check failed)", span));
                }
                self.bind_var(*v, t1);
                Ok(())
            }
            (Type::Con(a), Type::Con(b)) if a == b => Ok(()),
            (Type::Arrow(a1, b1, r1), Type::Arrow(a2, b2, r2)) => {
                self.unify(a1, a2, span)?;
                self.unify(b1, b2, span)?;
                self.unify(r1, r2, span)
            }
            (Type::Row(l1, t1), Type::Row(l2, t2)) => self.unify_rows(l1, t1, l2, t2, span),
            (Type::Tuple(ts1), Type::Tuple(ts2)) if ts1.len() == ts2.len() => {
                for (a, b) in ts1.iter().zip(ts2.iter()) {
                    self.unify(a, b, span)?;
                }
                Ok(())
            }
            (Type::App(n1, args1), Type::App(n2, args2))
                if n1 == n2 && args1.len() == args2.len() =>
            {
                for (a, b) in args1.iter().zip(args2.iter()) {
                    self.unify(a, b, span)?;
                }
                Ok(())
            }
            _ => Err(self.err(
                &format!(
                    "type mismatch: expected {}, found {}",
                    self.display(&t1),
                    self.display(&t2)
                ),
                span,
            )),
        }
    }

    /// Unify the rows `{l1 | t1}` and `{l2 | t2}` (labels sorted and
    /// distinct, tails resolved). Each side's tail takes the labels only the
    /// other side has; when both are open they share a fresh tail. A closed
    /// side cannot take labels.
    fn unify_rows(
        &mut self,
        l1: &[String],
        t1: &Option<Box<Type>>,
        l2: &[String],
        t2: &Option<Box<Type>>,
        span: Span,
    ) -> Result<(), TypeError> {
        let only1: Vec<String> = l1.iter().filter(|l| !l2.contains(l)).cloned().collect();
        let only2: Vec<String> = l2.iter().filter(|l| !l1.contains(l)).cloned().collect();
        let mismatch = |this: &Self| {
            this.err(
                &format!(
                    "effect mismatch: {} and {}",
                    Type::Row(l1.to_vec(), t1.clone()),
                    Type::Row(l2.to_vec(), t2.clone())
                ),
                span,
            )
        };
        let tail_var = |tail: &Option<Box<Type>>| match tail.as_deref() {
            None => None,
            Some(Type::Var(v)) => Some(*v),
            Some(_) => unreachable!("an applied row's tail is a variable or absent"),
        };
        match (tail_var(t1), tail_var(t2)) {
            // The same tail, or both closed: the labels must agree.
            (a, b) if a == b => {
                if only1.is_empty() && only2.is_empty() {
                    Ok(())
                } else {
                    Err(mismatch(self))
                }
            }
            (Some(a), Some(b)) => {
                if only1.is_empty() && only2.is_empty() {
                    // Same labels: the tails stand for the same effects.
                    self.link_vars(a, b);
                } else {
                    let tail = self.fresh();
                    self.bind_var(a, Type::row(only2, Some(tail.clone())));
                    self.bind_var(b, Type::row(only1, Some(tail)));
                }
                Ok(())
            }
            // One side is closed: the open side may not have labels the
            // closed side lacks, and its tail takes the closed side's extra
            // labels.
            (open_tail, _) => {
                let (open, open_only, closed_only) = match open_tail {
                    Some(a) => (a, &only1, only2),
                    None => (tail_var(t2).expect("one side is open"), &only2, only1),
                };
                if !open_only.is_empty() {
                    return Err(mismatch(self));
                }
                self.bind_var(open, Type::row(closed_only, None));
                Ok(())
            }
        }
    }

    fn occurs(&self, v: u32, ty: &Type) -> bool {
        if !self.charge() {
            return false;
        }
        match ty {
            Type::Var(u) => {
                if let Some(t) = self.subst.get(u) {
                    self.occurs(v, t)
                } else {
                    v == *u
                }
            }
            Type::Arrow(a, b, row) => self.occurs(v, a) || self.occurs(v, b) || self.occurs(v, row),
            Type::Row(_, tail) => tail.as_ref().is_some_and(|tail| self.occurs(v, tail)),
            Type::Tuple(ts) | Type::App(_, ts) => ts.iter().any(|t| self.occurs(v, t)),
            Type::Con(_) => false,
        }
    }

    // ── Generalization and instantiation ─────────────────────────────

    /// Generalize `ty` over its variables above the current level: those
    /// created in the right-hand side just inferred and not shared with the
    /// environment.
    fn generalize(&self, ty: &Type) -> Scheme {
        let ty = self.apply(ty);
        let vars: Vec<u32> = ty
            .free_vars()
            .into_iter()
            .filter(|&v| self.levels[v as usize] > self.level)
            .collect();
        #[cfg(any(test, feature = "check-generalization"))]
        self.check_levels_agree(&ty, &vars);
        Scheme { vars, ty }
    }

    /// The scheme of a binding that is not generalized (the value
    /// restriction): its type now belongs to the environment at this level.
    fn monomorphize(&mut self, ty: &Type) -> Scheme {
        let ty = self.apply(ty);
        self.lower_levels(&ty, self.level);
        Scheme::mono(ty)
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Type {
        let ty = self.apply(&scheme.ty);
        let mapping: HashMap<u32, Type> = scheme.vars.iter().map(|&v| (v, self.fresh())).collect();
        self.substitute_vars(&ty, &mapping)
    }

    // ── Environment ──────────────────────────────────────────────────

    fn lookup(&self, name: &str) -> Option<&Scheme> {
        for scope in self.scopes.iter().rev() {
            if let Some(s) = scope.get(name) {
                return Some(s);
            }
        }
        None
    }

    fn bind(&mut self, name: String, scheme: Scheme) {
        self.scopes.last_mut().unwrap().insert(name, scheme);
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    // ── Program inference ────────────────────────────────────────────

    pub fn infer_program(&mut self, program: &Program) -> Result<(), TypeError> {
        for decl in &program.decls {
            self.infer_decl(decl)?;
        }
        Ok(())
    }

    // ── Declaration inference ────────────────────────────────────────

    pub fn infer_decl(&mut self, decl: &Decl) -> Result<(), TypeError> {
        let scopes = self.scopes.len();
        let result = self.infer_decl_inner(decl);
        if result.is_err() {
            // An error leaves inference mid-way. Drop the scopes it left
            // open: their monomorphic bindings hold variables above this
            // level, which a later declaration must not see.
            self.scopes.truncate(scopes);
        }
        if self.work_exhausted() {
            return Err(self.err(
                &format!(
                    "type checking exceeded its work limit: max_type_work ({}); \
                     the types in this declaration are too large",
                    self.max_work
                ),
                decl.span,
            ));
        }
        result
    }

    fn infer_decl_inner(&mut self, decl: &Decl) -> Result<(), TypeError> {
        match &decl.kind {
            DeclKind::Val(pat, expr) => {
                let bindings = self.at_inner_level(|this| {
                    let expr_ty = this.infer_expr(expr)?;
                    let (pat_ty, bindings) = this.infer_pat(pat)?;
                    this.unify(&expr_ty, &pat_ty, decl.span)?;
                    Ok::<_, TypeError>(bindings)
                })?;
                let can_gen = is_syntactic_value(expr);
                for (name, ty) in bindings {
                    if self.constructors.contains_key(&name) {
                        return Err(self.err(
                            &format!("cannot shadow constructor '{name}' with a value binding"),
                            decl.span,
                        ));
                    }
                    let scheme = if can_gen {
                        self.generalize(&ty)
                    } else {
                        self.monomorphize(&ty)
                    };
                    self.bind(name, scheme);
                }
                Ok(())
            }
            DeclKind::ValRec(sym, expr) => {
                // A scope for the monomorphic self-binding, gone before
                // generalizing.
                let name = self.interner.resolve(*sym).to_string();
                self.push_scope();
                let func_var = self.at_inner_level(|this| {
                    let func_var = this.fresh();
                    this.bind(name.clone(), Scheme::mono(func_var.clone()));
                    let expr_ty = this.infer_expr(expr)?;
                    this.unify(&func_var, &expr_ty, decl.span)?;
                    Ok::<_, TypeError>(func_var)
                })?;
                self.pop_scope();
                let scheme = self.generalize(&func_var);
                self.bind(name, scheme);
                Ok(())
            }
            DeclKind::Fun(bindings) => self.infer_fun_bindings(bindings),
            DeclKind::Datatype(dt) => self.register_datatype(dt),
            DeclKind::TypeAlias(ta) => self.register_type_alias(ta),
            DeclKind::AbstractType(dt) => self.register_abstract_type(dt),
            DeclKind::ExportVal {
                public_name,
                internal_name,
                ty,
            } => {
                let internal = self.interner.resolve(*internal_name).to_string();
                let Some(internal_scheme) = self.lookup(&internal).cloned() else {
                    return Err(self.err(&unbound_message(&internal), decl.span));
                };

                let mut ann = Annotation::default();
                let export_ty = self.resolve_type_expr(ty, &mut ann)?;
                let public = self.interner.resolve(*public_name).to_string();
                if self.constructors.contains_key(&public) {
                    return Err(self.err(
                        &format!("cannot shadow constructor '{public}' with a value binding"),
                        decl.span,
                    ));
                }
                self.check_matches_signature(
                    &public,
                    &internal_scheme,
                    &export_ty,
                    &mut ann,
                    decl.span,
                )?;
                let scheme = Scheme {
                    vars: export_ty.free_vars(),
                    ty: export_ty,
                };
                self.bind(public, scheme);
                Ok(())
            }
            DeclKind::Local(locals, body) => {
                // locals are in a private scope
                self.push_scope();
                for d in locals {
                    self.infer_decl(d)?;
                }
                // body bindings are exported to the enclosing scope
                // so we collect them, pop the private scope, then re-bind
                self.push_scope();
                for d in body {
                    self.infer_decl(d)?;
                }
                let exported = self.scopes.pop().unwrap();
                self.pop_scope(); // pop the locals scope
                // re-bind exported names in the enclosing scope
                for (name, scheme) in exported {
                    self.bind(name, scheme);
                }
                Ok(())
            }
            DeclKind::Import(_) | DeclKind::ImportWithNames(_, _) | DeclKind::Use(_) => Ok(()),
            DeclKind::Signature(_) => unreachable!("signatures must be removed before inference"),
            DeclKind::Structure { .. } => {
                unreachable!("structures must be flattened before inference")
            }
            DeclKind::Effect(sym, payload) => {
                let name = self.interner.resolve(*sym).to_string();
                // Payload types are closed: a type variable here would be
                // generalized at each `perform` site while handlers see a
                // single unknown type.
                let arg_type = if let Some(ty_expr) = payload {
                    self.resolve_type_expr_strict(ty_expr, &mut Annotation::default())?
                } else {
                    Type::unit()
                };
                // The payload has no type variables, but a function type in
                // it has a row variable, which every `perform` and handler
                // shares like the result type.
                self.lower_levels(&arg_type, NEVER_GENERALIZED);
                let result_id = self.fresh_id();
                self.levels[result_id as usize] = NEVER_GENERALIZED;
                self.effect_sigs
                    .insert(name, (arg_type, Type::Var(result_id)));
                Ok(())
            }
        }
    }

    fn infer_fun_bindings(&mut self, bindings: &[FunBinding]) -> Result<(), TypeError> {
        self.push_scope();
        let func_vars = self.at_inner_level(|this| {
            // Monomorphic bindings for all functions (enables mutual recursion)
            let func_vars: Vec<Type> = bindings
                .iter()
                .map(|b| {
                    let v = this.fresh();
                    let name = this.interner.resolve(b.name).to_string();
                    this.bind(name, Scheme::mono(v.clone()));
                    v
                })
                .collect();
            for (binding, func_var) in bindings.iter().zip(func_vars.iter()) {
                let func_ty = this.infer_fun_binding(binding)?;
                this.unify(func_var, &func_ty, binding.span)?;
            }
            Ok::<_, TypeError>(func_vars)
        })?;
        self.pop_scope();

        let schemes: Vec<(String, Scheme)> = bindings
            .iter()
            .zip(func_vars.iter())
            .map(|(b, v)| {
                let name = self.interner.resolve(b.name).to_string();
                (name, self.generalize(v))
            })
            .collect();

        for (name, scheme) in schemes {
            self.bind(name, scheme);
        }
        Ok(())
    }

    fn infer_fun_binding(&mut self, binding: &FunBinding) -> Result<Type, TypeError> {
        let arity = binding.clauses[0].pats.len();
        let arg_vars: Vec<Type> = (0..arity).map(|_| self.fresh()).collect();
        let result_var = self.fresh();

        for clause in &binding.clauses {
            if clause.pats.len() != arity {
                return Err(self.err("clauses have different arities", clause.span));
            }
            self.push_scope();
            for (pat, arg_var) in clause.pats.iter().zip(arg_vars.iter()) {
                let (pat_ty, bindings) = self.infer_pat(pat)?;
                self.unify(&pat_ty, arg_var, pat.span)?;
                for (name, ty) in bindings {
                    self.bind(name, Scheme::mono(ty));
                }
            }
            let body_ty = self.infer_expr(&clause.body)?;
            self.unify(&body_ty, &result_var, clause.body.span)?;
            self.pop_scope();
        }

        // Exhaustiveness and redundancy checking for clausal function
        if arity == 1 {
            let resolved = self.apply(&arg_vars[0]);
            let type_info = self.type_info_for(&resolved);
            let pats: Vec<&Pat> = binding.clauses.iter().map(|c| &c.pats[0]).collect();
            self.check_exhaustiveness(&pats, &type_info, binding.span)?;
        } else {
            // Multi-arg: check as tuple patterns
            let tuple_ty = Type::Tuple(arg_vars.iter().map(|v| self.apply(v)).collect());
            let type_info = self.type_info_for(&tuple_ty);
            let tuple_pats: Vec<Pat> = binding
                .clauses
                .iter()
                .map(|c| Pat {
                    kind: PatKind::Tuple(c.pats.clone()),
                    span: c.span,
                })
                .collect();
            let pat_refs: Vec<&Pat> = tuple_pats.iter().collect();
            self.check_exhaustiveness(&pat_refs, &type_info, binding.span)?;
        }

        // Build the curried function type: arg1 -> arg2 -> ... -> result
        let mut ty = result_var;
        for arg in arg_vars.into_iter().rev() {
            ty = self.arrow(arg, ty);
        }
        Ok(ty)
    }

    /// Reject a user type name that is already declared or that names a
    /// primitive. Primitives are `Type::Con` values, so a datatype named
    /// `Int` would otherwise be the same type as `int`.
    fn check_new_type_name(&self, name: &str, span: Span) -> Result<(), TypeError> {
        if Type::primitive(name).is_some() || Type::primitive_surface_name(name).is_some() {
            return Err(self.err(
                &format!("type name '{name}' is reserved for a primitive type"),
                span,
            ));
        }
        // Type aliases are also recorded in `type_arities`.
        if self.type_arities.contains_key(name) {
            return Err(self.err(&format!("duplicate type name: {name}"), span));
        }
        Ok(())
    }

    fn register_datatype(&mut self, dt: &DatatypeDecl) -> Result<(), TypeError> {
        let dt_name = self.interner.resolve(dt.name).to_string();
        self.check_new_type_name(&dt_name, dt.span)?;
        let arity = dt.tyvars.len();
        self.type_arities.insert(dt_name.clone(), arity);

        // Create type variables for the parameters
        let param_vars: Vec<(String, u32)> = dt
            .tyvars
            .iter()
            .map(|tv| (self.interner.resolve(*tv).to_string(), self.fresh_id()))
            .collect();

        // The result type: T 'a 'b ...
        let result_ty = if arity == 0 {
            Type::Con(dt_name.clone())
        } else {
            Type::App(
                dt_name.clone(),
                param_vars.iter().map(|(_, v)| Type::Var(*v)).collect(),
            )
        };

        let mut all_vars: Vec<u32> = param_vars.iter().map(|(_, v)| *v).collect();
        // Strict resolution adds no type variables, so one copy serves all
        // constructors. It does add the annotation row of function types in
        // payloads, which is quantified with the parameters: for now each
        // use of a constructor gets its own (effect typing, stage 1).
        let mut ann = Annotation::with_tyvars(param_vars.into_iter().collect());
        let payload_row = self.annotation_row(&mut ann);
        all_vars.push(payload_row);

        for con in &dt.constructors {
            let con_name = self.interner.resolve(con.name).to_string();
            if self.constructors.contains_key(&con_name) {
                return Err(self.err(&format!("duplicate constructor name: {con_name}"), con.span));
            }
            let mut vars = all_vars.clone();
            let con_ty = if let Some(ref payload_texpr) = con.payload {
                let payload_ty = self.resolve_type_expr_strict(payload_texpr, &mut ann)?;
                // A constructor performs nothing: its own row is open and
                // quantified, separate from the rows inside the payload.
                let row = self.fresh_id();
                vars.push(row);
                Type::arrow(payload_ty, result_ty.clone(), Type::open_row(row))
            } else {
                result_ty.clone()
            };
            let scheme = Scheme { vars, ty: con_ty };
            self.constructors.insert(con_name.clone(), scheme.clone());
            self.bind(con_name, scheme);
        }
        // Store metadata for exhaustiveness checking
        let con_info: Vec<(String, usize)> = dt
            .constructors
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let c_name = self.interner.resolve(c.name).to_string();
                self.constructor_tags.insert(c_name.clone(), i as u16);
                let arity = if c.payload.is_some() { 1 } else { 0 };
                (c_name, arity)
            })
            .collect();
        self.datatype_constructors.insert(dt_name, con_info);
        Ok(())
    }

    fn register_type_alias(&mut self, ta: &TypeAliasDecl) -> Result<(), TypeError> {
        let ta_name = self.interner.resolve(ta.name).to_string();
        self.check_new_type_name(&ta_name, ta.span)?;
        let mut ann = Annotation::default();
        let mut param_vars = Vec::new();
        for tv in &ta.tyvars {
            let v = self.fresh_id();
            ann.tyvars.insert(self.interner.resolve(*tv).to_string(), v);
            param_vars.push(v);
        }
        let body = self.resolve_type_expr_strict(&ta.ty, &mut ann)?;
        self.type_aliases
            .insert(ta_name.clone(), (param_vars, ann.row, body));
        self.type_arities.insert(ta_name, ta.tyvars.len());
        Ok(())
    }

    fn register_abstract_type(&mut self, dt: &AbstractTypeDecl) -> Result<(), TypeError> {
        let name = self.interner.resolve(dt.name).to_string();
        self.check_new_type_name(&name, dt.span)?;

        if let Some(implementation) = dt.implementation {
            let impl_name = self.interner.resolve(implementation).to_string();
            let impl_arity = self
                .type_arities
                .get(&impl_name)
                .copied()
                .ok_or_else(|| self.err(&format!("unknown type: {impl_name}"), dt.span))?;
            if impl_arity != dt.tyvars.len() {
                return Err(self.err(
                    &format!(
                        "type {impl_name} expects {impl_arity} argument(s), got {}",
                        dt.tyvars.len()
                    ),
                    dt.span,
                ));
            }
            self.abstract_impls.insert(name.clone(), impl_name);
        }

        self.type_arities.insert(name, dt.tyvars.len());
        Ok(())
    }

    /// Check that a structure value matches its signature type.
    ///
    /// The signature's type variables are rigid: the value's own scheme must
    /// instantiate to the signature type with each variable held abstract,
    /// and abstract types are compared through their implementation. This
    /// rejects `val coerce : 'a -> 'b` implemented by `fun coerce x = x`,
    /// and a non-generalized value exported at a polymorphic type.
    fn check_matches_signature(
        &mut self,
        name: &str,
        scheme: &Scheme,
        spec_ty: &Type,
        spec: &mut Annotation,
        span: Span,
    ) -> Result<(), TypeError> {
        // Type variable names keep their leading quote, so these constructor
        // names cannot clash with a declared type.
        // The annotation's row variable is not a type variable, so it stays
        // flexible.
        let rigid: HashMap<u32, Type> = spec
            .tyvars
            .iter()
            .map(|(tv, &v)| (v, Type::Con(tv.clone())))
            .collect();
        #[cfg(any(test, feature = "check-generalization"))]
        let env_vars = self.env_free_vars();
        // Everything this check creates is one level deeper than the
        // environment, so the variables of these types at or below the outer
        // level belong to the environment. One that unification binds to a
        // type mentioning a rigid variable means the value is not polymorphic
        // in it (for example, a weak type variable).
        let (unified, escaped) = self.at_inner_level(|this| {
            let revealed = this.reveal_abstract_types(spec_ty, spec);
            let expected = this.substitute_vars(&revealed, &rigid);
            let actual = this.instantiate(scheme);
            let mut outer_vars = actual.free_vars();
            outer_vars.extend(this.apply(&expected).free_vars());
            outer_vars.retain(|&v| this.levels[v as usize] < this.level);
            let unified = this.unify(&actual, &expected, span);
            let escaped = outer_vars
                .iter()
                .any(|&v| mentions_rigid_var(&this.apply(&Type::Var(v))));
            (unified, escaped)
        });
        unified.map_err(|e| {
            self.err(
                &format!("value {name} does not match its signature: {}", e.message),
                span,
            )
        })?;
        #[cfg(any(test, feature = "check-generalization"))]
        assert!(
            self.work_exhausted()
                || escaped
                    == env_vars
                        .iter()
                        .any(|&v| mentions_rigid_var(&self.apply_with(&Type::Var(v), false))),
            "rigid escape by levels disagrees for {name}"
        );
        if escaped {
            return Err(self.err(
                &format!(
                    "value {name} is less polymorphic than its signature type {}",
                    self.substitute_vars(spec_ty, &rigid)
                ),
                span,
            ));
        }
        Ok(())
    }

    /// Replace abstract types introduced by signature ascription with their
    /// implementation types. An implementation that is an alias takes the
    /// row of the annotation `ann`, as a use of the alias would.
    fn reveal_abstract_types(&mut self, ty: &Type, ann: &mut Annotation) -> Type {
        match ty {
            Type::Con(name) => match self.abstract_impls.get(name).cloned() {
                Some(impl_name) => self.named_type(&impl_name, vec![], ann),
                None => ty.clone(),
            },
            Type::App(name, args) => {
                let args = args
                    .iter()
                    .map(|a| self.reveal_abstract_types(a, ann))
                    .collect();
                match self.abstract_impls.get(name).cloned() {
                    Some(impl_name) => self.named_type(&impl_name, args, ann),
                    None => Type::App(name.clone(), args),
                }
            }
            Type::Arrow(a, b, row) => {
                let a = self.reveal_abstract_types(a, ann);
                let b = self.reveal_abstract_types(b, ann);
                Type::arrow(a, b, (**row).clone())
            }
            Type::Row(..) => ty.clone(),
            Type::Tuple(ts) => Type::Tuple(
                ts.iter()
                    .map(|t| self.reveal_abstract_types(t, ann))
                    .collect(),
            ),
            Type::Var(_) => ty.clone(),
        }
    }

    /// The type named `name` applied to `args`, expanding type aliases.
    fn named_type(&mut self, name: &str, args: Vec<Type>, ann: &mut Annotation) -> Type {
        if let Some((params, row, body)) = self.type_aliases.get(name).cloned() {
            self.instantiate_alias(&body, &params, row, &args, ann)
        } else if args.is_empty() {
            Type::Con(name.to_string())
        } else {
            Type::App(name.to_string(), args)
        }
    }

    // ── Expression inference ─────────────────────────────────────────

    fn infer_expr(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        match &expr.kind {
            ExprKind::IntLit(_) => Ok(Type::int()),
            ExprKind::FloatLit(_) => Ok(Type::float()),
            ExprKind::WordLit(_) => Ok(Type::word()),
            ExprKind::StringLit(_) => Ok(Type::string()),
            ExprKind::CharLit(_) => Ok(Type::char()),
            ExprKind::BoolLit(_) => Ok(Type::bool()),
            ExprKind::Unit => Ok(Type::unit()),

            ExprKind::Var(sym) => {
                let name = self.interner.resolve(*sym);
                if let Some(scheme) = self.lookup(name).cloned() {
                    Ok(self.instantiate(&scheme))
                } else {
                    Err(self.err(&unbound_message(name), expr.span))
                }
            }
            ExprKind::Constructor(sym) => {
                let name = self.interner.resolve(*sym);
                if let Some(scheme) = self.constructors.get(name).cloned() {
                    Ok(self.instantiate(&scheme))
                } else {
                    Err(self.err(&format!("unknown constructor: {name}"), expr.span))
                }
            }

            ExprKind::Tuple(elems) => {
                let tys: Vec<Type> = elems
                    .iter()
                    .map(|e| self.infer_expr(e))
                    .collect::<Result<_, _>>()?;
                Ok(Type::Tuple(tys))
            }
            ExprKind::List(elems) => {
                // Same inference and unification order as the nested
                // `e1 :: (e2 :: ... :: [])` this literal stands for.
                let elem_tys: Vec<Type> = elems
                    .iter()
                    .map(|e| self.infer_expr(e))
                    .collect::<Result<_, _>>()?;
                let mut list_ty = Type::list(self.fresh());
                for elem_ty in elem_tys.into_iter().rev() {
                    let cons_ty = Type::list(elem_ty);
                    self.unify(&list_ty, &cons_ty, expr.span)?;
                    list_ty = cons_ty;
                }
                Ok(list_ty)
            }
            ExprKind::Cons(hd, tl) => {
                let hd_ty = self.infer_expr(hd)?;
                let tl_ty = self.infer_expr(tl)?;
                let list_ty = Type::list(hd_ty);
                self.unify(&tl_ty, &list_ty, expr.span)?;
                Ok(list_ty)
            }

            ExprKind::BinOp(op, lhs, rhs) => self.infer_binop(*op, lhs, rhs, expr.span),

            ExprKind::UnaryNeg(e) => {
                let ty = self.infer_expr(e)?;
                let resolved = self.apply(&ty);
                match &resolved {
                    Type::Con(n) if n == "Int" || n == "Float" => {
                        self.expr_types.insert(expr.span, resolved);
                        Ok(ty)
                    }
                    Type::Con(n) if n == "Word" => {
                        Err(self.err("cannot negate word value, words are unsigned", e.span))
                    }
                    Type::Var(_) => {
                        Err(self.err("~ requires int or float, but type is unconstrained", e.span))
                    }
                    _ => Err(self.err(
                        &format!("~ requires int or float, found {}", self.display(&resolved)),
                        e.span,
                    )),
                }
            }
            ExprKind::Not(_) => {
                unreachable!("desugared to if-then-else")
            }

            ExprKind::App(func, arg) => {
                let func_ty = self.infer_expr(func)?;
                let arg_ty = self.infer_expr(arg)?;
                let result = self.fresh();
                let expected = self.arrow(arg_ty, result.clone());
                self.unify(&func_ty, &expected, expr.span)?;
                Ok(result)
            }

            ExprKind::Fn(pat, body) => {
                self.push_scope();
                let (param_ty, bindings) = self.infer_pat(pat)?;
                for (name, ty) in bindings {
                    self.bind(name, Scheme::mono(ty));
                }
                let body_ty = self.infer_expr(body)?;
                self.pop_scope();
                // Check exhaustiveness for the fn pattern
                let resolved = self.apply(&param_ty);
                let type_info = self.type_info_for(&resolved);
                self.check_exhaustiveness(&[pat], &type_info, expr.span)?;
                Ok(self.arrow(param_ty, body_ty))
            }

            ExprKind::If(cond, then_br, else_br) => {
                let cond_ty = self.infer_expr(cond)?;
                self.unify(&cond_ty, &Type::bool(), cond.span)?;
                let then_ty = self.infer_expr(then_br)?;
                let else_ty = self.infer_expr(else_br)?;
                self.unify(&then_ty, &else_ty, expr.span)?;
                Ok(then_ty)
            }

            ExprKind::Let(decls, body) => {
                self.push_scope();
                for d in decls {
                    self.infer_decl(d)?;
                }
                let ty = self.infer_expr(body)?;
                self.pop_scope();
                Ok(ty)
            }

            ExprKind::Case(scrutinee, branches) => {
                let scrut_ty = self.infer_expr(scrutinee)?;
                let result_var = self.fresh();
                for (pat, body) in branches {
                    self.push_scope();
                    let (pat_ty, bindings) = self.infer_pat(pat)?;
                    self.unify(&pat_ty, &scrut_ty, pat.span)?;
                    for (name, ty) in bindings {
                        self.bind(name, Scheme::mono(ty));
                    }
                    let body_ty = self.infer_expr(body)?;
                    self.unify(&body_ty, &result_var, body.span)?;
                    self.pop_scope();
                }
                // Exhaustiveness and redundancy checking
                let resolved = self.apply(&scrut_ty);
                let type_info = self.type_info_for(&resolved);
                let pats: Vec<&Pat> = branches.iter().map(|(p, _)| p).collect();
                self.check_exhaustiveness(&pats, &type_info, expr.span)?;
                Ok(result_var)
            }

            ExprKind::Ann(e, ty_expr) => {
                let inferred = self.infer_expr(e)?;
                let declared = self.resolve_type_expr(ty_expr, &mut Annotation::default())?;
                self.unify(&inferred, &declared, expr.span)?;
                Ok(declared)
            }

            ExprKind::Paren(e) => self.infer_expr(e),

            ExprKind::Perform(sym, arg) => {
                let name = self.interner.resolve(*sym).to_string();
                let arg_ty = self.infer_expr(arg)?;
                if let Some((declared_arg, declared_res)) = self.effect_sigs.get(&name).cloned() {
                    self.unify(&arg_ty, &declared_arg, expr.span)?;
                    Ok(declared_res)
                } else {
                    Err(self.err(&format!("undeclared effect: {name}"), expr.span))
                }
            }

            ExprKind::Handle {
                body,
                return_var,
                return_body,
                handlers,
            } => {
                let body_ty = self.infer_expr(body)?;
                self.push_scope();
                let return_var_name = self.interner.resolve(*return_var).to_string();
                self.bind(return_var_name, Scheme::mono(body_ty.clone()));
                let return_ty = self.infer_expr(return_body)?;
                self.pop_scope();
                for handler in handlers {
                    self.push_scope();
                    let effect_name = self.interner.resolve(handler.effect_name).to_string();
                    let (payload_ty, effect_result_ty) =
                        self.effect_sigs.get(&effect_name).cloned().ok_or_else(|| {
                            self.err(&format!("undeclared effect: {effect_name}"), handler.span)
                        })?;
                    let payload_var_name = self.interner.resolve(handler.payload_var).to_string();
                    self.bind(payload_var_name, Scheme::mono(payload_ty));
                    let cont_ty = self.arrow(effect_result_ty, body_ty.clone());
                    let cont_var_name = self.interner.resolve(handler.cont_var).to_string();
                    self.bind(cont_var_name, Scheme::mono(cont_ty));
                    let handler_ty = self.infer_expr(&handler.body)?;
                    self.unify(&handler_ty, &return_ty, handler.body.span)?;
                    self.pop_scope();
                }
                Ok(return_ty)
            }

            ExprKind::Resume(cont, arg) => {
                let cont_ty = self.infer_expr(cont)?;
                let arg_ty = self.infer_expr(arg)?;
                let result_ty = self.fresh();
                let expected = self.arrow(arg_ty, result_ty.clone());
                self.unify(&cont_ty, &expected, expr.span)?;
                Ok(result_ty)
            }
        }
    }

    fn infer_binop(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
    ) -> Result<Type, TypeError> {
        let lhs_ty = self.infer_expr(lhs)?;
        let rhs_ty = self.infer_expr(rhs)?;

        let (expected_lhs, expected_rhs, result) = match op {
            BinOp::Pipe => unreachable!("pipeline is desugared to application"),

            // Generic arithmetic: resolve via unification
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                return self.resolve_numeric_operands(&lhs_ty, &rhs_ty, "arithmetic", span);
            }

            // Generic comparison: resolve via unification, return bool
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                self.resolve_numeric_operands(&lhs_ty, &rhs_ty, "comparison", span)?;
                return Ok(Type::bool());
            }

            // Type-specific int ops (kept for safety)
            BinOp::AddInt | BinOp::SubInt | BinOp::MulInt | BinOp::DivInt | BinOp::ModInt => {
                (Type::int(), Type::int(), Type::int())
            }
            BinOp::LtInt | BinOp::GtInt | BinOp::LeInt | BinOp::GeInt => {
                (Type::int(), Type::int(), Type::bool())
            }

            // Type-specific word ops (kept for safety)
            BinOp::AddWord | BinOp::SubWord | BinOp::MulWord | BinOp::DivWord | BinOp::ModWord => {
                (Type::word(), Type::word(), Type::word())
            }
            BinOp::LtWord | BinOp::GtWord | BinOp::LeWord | BinOp::GeWord => {
                (Type::word(), Type::word(), Type::bool())
            }

            BinOp::ConcatStr => (Type::string(), Type::string(), Type::string()),
            BinOp::Andalso | BinOp::Orelse => {
                unreachable!("desugared to if-then-else")
            }
            BinOp::Eq | BinOp::Ne => {
                self.unify(&lhs_ty, &rhs_ty, span)?;
                let resolved = self.apply(&lhs_ty);
                if !resolved.is_equality() {
                    return Err(self.err(
                        &format!(
                            "= and <> require equality types, found {}",
                            self.display(&resolved)
                        ),
                        span,
                    ));
                }
                return Ok(Type::bool());
            }
        };

        self.unify(&lhs_ty, &expected_lhs, lhs.span)?;
        self.unify(&rhs_ty, &expected_rhs, rhs.span)?;
        Ok(result)
    }

    /// Unify the operands of a generic numeric operator and resolve their
    /// type to int, float, or word. An unconstrained operand type defaults to
    /// int. The resolved type is recorded for typed codegen and returned.
    fn resolve_numeric_operands(
        &mut self,
        lhs_ty: &Type,
        rhs_ty: &Type,
        kind: &str,
        span: Span,
    ) -> Result<Type, TypeError> {
        self.unify(lhs_ty, rhs_ty, span)?;
        let resolved = match self.apply(lhs_ty) {
            Type::Var(_) => {
                self.unify(lhs_ty, &Type::int(), span)?;
                Type::int()
            }
            Type::Con(n) if n == "Int" || n == "Float" || n == "Word" => Type::Con(n),
            other => {
                return Err(self.err(
                    &format!(
                        "{kind} operators require int, float, or word, found {}",
                        self.display(&other)
                    ),
                    span,
                ));
            }
        };
        self.expr_types.insert(span, resolved.clone());
        Ok(resolved)
    }

    // ── Pattern inference ────────────────────────────────────────────

    fn infer_pat(&mut self, pat: &Pat) -> Result<(Type, Vec<(String, Type)>), TypeError> {
        let (ty, bindings) = self.infer_pat_inner(pat)?;
        // Check for duplicate variable binders
        let mut seen = HashSet::new();
        for (name, _) in &bindings {
            if name != "_" && !seen.insert(name.clone()) {
                return Err(self.err(&format!("duplicate variable in pattern: {name}"), pat.span));
            }
        }
        Ok((ty, bindings))
    }

    fn infer_pat_inner(&mut self, pat: &Pat) -> Result<(Type, Vec<(String, Type)>), TypeError> {
        match &pat.kind {
            PatKind::Wildcard => Ok((self.fresh(), vec![])),
            PatKind::Var(sym) => {
                let name = self.interner.resolve(*sym).to_string();
                let ty = self.fresh();
                Ok((ty.clone(), vec![(name, ty)]))
            }
            PatKind::IntLit(_) => Ok((Type::int(), vec![])),
            PatKind::FloatLit(_) => Ok((Type::float(), vec![])),
            PatKind::WordLit(_) => Ok((Type::word(), vec![])),
            PatKind::StringLit(_) => Ok((Type::string(), vec![])),
            PatKind::CharLit(_) => Ok((Type::char(), vec![])),
            PatKind::BoolLit(_) => Ok((Type::bool(), vec![])),
            PatKind::Unit => Ok((Type::unit(), vec![])),

            PatKind::Tuple(pats) => {
                let mut tys = Vec::new();
                let mut all_bindings = Vec::new();
                for p in pats {
                    let (ty, bindings) = self.infer_pat_inner(p)?;
                    tys.push(ty);
                    all_bindings.extend(bindings);
                }
                Ok((Type::Tuple(tys), all_bindings))
            }
            PatKind::List(pats) => {
                assert!(
                    pats.is_empty(),
                    "non-empty list pattern should be desugared to Cons"
                );
                Ok((Type::list(self.fresh()), vec![]))
            }
            PatKind::Cons(hd, tl) => {
                let (hd_ty, mut bindings) = self.infer_pat_inner(hd)?;
                let (tl_ty, tl_bindings) = self.infer_pat_inner(tl)?;
                bindings.extend(tl_bindings);
                let list_ty = Type::list(hd_ty);
                self.unify(&tl_ty, &list_ty, pat.span)?;
                Ok((list_ty, bindings))
            }

            PatKind::Constructor(sym, payload) => {
                let name = self.interner.resolve(*sym).to_string();
                let scheme =
                    self.constructors.get(&name).cloned().ok_or_else(|| {
                        self.err(&format!("unknown constructor: {name}"), pat.span)
                    })?;
                let con_ty = self.instantiate(&scheme);
                match payload {
                    None => Ok((con_ty, vec![])),
                    Some(payload_pat) => {
                        let result = self.fresh();
                        let (payload_ty, bindings) = self.infer_pat_inner(payload_pat)?;
                        let expected = self.arrow(payload_ty, result.clone());
                        self.unify(&con_ty, &expected, pat.span)?;
                        Ok((result, bindings))
                    }
                }
            }

            PatKind::Ann(p, ty_expr) => {
                let (pat_ty, bindings) = self.infer_pat_inner(p)?;
                let declared = self.resolve_type_expr(ty_expr, &mut Annotation::default())?;
                self.unify(&pat_ty, &declared, pat.span)?;
                Ok((declared, bindings))
            }
            PatKind::As(sym, p) => {
                let name = self.interner.resolve(*sym).to_string();
                let (pat_ty, mut bindings) = self.infer_pat_inner(p)?;
                bindings.push((name, pat_ty.clone()));
                Ok((pat_ty, bindings))
            }
            PatKind::Paren(p) => self.infer_pat_inner(p),
        }
    }

    // ── Type expression resolution ───────────────────────────────────

    /// An alias body with its parameters replaced by `args` and its row
    /// variable by the using annotation's.
    fn instantiate_alias(
        &mut self,
        body: &Type,
        params: &[u32],
        row: Option<u32>,
        args: &[Type],
        ann: &mut Annotation,
    ) -> Type {
        let mut mapping: HashMap<u32, Type> =
            params.iter().copied().zip(args.iter().cloned()).collect();
        if let Some(row) = row {
            mapping.insert(row, Type::Var(self.annotation_row(ann)));
        }
        if mapping.is_empty() {
            return body.clone();
        }
        self.substitute_vars(body, &mapping)
    }

    fn resolve_type_expr_strict(
        &mut self,
        ty_expr: &TypeExpr,
        ann: &mut Annotation,
    ) -> Result<Type, TypeError> {
        self.resolve_type_expr_inner(ty_expr, ann, true)
    }

    fn resolve_type_expr(
        &mut self,
        ty_expr: &TypeExpr,
        ann: &mut Annotation,
    ) -> Result<Type, TypeError> {
        self.resolve_type_expr_inner(ty_expr, ann, false)
    }

    fn resolve_type_expr_inner(
        &mut self,
        ty_expr: &TypeExpr,
        ann: &mut Annotation,
        strict_tyvars: bool,
    ) -> Result<Type, TypeError> {
        match &ty_expr.kind {
            TypeExprKind::Named(sym) => {
                let name = self.interner.resolve(*sym);
                if let Some(ty) = Type::primitive(name) {
                    return Ok(ty);
                }
                if let Some(lowercase) = Type::primitive_surface_name(name) {
                    return Err(self.err(
                        &format!(
                            "primitive type '{name}' is no longer supported; use '{lowercase}'"
                        ),
                        ty_expr.span,
                    ));
                }
                // Check type aliases (0-arg)
                if let Some((params, row, body)) = self.type_aliases.get(name).cloned() {
                    if params.is_empty() {
                        return Ok(self.instantiate_alias(&body, &[], row, &[], ann));
                    }
                    return Err(self.err(
                        &format!("type {name} expects {} argument(s)", params.len()),
                        ty_expr.span,
                    ));
                }
                // Check type constructors (0-arg)
                if let Some(&arity) = self.type_arities.get(name) {
                    if arity == 0 {
                        return Ok(Type::Con(name.to_string()));
                    }
                    return Err(self.err(
                        &format!("type {name} expects {arity} argument(s)"),
                        ty_expr.span,
                    ));
                }
                Err(self.err(&format!("unknown type: {name}"), ty_expr.span))
            }
            TypeExprKind::Var(sym) => {
                let name = self.interner.resolve(*sym);
                if let Some(&v) = ann.tyvars.get(name) {
                    Ok(Type::Var(v))
                } else if strict_tyvars {
                    Err(self.err(&format!("unbound type variable: {name}"), ty_expr.span))
                } else {
                    let name = name.to_string();
                    let v = self.fresh_id();
                    ann.tyvars.insert(name, v);
                    Ok(Type::Var(v))
                }
            }
            TypeExprKind::App(sym, args) => {
                let name = self.interner.resolve(*sym).to_string();
                let resolved_args: Vec<Type> = args
                    .iter()
                    .map(|a| self.resolve_type_expr_inner(a, ann, strict_tyvars))
                    .collect::<Result<_, _>>()?;
                // Check alias
                if let Some((param_vars, row, body)) = self.type_aliases.get(&name).cloned() {
                    if param_vars.len() != resolved_args.len() {
                        return Err(self.err(
                            &format!(
                                "type {name} expects {} argument(s), got {}",
                                param_vars.len(),
                                resolved_args.len()
                            ),
                            ty_expr.span,
                        ));
                    }
                    return Ok(self.instantiate_alias(
                        &body,
                        &param_vars,
                        row,
                        &resolved_args,
                        ann,
                    ));
                }
                // Check type constructor
                if let Some(&arity) = self.type_arities.get(&name) {
                    if arity != resolved_args.len() {
                        return Err(self.err(
                            &format!(
                                "type {name} expects {arity} argument(s), got {}",
                                resolved_args.len()
                            ),
                            ty_expr.span,
                        ));
                    }
                    return Ok(Type::App(name, resolved_args));
                }
                Err(self.err(&format!("unknown type constructor: {name}"), ty_expr.span))
            }
            TypeExprKind::Arrow(a, b) => {
                let a = self.resolve_type_expr_inner(a, ann, strict_tyvars)?;
                let b = self.resolve_type_expr_inner(b, ann, strict_tyvars)?;
                let row = self.annotation_row(ann);
                Ok(Type::arrow(a, b, Type::open_row(row)))
            }
            TypeExprKind::Tuple(ts) => {
                let resolved: Vec<Type> = ts
                    .iter()
                    .map(|t| self.resolve_type_expr_inner(t, ann, strict_tyvars))
                    .collect::<Result<_, _>>()?;
                Ok(Type::Tuple(resolved))
            }
            TypeExprKind::Paren(t) => self.resolve_type_expr_inner(t, ann, strict_tyvars),
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn type_info_for(&self, ty: &Type) -> TypeInfo {
        match ty {
            Type::Con(name) => match name.as_str() {
                "Bool" => TypeInfo::bool_type(),
                "Unit" => TypeInfo::unit_type(),
                "Int" | "Float" | "Word" | "String" | "Char" => TypeInfo::infinite(),
                _ => {
                    if let Some(cons) = self.datatype_constructors.get(name) {
                        TypeInfo::adt_type(name, cons)
                    } else {
                        TypeInfo::infinite()
                    }
                }
            },
            Type::App(name, _) => {
                if name == "list" {
                    TypeInfo::list_type()
                } else if let Some(cons) = self.datatype_constructors.get(name) {
                    TypeInfo::adt_type(name, cons)
                } else {
                    TypeInfo::infinite()
                }
            }
            Type::Tuple(elems) => TypeInfo::tuple_type(elems.len()),
            Type::Var(_) => TypeInfo::infinite(), // unconstrained, assume infinite
            Type::Arrow(..) | Type::Row(..) => TypeInfo::infinite(),
        }
    }

    fn check_exhaustiveness(
        &mut self,
        pats: &[&Pat],
        type_info: &TypeInfo,
        span: Span,
    ) -> Result<(), TypeError> {
        let result = exhaustive::check_match(
            pats,
            type_info,
            &self.constructor_tags,
            &self.constructor_aliases,
            &self.datatype_constructors,
            &self.interner,
        );
        if !result.exhaustive {
            return Err(self.err("non-exhaustive match", span));
        }
        for idx in result.redundant_clauses {
            self.warnings.push(Warning {
                message: format!("redundant match clause (clause {})", idx + 1),
                span: pats[idx].span,
            });
        }
        Ok(())
    }

    fn display(&self, ty: &Type) -> String {
        format!("{}", self.apply(ty))
    }

    fn err(&self, message: &str, span: Span) -> TypeError {
        TypeError {
            message: message.to_string(),
            span,
        }
    }

    pub fn lookup_type(&self, name: &str) -> Option<Scheme> {
        self.lookup(name).map(|s| self.apply_scheme(s))
    }

    /// Check if a constructor name exists.
    pub fn has_constructor(&self, name: &str) -> bool {
        self.constructors.contains_key(name)
    }

    /// Get the tag for a constructor, if it exists.
    pub fn constructor_tag(&self, name: &str) -> Option<u16> {
        self.constructor_tags.get(name).copied()
    }

    /// Get the scheme for a constructor, if it exists.
    pub fn constructor_scheme(&self, name: &str) -> Option<&Scheme> {
        self.constructors.get(name)
    }

    /// Check if a value binding name exists.
    pub fn has_value(&self, name: &str) -> bool {
        self.lookup(name).is_some()
    }

    /// Bind an alias for a value: make `alias_name` resolve to the same
    /// type scheme as `target_name`.
    pub fn bind_value_alias(&mut self, alias_name: &str, target_name: &str) -> Result<(), String> {
        let scheme = self
            .lookup(target_name)
            .cloned()
            .ok_or_else(|| format!("import alias: '{}' not found in module scope", target_name))?;
        let scheme = self.apply_scheme(&scheme);
        self.bind(alias_name.to_string(), scheme);
        Ok(())
    }

    /// Bind an alias for a constructor: make `alias_name` resolve to the same
    /// constructor as `target_name`. Also copies tag and arity metadata.
    pub fn bind_constructor_alias(
        &mut self,
        alias_name: &str,
        target_name: &str,
    ) -> Result<(), String> {
        let scheme = self
            .constructors
            .get(target_name)
            .cloned()
            .ok_or_else(|| format!("import alias: constructor '{}' not found", target_name))?;
        if self.constructors.contains_key(alias_name) {
            return Err(format!(
                "import alias: '{}' conflicts with an existing constructor",
                alias_name
            ));
        }
        self.constructors
            .insert(alias_name.to_string(), scheme.clone());
        self.bind(alias_name.to_string(), scheme);
        if let Some(&tag) = self.constructor_tags.get(target_name) {
            self.constructor_tags.insert(alias_name.to_string(), tag);
        }
        let canonical = self
            .constructor_aliases
            .get(target_name)
            .cloned()
            .unwrap_or_else(|| target_name.to_string());
        self.constructor_aliases
            .insert(alias_name.to_string(), canonical);
        Ok(())
    }
}

/// `{labels | tail}` with an applied `tail`: a row tail is merged in, so
/// the result's tail is a variable or absent. Both label lists are sorted,
/// so they merge without sorting again.
fn extend_row(labels: &[String], tail: Type) -> Type {
    match tail {
        Type::Row(more, rest) => {
            let mut all = Vec::with_capacity(labels.len() + more.len());
            let (mut i, mut j) = (0, 0);
            while i < labels.len() || j < more.len() {
                let take_left = j == more.len() || (i < labels.len() && labels[i] <= more[j]);
                let next = if take_left {
                    i += 1;
                    &labels[i - 1]
                } else {
                    j += 1;
                    &more[j - 1]
                };
                if all.last() != Some(next) {
                    all.push(next.clone());
                }
            }
            Type::Row(all, rest)
        }
        tail => Type::Row(labels.to_vec(), Some(Box::new(tail))),
    }
}

/// Checks that generalization by levels agrees with its definition, which
/// walks the environment. Slow; only in tests and with the
/// `check-generalization` feature.
#[cfg(any(test, feature = "check-generalization"))]
impl InferCtx {
    /// Levels must generalize exactly the variables of `ty` that are free
    /// nowhere in the environment.
    fn check_levels_agree(&self, ty: &Type, vars: &[u32]) {
        // Past the work limit, `apply` returns placeholders and the
        // declaration fails anyway.
        if self.work_exhausted() {
            return;
        }
        let env_vars = self.env_free_vars();
        let expected: Vec<u32> = ty
            .free_vars()
            .into_iter()
            .filter(|v| !env_vars.contains(v))
            .collect();
        assert_eq!(
            vars, expected,
            "generalization by levels disagrees for {ty}"
        );
    }

    /// The variables free in the environment, found by walking it.
    fn env_free_vars(&self) -> HashSet<u32> {
        let mut vars = HashSet::new();
        for scope in &self.scopes {
            for scheme in scope.values() {
                for v in self.apply_with(&scheme.ty, false).free_vars() {
                    if !scheme.vars.contains(&v) {
                        vars.insert(v);
                    }
                }
            }
        }
        for (arg_ty, result_ty) in self.effect_sigs.values() {
            vars.extend(self.apply_with(arg_ty, false).free_vars());
            vars.extend(self.apply_with(result_ty, false).free_vars());
        }
        vars
    }
}

/// Whether `ty` contains a rigid signature type variable, which
/// `check_matches_signature` represents as a constructor named `'a`.
fn mentions_rigid_var(ty: &Type) -> bool {
    match ty {
        Type::Con(name) => name.starts_with('\''),
        Type::Var(_) => false,
        Type::Arrow(a, b, _) => mentions_rigid_var(a) || mentions_rigid_var(b),
        Type::Row(..) => false,
        Type::Tuple(ts) | Type::App(_, ts) => ts.iter().any(mentions_rigid_var),
    }
}

fn is_syntactic_value(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::WordLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::CharLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Unit
        | ExprKind::Var(_)
        | ExprKind::Constructor(_)
        | ExprKind::Fn(_, _) => true,
        ExprKind::Tuple(elems) | ExprKind::List(elems) => elems.iter().all(is_syntactic_value),
        ExprKind::Cons(hd, tl) => is_syntactic_value(hd) && is_syntactic_value(tl),
        ExprKind::App(func, arg) => {
            matches!(&func.kind, ExprKind::Constructor(_)) && is_syntactic_value(arg)
        }
        ExprKind::Ann(e, _) => is_syntactic_value(e),
        _ => false,
    }
}

/// A builtin left out of this build is reported with the Cargo feature that
/// provides it, rather than as an ordinary unbound name.
fn unbound_message(name: &str) -> String {
    match hiko_builtin_meta::missing_builtin_feature(name) {
        Some(feature) => format!(
            "builtin `{name}` is not in this build of hiko; enable cargo feature `{feature}`"
        ),
        None => format!("unbound variable: {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiko_builtin_meta::builtins;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;

    fn infer(input: &str) -> InferCtx {
        let tokens = Lexer::new(input, 0).tokenize().expect("lex error");
        let program = Parser::new(tokens).parse_program().expect("parse error");
        let program = hiko_syntax::desugar::desugar_program(program);
        let mut ctx = InferCtx::new();
        ctx.interner = program.interner.clone();
        ctx.infer_program(&program).expect("type error");
        ctx
    }

    fn infer_err(input: &str) -> String {
        let tokens = Lexer::new(input, 0).tokenize().expect("lex error");
        let program = Parser::new(tokens).parse_program().expect("parse error");
        let program = hiko_syntax::desugar::desugar_program(program);
        let mut ctx = InferCtx::new();
        ctx.interner = program.interner.clone();
        ctx.infer_program(&program).unwrap_err().message
    }

    /// `val x0 = 0 val x1 = (x0, x0) ...`: the type of `xn` has 2^n leaves.
    fn doubling_tuples(n: usize) -> String {
        let mut src = String::from("val x0 = 0\n");
        for i in 1..=n {
            src.push_str(&format!("val x{i} = (x{}, x{})\n", i - 1, i - 1));
        }
        src
    }

    #[test]
    fn test_exponential_type_hits_the_work_limit() {
        let start = std::time::Instant::now();
        let msg = infer_err(&doubling_tuples(30));
        assert!(
            msg.contains("type checking exceeded its work limit: max_type_work (10000000)"),
            "{msg}"
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn test_exponential_let_polymorphism_hits_the_work_limit() {
        let mut src = String::from("fun f0 x = (x, x)\n");
        for i in 1..10 {
            src.push_str(&format!("fun f{i} x = f{} (f{} x)\n", i - 1, i - 1));
        }
        let msg = infer_err(&src);
        assert!(msg.contains("exceeded its work limit"), "{msg}");
    }

    #[test]
    fn test_type_work_limit_is_configurable() {
        let tokens = Lexer::new(&doubling_tuples(8), 0)
            .tokenize()
            .expect("lex error");
        let program = Parser::new(tokens).parse_program().expect("parse error");
        let program = hiko_syntax::desugar::desugar_program(program);

        let mut ctx = InferCtx::new();
        ctx.interner = program.interner.clone();
        ctx.infer_program(&program).expect("type error");
        let used = ctx.type_work();

        let mut ctx = InferCtx::new();
        ctx.interner = program.interner.clone();
        ctx.set_max_type_work(used - 1);
        let msg = ctx.infer_program(&program).unwrap_err().message;
        assert!(
            msg.contains(&format!("max_type_work ({})", used - 1)),
            "{msg}"
        );
    }

    #[test]
    fn test_many_declarations_do_not_rescan_closed_bindings() {
        // Generalizing used to walk every binding in scope, so work grew with
        // the square of the number of declarations.
        let src: String = (0..2000)
            .map(|i| format!("fun f{i} x = (x, {i})\nval v{i} = f{i} \"s\"\n"))
            .collect();
        let ctx = infer(&src);
        assert!(ctx.type_work() < 2_000_000, "{}", ctx.type_work());
    }

    #[test]
    fn test_weak_bindings_do_not_make_generalization_quadratic() {
        // A value-restricted binding keeps a free type variable, so walking
        // the environment at each generalization never got to skip it.
        // 1,000 pairs cost 133,000 work by levels; walking the environment
        // cost 8,130,700.
        let src: String = (0..1000)
            .map(|i| format!("val w{i} = (fn x => x) (fn y => y)\nfun f{i} x = (x, w{i})\n"))
            .collect();
        let ctx = infer(&src);
        assert!(ctx.type_work() < 500_000, "{}", ctx.type_work());
    }

    #[test]
    fn test_weak_type_variable_is_not_generalized_by_a_later_declaration() {
        // `w`'s type variable belongs to the environment: `g` must not
        // generalize it, so its first use fixes it for the second.
        let msg = infer_err(
            "val w = (fn x => x) (fn y => y)
             fun g z = w z
             val a = g 1
             val b = g \"s\"",
        );
        assert!(msg.contains("mismatch"), "got: {msg}");
    }

    #[test]
    fn test_weak_type_variable_inside_a_let_is_not_generalized() {
        let msg = infer_err(
            "fun f u =
               let
                 val w = (fn x => x) (fn y => y)
                 fun g z = w z
               in
                 (g 1, g \"s\")
               end",
        );
        assert!(msg.contains("mismatch"), "got: {msg}");
    }

    #[test]
    fn test_effect_result_type_is_never_generalized() {
        // One unknown type shared by every `perform` and handler.
        let msg = infer_err(
            "effect Ask of unit
             fun ask () = perform Ask ()
             val a = ask () + 1
             val b = ask () ^ \"s\"",
        );
        assert!(msg.contains("mismatch"), "got: {msg}");
    }

    #[test]
    fn test_inner_function_generalizes_over_its_own_variables_only() {
        let ctx = infer(
            "fun pair x =
               let fun with_x y = (x, y)
               in (with_x 1, with_x \"s\") end",
        );
        let ty = type_of(&ctx, "pair");
        let var = ty.split(' ').next().unwrap();
        assert_eq!(ty.replace(var, "'a"), "'a -> ('a * int) * ('a * string)");
    }

    #[test]
    fn test_bindings_sharing_one_variable_do_not_chain() {
        // Each `val u = y` unifies two variables; binding the older one to
        // the newer made `y`'s type a chain every later lookup walked.
        let src = format!(
            "fun f y = let\n{}\nin u0 end",
            (0..800)
                .map(|i| format!("val u{i} = y\n"))
                .collect::<String>()
        );
        let ctx = infer(&src);
        assert!(ctx.type_work() < 50_000, "{}", ctx.type_work());
    }

    #[test]
    fn test_effect_payload_row_is_never_generalized() {
        // A function-typed payload has a row variable that every `perform`
        // and handler shares, wherever the effect is declared.
        let ctx = infer(
            "effect E of int -> int
             fun g f = perform E f",
        );
        assert!(type_of(&ctx, "g").starts_with("(int -> int) -> "));
        infer(
            "fun h u = let effect E of int -> int in u end
             fun g f = perform E f",
        );
    }

    #[test]
    fn test_a_failed_declaration_leaves_no_scope_behind() {
        let tokens = Lexer::new("fun f x = x + \"a\"", 0).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let program = hiko_syntax::desugar::desugar_program(program);
        let mut ctx = InferCtx::new();
        ctx.interner = program.interner.clone();
        let scopes = ctx.scopes.len();
        assert!(ctx.infer_program(&program).is_err());
        assert_eq!(ctx.scopes.len(), scopes);
    }

    #[test]
    fn infer_ctx_contains_all_metadata_builtins() {
        let ctx = InferCtx::new();
        for meta in builtins() {
            assert!(
                ctx.lookup_type(meta.name).is_some(),
                "missing builtin in InferCtx: {}",
                meta.name
            );
        }
    }

    /// The VM builds `BuiltinProcess.await_result` and `BuiltinProcess.error`
    /// values from the tags in `hiko_builtin_meta`; those tags must match the
    /// declaration order of the datatypes in the `__Builtin.Process` source.
    #[cfg(feature = "builtin-process")]
    #[test]
    fn builtin_process_constructor_tags_match_runtime_abi() {
        use hiko_builtin_meta::{
            PROCESS_AWAIT_RESULT_CONSTRUCTORS, PROCESS_JOIN_ERROR_CONSTRUCTORS,
            internal_builtin_module,
        };
        let module = internal_builtin_module("Process").expect("Process module");
        let ctx = infer(module.source);
        for &(name, tag, arity) in PROCESS_AWAIT_RESULT_CONSTRUCTORS
            .iter()
            .chain(PROCESS_JOIN_ERROR_CONSTRUCTORS)
        {
            let qualified = format!("BuiltinProcess.{name}");
            assert_eq!(ctx.constructor_tag(&qualified), Some(tag), "tag of {name}");
            let scheme = ctx
                .constructor_scheme(&qualified)
                .unwrap_or_else(|| panic!("missing constructor {qualified}"));
            let has_payload = matches!(scheme.ty, Type::Arrow(..));
            assert_eq!(has_payload, arity == 1, "arity of {name}");
        }
    }

    fn type_of(ctx: &InferCtx, name: &str) -> String {
        let scheme = ctx
            .lookup_type(name)
            .unwrap_or_else(|| panic!("no binding: {name}"));
        format!("{}", scheme.ty)
    }

    // ── Literals ─────────────────────────────────────────────────────

    #[test]
    fn test_int_lit() {
        let ctx = infer("val x = 42");
        assert_eq!(type_of(&ctx, "x"), "int");
    }

    #[test]
    fn test_float_lit() {
        let ctx = infer("val x = 3.14");
        assert_eq!(type_of(&ctx, "x"), "float");
    }

    #[test]
    fn test_string_lit() {
        let ctx = infer(r#"val x = "hello""#);
        assert_eq!(type_of(&ctx, "x"), "string");
    }

    #[test]
    fn test_bool_lit() {
        let ctx = infer("val x = true");
        assert_eq!(type_of(&ctx, "x"), "bool");
    }

    #[test]
    fn test_unit() {
        let ctx = infer("val x = ()");
        assert_eq!(type_of(&ctx, "x"), "unit");
    }

    // ── Functions ────────────────────────────────────────────────────

    #[test]
    fn test_identity() {
        let ctx = infer("val id = fn x => x");
        let scheme = ctx.lookup_type("id").unwrap();
        assert!(!scheme.vars.is_empty(), "id should be polymorphic");
    }

    #[test]
    fn test_const_fn() {
        let ctx = infer("fun f x y = x + y");
        assert_eq!(type_of(&ctx, "f"), "int -> int -> int");
    }

    #[test]
    fn test_higher_order() {
        let ctx = infer("fun twice f x = f (f x)");
        let scheme = ctx.lookup_type("twice").unwrap();
        assert!(!scheme.vars.is_empty());
    }

    #[test]
    fn test_structure_qualified_call() {
        let ctx = infer(
            "structure Math = struct
               fun add x y = x + y
             end
             val result = Math.add 1 2",
        );
        assert_eq!(type_of(&ctx, "result"), "int");
    }

    #[test]
    fn test_structure_internal_reference() {
        let ctx = infer(
            "structure M = struct
               fun f x = x + 1
               fun g y = f y
             end
             val result = M.g 41",
        );
        assert_eq!(type_of(&ctx, "result"), "int");
    }

    #[test]
    fn test_structure_signature_checked() {
        let ctx = infer(
            "signature LIST = sig
               val fold : (int * int -> int) -> int -> int list -> int
             end
             structure List : LIST = struct
               fun fold f acc xs =
                 case xs of
                     [] => acc
                   | x :: rest => fold f (f (x, acc)) rest
             end
             val result = List.fold (fn (x, acc) => x + acc) 0 [1, 2, 3]",
        );
        assert_eq!(type_of(&ctx, "result"), "int");
    }

    #[test]
    fn test_structure_signature_missing_export_rejected() {
        let msg = infer_err(
            "signature LIST = sig
               val fold : int -> int
             end
             structure List : LIST = struct
               fun map x = x
             end",
        );
        assert!(msg.contains("unbound variable: List.fold"), "got: {msg}");
    }

    #[test]
    fn test_opaque_ascription_hides_datatype_representation() {
        let ctx = infer(
            "signature BOX = sig
               type t
               val make : int -> t
               val get : t -> int
             end
             structure Box :> BOX = struct
               datatype t = Box of int
               fun make x = Box x
               fun get (Box x) = x
             end
             val result = Box.get (Box.make 41)",
        );
        assert_eq!(type_of(&ctx, "result"), "int");
    }

    #[test]
    fn test_opaque_ascription_hides_type_alias_equality() {
        let msg = infer_err(
            "signature QUEUE = sig
               type 'a t
               val empty : 'a t
             end
             structure Queue :> QUEUE = struct
               type 'a t = 'a list * 'a list
               val empty = ([], [])
             end
             val bad : int list * int list = Queue.empty",
        );
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    #[test]
    fn test_transparent_ascription_keeps_type_equalities() {
        let ctx = infer(
            "signature COUNTER = sig
               type t
               type 'a box
               val zero : t
               val wrap : 'a -> 'a box
             end
             structure C : COUNTER = struct
               type t = int
               type 'a box = 'a list
               val zero = 0
               fun wrap x = [x]
               val hidden = 1
             end
             val n = C.zero + 1
             val xs : int list = C.wrap 1
             val z : C.t = 5",
        );
        assert_eq!(type_of(&ctx, "n"), "int");
        assert_eq!(type_of(&ctx, "xs"), "int list");
        // Transparency only reveals types; unlisted values stay hidden.
        let msg = infer_err(
            "signature S = sig type t val zero : t end
             structure C : S = struct type t = int val zero = 0 val hidden = 1 end
             val h = C.hidden",
        );
        assert!(msg.contains("unbound variable: C.hidden"), "got: {msg}");
    }

    #[test]
    fn test_opaque_ascription_hides_type_equalities() {
        for use_site in ["val n = C.zero + 1", "val z : C.t = 5"] {
            let msg = infer_err(&format!(
                "signature COUNTER = sig type t val zero : t end
                 structure C :> COUNTER = struct type t = int val zero = 0 end
                 {use_site}"
            ));
            assert!(msg.contains("type mismatch"), "{use_site}: got: {msg}");
        }
    }

    #[test]
    fn test_transparent_ascription_checks_type_specs() {
        let msg = infer_err(
            "signature S = sig type t val x : t end
             structure M : S = struct val x = 1 end",
        );
        assert!(msg.contains("unknown type"), "got: {msg}");
    }

    #[test]
    fn test_opaque_ascription_hides_constructors() {
        let msg = infer_err(
            "signature BOX = sig
               type t
               val make : int -> t
             end
             structure Box :> BOX = struct
               datatype t = Box of int
               fun make x = Box x
             end
             val bad = Box.Box 1",
        );
        assert!(msg.contains("unknown constructor: Box.Box"), "got: {msg}");
    }

    /// Rows unify as sets with a tail. Stage 1 adds no labels, so these
    /// cases are checked here directly.
    #[test]
    fn rows_unify_as_sets_with_a_tail() {
        let span = Span::dummy();
        let labels = |ls: &[&str]| ls.iter().map(|l| l.to_string()).collect::<Vec<_>>();
        let mut ctx = InferCtx::new();
        let (a, b) = (ctx.fresh_id(), ctx.fresh_id());
        let r1 = Type::row(labels(&["A"]), Some(Type::Var(a)));
        let r2 = Type::row(labels(&["B"]), Some(Type::Var(b)));
        ctx.unify(&r1, &r2, span).unwrap();
        assert_eq!(ctx.apply(&r1), ctx.apply(&r2));
        let Type::Row(both, Some(_)) = ctx.apply(&r1) else {
            panic!("an open row with A and B")
        };
        assert_eq!(both, labels(&["A", "B"]));

        // A closed row gives an open one its extra labels, and closes it.
        let mut ctx = InferCtx::new();
        let b = ctx.fresh_id();
        let closed = Type::row(labels(&["A", "B"]), None);
        let open = Type::row(labels(&["A"]), Some(Type::Var(b)));
        ctx.unify(&closed, &open, span).unwrap();
        assert_eq!(ctx.apply(&open), closed);

        // A closed row cannot take labels.
        let mut ctx = InferCtx::new();
        let b = ctx.fresh_id();
        let closed = Type::row(labels(&["A"]), None);
        let open = Type::row(labels(&["B"]), Some(Type::Var(b)));
        assert!(ctx.unify(&closed, &open, span).is_err());
        assert!(ctx.unify(&open, &closed, span).is_err());

        // The same tail: the labels must agree.
        let mut ctx = InferCtx::new();
        let a = ctx.fresh_id();
        let r1 = Type::row(labels(&["A"]), Some(Type::Var(a)));
        let r2 = Type::row(labels(&["B"]), Some(Type::Var(a)));
        assert!(ctx.unify(&r1, &r2, span).is_err());
        assert!(ctx.unify(&r1, &r1.clone(), span).is_ok());
    }

    #[test]
    fn extend_row_merges_sorted_labels() {
        let labels = |ls: &[&str]| ls.iter().map(|l| l.to_string()).collect::<Vec<_>>();
        let tail = Type::row(labels(&["B", "D"]), Some(Type::Var(7)));
        assert_eq!(
            extend_row(&labels(&["A", "B", "C"]), tail),
            Type::row(labels(&["A", "B", "C", "D"]), Some(Type::Var(7)))
        );
        assert_eq!(
            extend_row(&labels(&["A"]), Type::Var(3)),
            Type::row(labels(&["A"]), Some(Type::Var(3)))
        );
    }

    #[test]
    fn test_signature_type_variables_are_rigid() {
        for ascription in [":", ":>"] {
            let msg = infer_err(&format!(
                "signature S = sig
                   val coerce : 'a -> 'b
                 end
                 structure M {ascription} S = struct
                   fun coerce x = x
                 end
                 val s : string = M.coerce 42"
            ));
            assert!(msg.contains("does not match its signature"), "got: {msg}");
        }
    }

    #[test]
    fn test_signature_rejects_weak_value_at_polymorphic_type() {
        let msg = infer_err(
            "signature S = sig
               val f : 'a -> 'a
             end
             structure M : S = struct
               val f = (fn x => x) (fn y => y)
             end",
        );
        assert!(msg.contains("less polymorphic"), "got: {msg}");
    }

    #[test]
    fn test_signature_accepts_more_general_value() {
        let ctx = infer(
            "signature S = sig
               type 'a box
               val wrap : 'a -> 'a box
               val pair : int -> int * int
             end
             structure M :> S = struct
               type 'a box = 'a list
               fun wrap x = [x]
               fun pair x = (x, x)
             end
             val b = M.wrap true",
        );
        assert_eq!(type_of(&ctx, "b"), "bool M.box");
    }

    // ── Let-polymorphism ─────────────────────────────────────────────

    #[test]
    fn test_let_poly() {
        let ctx = infer("val r = let val id = fn x => x in (id 42, id true) end");
        assert_eq!(type_of(&ctx, "r"), "int * bool");
    }

    // ── Value restriction ────────────────────────────────────────────

    #[test]
    fn test_value_restriction() {
        let ctx = infer("val f = fn x => x val r = f 42");
        // f is polymorphic, r is int
        assert_eq!(type_of(&ctx, "r"), "int");
    }

    // ── Datatypes ────────────────────────────────────────────────────

    #[test]
    fn test_option() {
        let ctx = infer("datatype 'a option = None | Some of 'a val x = Some 42");
        assert_eq!(type_of(&ctx, "x"), "int option");
    }

    #[test]
    fn test_option_none() {
        let ctx = infer("datatype 'a option = None | Some of 'a val x = None");
        let scheme = ctx.lookup_type("x").unwrap();
        assert!(!scheme.vars.is_empty(), "None should be polymorphic");
    }

    // ── Pattern matching ─────────────────────────────────────────────

    #[test]
    fn test_case_option() {
        let ctx = infer(
            "datatype 'a option = None | Some of 'a
             fun get_or_default opt d = case opt of None => d | Some x => x",
        );
        let scheme = ctx.lookup_type("get_or_default").unwrap();
        assert!(!scheme.vars.is_empty());
    }

    #[test]
    fn test_pattern_tuple() {
        let ctx = infer("val (x, y) = (1, true)");
        assert_eq!(type_of(&ctx, "x"), "int");
        assert_eq!(type_of(&ctx, "y"), "bool");
    }

    // ── Lists ────────────────────────────────────────────────────────

    #[test]
    fn test_list_literal() {
        let ctx = infer("val xs = [1, 2, 3]");
        assert_eq!(type_of(&ctx, "xs"), "int list");
    }

    #[test]
    fn test_list_literal_elements_must_agree() {
        assert_eq!(
            infer_err("val xs = [1, 2, \"a\"]"),
            "type mismatch: expected string, found int"
        );
        let ctx = infer("val xss = [[], [1], [2, 3]]");
        assert_eq!(type_of(&ctx, "xss"), "(int list) list");
    }

    #[test]
    fn test_list_literal_of_values_is_generalized() {
        let ctx = infer("val xs = [[], []]");
        assert_eq!(ctx.lookup_type("xs").unwrap().vars.len(), 1);
    }

    #[test]
    fn test_cons() {
        let ctx = infer("val xs = 1 :: [2, 3]");
        assert_eq!(type_of(&ctx, "xs"), "int list");
    }

    #[test]
    fn test_list_pattern() {
        // Note: hd returns 0 for empty list, so type is int list -> int
        let ctx = infer("fun hd (x :: _) = x | hd [] = 0");
        assert!(ctx.lookup_type("hd").is_some());
    }

    // ── Operators ────────────────────────────────────────────────────

    #[test]
    fn test_int_arith() {
        let ctx = infer("val x = 1 + 2 * 3");
        assert_eq!(type_of(&ctx, "x"), "int");
    }

    #[test]
    fn test_float_arith() {
        let ctx = infer("val x = 1.0 + 2.0");
        assert_eq!(type_of(&ctx, "x"), "float");
    }

    #[test]
    fn test_comparison() {
        let ctx = infer("val b = 1 < 2");
        assert_eq!(type_of(&ctx, "b"), "bool");
    }

    #[test]
    fn test_equality() {
        let ctx = infer("val b = 42 = 42");
        assert_eq!(type_of(&ctx, "b"), "bool");
    }

    // ── Type errors ──────────────────────────────────────────────────

    #[test]
    fn test_type_mismatch() {
        let msg = infer_err("val x = 1 + true");
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    #[test]
    fn test_occurs_check() {
        let msg = infer_err("val f = fn x => x x");
        assert!(msg.contains("infinite type"), "got: {msg}");
    }

    #[test]
    fn test_unbound_var() {
        let msg = infer_err("val x = y");
        assert!(msg.contains("unbound variable"), "got: {msg}");
    }

    #[cfg(not(feature = "builtin-json"))]
    #[test]
    fn test_builtin_left_out_of_build_names_its_feature() {
        let msg = infer_err(r#"val x = json_parse "[]""#);
        assert!(
            msg.contains("builtin `json_parse` is not in this build")
                && msg.contains("`builtin-json`"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_equality_on_list() {
        let msg = infer_err("val b = [1] = [1]");
        assert!(msg.contains("equality"), "got: {msg}");
    }

    // ── If/then/else ─────────────────────────────────────────────────

    #[test]
    fn test_if() {
        let ctx = infer("val x = if true then 1 else 2");
        assert_eq!(type_of(&ctx, "x"), "int");
    }

    #[test]
    fn test_if_branch_mismatch() {
        let msg = infer_err("val x = if true then 1 else true");
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    // ── Mutual recursion ─────────────────────────────────────────────

    #[test]
    fn test_mutual_recursion() {
        let ctx = infer(
            "fun is_even 0 = true | is_even n = is_odd (n - 1)
             and is_odd 0 = false | is_odd n = is_even (n - 1)",
        );
        assert_eq!(type_of(&ctx, "is_even"), "int -> bool");
        assert_eq!(type_of(&ctx, "is_odd"), "int -> bool");
    }

    // ── Type annotations ─────────────────────────────────────────────

    #[test]
    fn test_annotation() {
        let ctx = infer("val x = (42 : int)");
        assert_eq!(type_of(&ctx, "x"), "int");
    }

    #[test]
    fn test_annotation_mismatch() {
        let msg = infer_err("val x = (42 : bool)");
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    #[test]
    fn test_uppercase_primitive_annotation_rejected() {
        let msg = infer_err("val x = (42 : Int)");
        assert!(
            msg.contains("primitive type 'Int' is no longer supported; use 'int'"),
            "got: {msg}"
        );
    }

    // ── Val rec ──────────────────────────────────────────────────────

    #[test]
    fn test_val_rec() {
        let ctx = infer("val rec f = fn n => if n = 0 then 1 else n * f (n - 1)");
        assert_eq!(type_of(&ctx, "f"), "int -> int");
    }

    // ── Complex programs ─────────────────────────────────────────────

    #[test]
    fn test_map() {
        let ctx = infer("fun map f xs = case xs of [] => [] | x :: xs => f x :: map f xs");
        let scheme = ctx.lookup_type("map").unwrap();
        // map should be polymorphic: ('a -> 'b) -> 'a list -> 'b list
        assert!(scheme.vars.len() >= 2, "map should have ≥2 type vars");
    }

    #[test]
    fn test_complex_program() {
        let ctx = infer(
            "datatype 'a option = None | Some of 'a
             fun map_opt f opt = case opt of None => None | Some x => Some (f x)
             val result = map_opt (fn x => x + 1) (Some 42)",
        );
        assert_eq!(type_of(&ctx, "result"), "int option");
    }

    // ── Constructor shadowing ────────────────────────────────────────

    #[test]
    fn test_constructor_pattern_type_mismatch() {
        // `val Red = 42` where Red is a constructor pattern of type color, not int
        let msg = infer_err("datatype color = Red | Blue val Red = 42");
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    // ── Equality on polymorphic vars ─────────────────────────────────

    #[test]
    fn test_equality_polymorphic_rejected() {
        // Polymorphic equality (unresolved type variables) is unsound:
        // it would allow comparing functions. Conservative fix: reject.
        let msg = infer_err("fun f x y = x = y");
        assert!(msg.contains("equality"), "got: {msg}");
    }

    #[test]
    fn test_equality_on_functions_rejected() {
        let msg = infer_err("val b = (fn x => x) = (fn x => x)");
        assert!(msg.contains("equality"), "got: {msg}");
    }

    #[test]
    fn test_equality_on_option_rejected() {
        let msg = infer_err(
            "datatype 'a option = None | Some of 'a
             val b = Some 1 = Some 2",
        );
        assert!(msg.contains("equality"), "got: {msg}");
    }

    // ── Type alias with parameters ───────────────────────────────────

    #[test]
    fn test_type_alias_param() {
        let ctx = infer(
            "type 'a pair = 'a * 'a
             val x = (1, 2) : int pair",
        );
        assert_eq!(type_of(&ctx, "x"), "int * int");
    }

    // ── Unary neg ────────────────────────────────────────────────────

    #[test]
    fn test_neg_int() {
        let ctx = infer("val x = ~42");
        assert_eq!(type_of(&ctx, "x"), "int");
    }

    #[test]
    fn test_neg_float() {
        let ctx = infer("val x = ~3.14");
        assert_eq!(type_of(&ctx, "x"), "float");
    }

    #[test]
    fn test_neg_bool_rejected() {
        let msg = infer_err("val x = ~true");
        assert!(msg.contains("int or float"), "got: {msg}");
    }

    // ── Local exports body bindings ──────────────────────────────────

    #[test]
    fn test_local_exports_body() {
        let ctx = infer("local val x = 1 in val y = x end val z = y");
        assert_eq!(type_of(&ctx, "z"), "int");
    }

    #[test]
    fn test_local_hides_private() {
        let msg = infer_err("local val x = 1 in val y = x end val z = x");
        assert!(msg.contains("unbound variable"), "got: {msg}");
    }

    // ── Unbound type variables rejected ──────────────────────────────

    #[test]
    fn test_unbound_tyvar_in_datatype() {
        let msg = infer_err("datatype 'a bad = Bad of 'b");
        assert!(msg.contains("unbound type variable"), "got: {msg}");
    }

    #[test]
    fn test_unbound_tyvar_in_type_alias() {
        let msg = infer_err("type 'a bad = 'b");
        assert!(msg.contains("unbound type variable"), "got: {msg}");
    }

    // ── Duplicate type/constructor names rejected ────────────────────

    #[test]
    fn test_duplicate_constructor() {
        let msg = infer_err("datatype a = Red datatype b = Red");
        assert!(msg.contains("duplicate constructor"), "got: {msg}");
    }

    #[test]
    fn test_duplicate_type_name() {
        let msg = infer_err("datatype color = Red type color = int");
        assert!(msg.contains("duplicate type"), "got: {msg}");
    }

    #[test]
    fn test_datatype_cannot_reuse_primitive_name() {
        // `Int` is the internal name of `int`; a datatype with that name
        // would make its constructors usable as integers.
        let msg = infer_err("datatype Int = A val x : int = A val y = x + 1");
        assert!(msg.contains("reserved for a primitive"), "got: {msg}");
        let msg = infer_err("datatype Bool = X | Y val c = if X then 1 else 2");
        assert!(msg.contains("reserved for a primitive"), "got: {msg}");
        let msg = infer_err("datatype int = A");
        assert!(msg.contains("reserved for a primitive"), "got: {msg}");
        let msg = infer_err("type String = int");
        assert!(msg.contains("reserved for a primitive"), "got: {msg}");
    }

    // ── val rec polymorphism ─────────────────────────────────────────

    #[test]
    fn test_val_rec_polymorphic() {
        let ctx = infer("val rec id = fn x => x val a = id 1 val b = id true");
        assert_eq!(type_of(&ctx, "a"), "int");
        assert_eq!(type_of(&ctx, "b"), "bool");
    }

    // ── Duplicate pattern variables rejected ─────────────────────────

    #[test]
    fn test_duplicate_pat_var() {
        let msg = infer_err("val (x, x) = (1, 2)");
        assert!(msg.contains("duplicate variable"), "got: {msg}");
    }

    // ── Builtins are typed ───────────────────────────────────────────

    #[test]
    fn test_builtin_int_to_string() {
        let ctx = infer("val s = int_to_string 42");
        assert_eq!(type_of(&ctx, "s"), "string");
    }

    #[test]
    fn test_spawn_returns_pid() {
        let ctx = infer("val child = spawn (fn () => 42)");
        assert_eq!(type_of(&ctx, "child"), "pid");
    }

    #[test]
    fn test_await_process_rejects_int() {
        let msg = infer_err("val x = await_process 123");
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    #[test]
    fn test_panic_instantiates_per_call_site() {
        let ctx = infer(
            "datatype 'a option = None | Some of 'a
             val n = case Some 1 of Some x => x | None => panic \"missing int\"
             val b = case Some true of Some x => x | None => panic \"missing bool\"",
        );
        assert_eq!(type_of(&ctx, "n"), "int");
        assert_eq!(type_of(&ctx, "b"), "bool");
    }

    #[test]
    fn test_assert_eq_instantiates_per_call_site() {
        let ctx = infer(
            r#"val int_check = assert_eq (1, 1, "int")
               val bool_check = assert_eq (true, true, "bool")"#,
        );
        assert_eq!(type_of(&ctx, "int_check"), "unit");
        assert_eq!(type_of(&ctx, "bool_check"), "unit");
    }

    #[test]
    fn test_await_process_instantiates_per_call_site() {
        let ctx = infer(
            "fun wait_int p = await_process p + 1
             fun wait_bool p = if await_process p then 1 else 0",
        );
        assert_eq!(type_of(&ctx, "wait_int"), "pid -> int");
        assert_eq!(type_of(&ctx, "wait_bool"), "pid -> int");
    }

    #[test]
    fn test_pipeline_desugars_before_inference() {
        let ctx = infer(
            "fun inc x = x + 1
             val y = 1 |> inc",
        );
        assert_eq!(type_of(&ctx, "y"), "int");
    }

    // ── Algebraic effects ────────────────────────────────────────────

    #[test]
    fn test_perform_undeclared_effect_rejected() {
        let msg = infer_err("val x = perform Missing ()");
        assert!(msg.contains("undeclared effect: Missing"), "got: {msg}");
    }

    #[test]
    fn test_perform_payload_type_checked() {
        let msg = infer_err("effect Log of string val x = perform Log 1");
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    #[test]
    fn test_perform_result_type_flows_to_context() {
        let ctx = infer(
            "effect Get of unit
             val x = (perform Get ()) + 1",
        );
        assert_eq!(type_of(&ctx, "x"), "int");
    }

    #[test]
    fn test_handler_undeclared_effect_rejected() {
        let msg = infer_err(
            "val x = handle
               1
             with
               return y => y
             | Missing payload k => 0",
        );
        assert!(msg.contains("undeclared effect: Missing"), "got: {msg}");
    }

    #[test]
    fn test_handler_body_return_type_checked() {
        let msg = infer_err(
            "effect Fail of string
             val x = handle
               1
             with
               return y => y
             | Fail message k => true",
        );
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    #[test]
    fn test_resume_argument_type_checked() {
        let msg = infer_err(
            "effect Get of unit
             val x = handle
               (perform Get ()) + 1
             with
               return y => y
             | Get _ k => resume k true",
        );
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    #[test]
    fn test_resume_result_matches_handled_body_type() {
        let ctx = infer(
            "effect Get of unit
             val x = handle
               (perform Get ()) + 1
             with
               return y => y
             | Get _ k => resume k 41",
        );
        assert_eq!(type_of(&ctx, "x"), "int");
    }

    #[test]
    fn test_resume_result_allows_generator_wrapper() {
        let ctx = infer(
            "effect Yield of int
             fun collect f = handle
               f ()
             with
               return _ => []
             | Yield n k => n :: collect (fn _ => resume k ())
             val xs = collect (fn _ => perform Yield 1)",
        );
        assert_eq!(type_of(&ctx, "xs"), "int list");
    }

    #[test]
    fn test_effect_result_type_not_generalized() {
        // The handler resumes Get with an int, so `f ()` must not also be
        // usable as a string.
        let msg = infer_err(
            "effect Get of unit
             val f = fn () => perform Get ()
             val s : string = f ()
             val r = handle string_length (f ()) with
                 return x => x
               | Get _ k => resume k 42",
        );
        assert!(msg.contains("type mismatch"), "got: {msg}");
    }

    #[test]
    fn test_effect_payload_type_variable_rejected() {
        let msg = infer_err(
            "effect Put of 'a
             val f = fn x => perform Put x
             val r = handle f \"str\" with
                 return _ => 0
               | Put v k => v + 1",
        );
        assert!(msg.contains("unbound type variable"), "got: {msg}");
    }

    // ── Exhaustiveness checking ──────────────────────────────────────

    #[test]
    fn test_non_exhaustive_case_bool() {
        let msg = infer_err("val x = case true of false => 0");
        assert!(msg.contains("non-exhaustive"), "got: {msg}");
    }

    #[test]
    fn test_non_exhaustive_clausal_fun() {
        let msg = infer_err("fun f 0 = 1");
        assert!(msg.contains("non-exhaustive"), "got: {msg}");
    }

    #[test]
    fn test_exhaustive_bool() {
        let ctx = infer("val x = case true of true => 1 | false => 0");
        assert_eq!(type_of(&ctx, "x"), "int");
    }

    #[test]
    fn test_exhaustive_option() {
        let _ctx = infer(
            "datatype 'a option = None | Some of 'a
             fun f opt = case opt of None => 0 | Some x => x",
        );
    }

    #[test]
    fn test_qualified_constructor_expr_and_exhaustive_case() {
        let ctx = infer(
            "structure Option = struct
               datatype 'a option = None | Some of 'a
             end
             val result =
               case Option.Some 41 of
                   Option.None => 0
                 | Option.Some x => x",
        );
        assert_eq!(type_of(&ctx, "result"), "int");
    }

    #[test]
    fn test_qualified_constructor_case_non_exhaustive() {
        let msg = infer_err(
            "structure Option = struct
               datatype 'a option = None | Some of 'a
             end
             fun unwrap opt =
               case opt of
                   Option.Some x => x",
        );
        assert!(msg.contains("non-exhaustive"), "got: {msg}");
    }

    #[test]
    fn test_exhaustive_wildcard() {
        let ctx = infer("val x = case 42 of _ => 1");
        assert_eq!(type_of(&ctx, "x"), "int");
    }

    #[test]
    fn test_exhaustive_list() {
        let _ctx = infer("fun f xs = case xs of [] => 0 | _ :: _ => 1");
    }

    #[test]
    fn test_non_exhaustive_list() {
        let msg = infer_err("fun f (x :: _) = x");
        assert!(msg.contains("non-exhaustive"), "got: {msg}");
    }

    #[test]
    fn test_exhaustive_3way_adt() {
        let _ctx = infer(
            "datatype expr = Num of int | Add of expr * expr | Mul of expr * expr
             fun eval e = case e of Num n => n | Add (a, b) => eval a + eval b | Mul (a, b) => eval a * eval b
             val result = eval (Add (Num 1, Mul (Num 2, Num 3)))",
        );
    }

    #[test]
    fn test_redundant_clause_warning() {
        let tokens = hiko_syntax::lexer::Lexer::new("val x = case true of _ => 1 | false => 2", 0)
            .tokenize()
            .unwrap();
        let program = hiko_syntax::parser::Parser::new(tokens)
            .parse_program()
            .unwrap();
        let program = hiko_syntax::desugar::desugar_program(program);
        let mut ctx = InferCtx::new();
        ctx.interner = program.interner.clone();
        ctx.infer_program(&program).unwrap();
        assert!(!ctx.warnings.is_empty(), "expected redundancy warning");
        assert!(
            ctx.warnings[0].message.contains("redundant"),
            "got: {}",
            ctx.warnings[0].message
        );
    }

    // ── Nested ADT payload exhaustiveness ────────────────────────────

    #[test]
    fn test_nested_adt_non_exhaustive() {
        let msg = infer_err(
            "datatype flag = On | Off
             datatype t = A of flag | B
             val x = case A Off of A On => 1 | B => 0",
        );
        assert!(msg.contains("non-exhaustive"), "got: {msg}");
    }

    #[test]
    fn test_nested_adt_exhaustive() {
        let _ctx = infer(
            "datatype flag = On | Off
             datatype t = A of flag | B
             val x = case A Off of A On => 1 | A Off => 2 | B => 0",
        );
    }

    #[test]
    fn test_constructor_alias_nested_non_exhaustive() {
        // `X1`/`X2` alias `A1`/`A2`, as named imports do. `A3` is missing, so
        // the match must be rejected whichever datatype shares their tags.
        // Repeat with fresh contexts so HashMap iteration order varies.
        let src = "datatype a = A1 | A2 | A3
                   datatype b = B1 | B2
                   datatype 'x box = Box of 'x
                   val f = fn v => case v of Box X1 => 1 | Box X2 => 2";
        for _ in 0..32 {
            let tokens = Lexer::new(src, 0).tokenize().expect("lex error");
            let program = Parser::new(tokens).parse_program().expect("parse error");
            let program = hiko_syntax::desugar::desugar_program(program);
            let mut ctx = InferCtx::new();
            ctx.interner = program.interner.clone();
            let (types, rest) = program.decls.split_at(3);
            for decl in types {
                ctx.infer_decl(decl).expect("type error");
            }
            ctx.bind_constructor_alias("X1", "A1").unwrap();
            ctx.bind_constructor_alias("X2", "A2").unwrap();
            let msg = ctx.infer_decl(&rest[0]).unwrap_err().message;
            assert!(msg.contains("non-exhaustive"), "got: {msg}");
        }
    }

    // ── fn pattern exhaustiveness ────────────────────────────────────

    #[test]
    fn test_fn_non_exhaustive() {
        let msg = infer_err("val f = fn true => 1");
        assert!(msg.contains("non-exhaustive"), "got: {msg}");
    }

    #[test]
    fn test_fn_exhaustive() {
        let _ctx = infer("val f = fn _ => 1");
    }

    // ── Distinct literal redundancy ──────────────────────────────────

    #[test]
    fn test_distinct_literals_not_redundant() {
        let tokens =
            hiko_syntax::lexer::Lexer::new("val x = case 2 of 1 => 1 | 2 => 2 | _ => 3", 0)
                .tokenize()
                .unwrap();
        let program = hiko_syntax::parser::Parser::new(tokens)
            .parse_program()
            .unwrap();
        let program = hiko_syntax::desugar::desugar_program(program);
        let mut ctx = InferCtx::new();
        ctx.interner = program.interner.clone();
        ctx.infer_program(&program).unwrap();
        assert!(
            ctx.warnings.is_empty(),
            "distinct literals should not be redundant, got: {:?}",
            ctx.warnings
        );
    }
}
