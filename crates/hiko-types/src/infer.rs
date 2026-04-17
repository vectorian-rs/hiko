use std::collections::{HashMap, HashSet};

use hiko_syntax::ast::*;
use hiko_syntax::intern::StringInterner;
use hiko_syntax::span::Span;

use crate::exhaustive::{self, TypeInfo};
use crate::ty::{Scheme, Type};

const UPPERCASE_PRIMITIVE_ALIASES: &[(&str, &str)] = &[
    ("Int", "int"),
    ("Float", "float"),
    ("Bool", "bool"),
    ("String", "string"),
    ("Char", "char"),
    ("Unit", "unit"),
    ("Bytes", "bytes"),
    ("Rng", "rng"),
    ("Pid", "pid"),
];

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
    constructors: HashMap<String, Scheme>,
    type_arities: HashMap<String, usize>,
    type_aliases: HashMap<String, (Vec<u32>, Type)>,
    /// Constructor name → tag (for exhaustiveness checking)
    constructor_tags: HashMap<String, u16>,
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
            constructors: HashMap::new(),
            type_arities: HashMap::new(),
            type_aliases: HashMap::new(),
            constructor_tags: HashMap::new(),
            datatype_constructors: HashMap::new(),
            warnings: Vec::new(),
            effect_sigs: HashMap::new(),
            expr_types: HashMap::new(),
            interner: StringInterner::new(),
        };
        ctx.type_arities.insert("list".into(), 1);
        // Register runtime builtins
        // 'a type variable for polymorphic builtins
        let a_var = ctx.next_var;
        ctx.next_var += 1;
        let a = Type::Var(a_var);

        let builtins: &[(&str, Type)] = &[
            // I/O
            ("print", Type::arrow(Type::string(), Type::unit())),
            ("println", Type::arrow(Type::string(), Type::unit())),
            ("read_line", Type::arrow(Type::unit(), Type::string())),
            ("read_stdin", Type::arrow(Type::unit(), Type::string())),
            // Conversions
            ("int_to_string", Type::arrow(Type::int(), Type::string())),
            (
                "float_to_string",
                Type::arrow(Type::float(), Type::string()),
            ),
            ("string_to_int", Type::arrow(Type::string(), Type::int())),
            ("char_to_int", Type::arrow(Type::char(), Type::int())),
            ("int_to_char", Type::arrow(Type::int(), Type::char())),
            ("int_to_float", Type::arrow(Type::int(), Type::float())),
            // string ops
            ("string_length", Type::arrow(Type::string(), Type::int())),
            (
                "substring",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::int(), Type::int()]),
                    Type::string(),
                ),
            ),
            (
                "string_contains",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string()]),
                    Type::bool(),
                ),
            ),
            ("trim", Type::arrow(Type::string(), Type::string())),
            (
                "split",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string()]),
                    Type::list(Type::string()),
                ),
            ),
            (
                "string_replace",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string(), Type::string()]),
                    Type::string(),
                ),
            ),
            // Math
            ("sqrt", Type::arrow(Type::float(), Type::float())),
            ("abs_int", Type::arrow(Type::int(), Type::int())),
            ("abs_float", Type::arrow(Type::float(), Type::float())),
            ("floor", Type::arrow(Type::float(), Type::int())),
            ("ceil", Type::arrow(Type::float(), Type::int())),
            // Filesystem
            ("read_file", Type::arrow(Type::string(), Type::string())),
            (
                "write_file",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string()]),
                    Type::unit(),
                ),
            ),
            ("file_exists", Type::arrow(Type::string(), Type::bool())),
            (
                "list_dir",
                Type::arrow(Type::string(), Type::list(Type::string())),
            ),
            ("remove_file", Type::arrow(Type::string(), Type::unit())),
            ("create_dir", Type::arrow(Type::string(), Type::unit())),
            ("is_dir", Type::arrow(Type::string(), Type::bool())),
            ("is_file", Type::arrow(Type::string(), Type::bool())),
            (
                "path_join",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string()]),
                    Type::string(),
                ),
            ),
            // HTTP
            (
                "http_get",
                Type::arrow(
                    Type::string(),
                    Type::Tuple(vec![
                        Type::int(),
                        Type::list(Type::Tuple(vec![Type::string(), Type::string()])),
                        Type::string(),
                    ]),
                ),
            ),
            // General HTTP: (method, url, headers, body) -> (status, response_headers, body)
            (
                "http",
                Type::arrow(
                    Type::Tuple(vec![
                        Type::string(),
                        Type::string(),
                        Type::list(Type::Tuple(vec![Type::string(), Type::string()])),
                        Type::string(),
                    ]),
                    Type::Tuple(vec![
                        Type::int(),
                        Type::list(Type::Tuple(vec![Type::string(), Type::string()])),
                        Type::string(),
                    ]),
                ),
            ),
            (
                "http_json",
                Type::arrow(
                    Type::Tuple(vec![
                        Type::string(),
                        Type::string(),
                        Type::list(Type::Tuple(vec![Type::string(), Type::string()])),
                        Type::string(),
                    ]),
                    Type::Tuple(vec![
                        Type::int(),
                        Type::list(Type::Tuple(vec![Type::string(), Type::string()])),
                        a.clone(),
                    ]),
                ),
            ),
            (
                "http_msgpack",
                Type::arrow(
                    Type::Tuple(vec![
                        Type::string(),
                        Type::string(),
                        Type::list(Type::Tuple(vec![Type::string(), Type::string()])),
                        Type::string(),
                    ]),
                    Type::Tuple(vec![
                        Type::int(),
                        Type::list(Type::Tuple(vec![Type::string(), Type::string()])),
                        a.clone(),
                    ]),
                ),
            ),
            // HTTP bytes
            (
                "http_bytes",
                Type::arrow(
                    Type::Tuple(vec![
                        Type::string(),
                        Type::string(),
                        Type::list(Type::Tuple(vec![Type::string(), Type::string()])),
                        Type::string(),
                    ]),
                    Type::Tuple(vec![
                        Type::int(),
                        Type::list(Type::Tuple(vec![Type::string(), Type::string()])),
                        Type::bytes(),
                    ]),
                ),
            ),
            // File I/O (bytes)
            (
                "read_file_bytes",
                Type::arrow(Type::string(), Type::bytes()),
            ),
            // Hashing
            ("blake3", Type::arrow(Type::bytes(), Type::string())),
            // bytes
            ("bytes_length", Type::arrow(Type::bytes(), Type::int())),
            (
                "bytes_to_string",
                Type::arrow(Type::bytes(), Type::string()),
            ),
            (
                "string_to_bytes",
                Type::arrow(Type::string(), Type::bytes()),
            ),
            (
                "bytes_get",
                Type::arrow(Type::Tuple(vec![Type::bytes(), Type::int()]), Type::int()),
            ),
            (
                "bytes_slice",
                Type::arrow(
                    Type::Tuple(vec![Type::bytes(), Type::int(), Type::int()]),
                    Type::bytes(),
                ),
            ),
            // Crypto RNG
            ("random_bytes", Type::arrow(Type::int(), Type::bytes())),
            // Deterministic RNG
            ("rng_seed", Type::arrow(Type::bytes(), Type::rng())),
            (
                "rng_bytes",
                Type::arrow(
                    Type::Tuple(vec![Type::rng(), Type::int()]),
                    Type::Tuple(vec![Type::bytes(), Type::rng()]),
                ),
            ),
            (
                "rng_int",
                Type::arrow(
                    Type::Tuple(vec![Type::rng(), Type::int()]),
                    Type::Tuple(vec![Type::int(), Type::rng()]),
                ),
            ),
            // Hashline read
            (
                "read_file_tagged",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::int(), Type::int()]),
                    Type::string(),
                ),
            ),
            (
                "edit_file_tagged",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string()]),
                    Type::string(),
                ),
            ),
            // Glob & walk
            (
                "glob",
                Type::arrow(Type::string(), Type::list(Type::string())),
            ),
            (
                "walk_dir",
                Type::arrow(Type::string(), Type::list(Type::string())),
            ),
            // Regex
            (
                "regex_match",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string()]),
                    Type::bool(),
                ),
            ),
            (
                "regex_replace",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string(), Type::string()]),
                    Type::string(),
                ),
            ),
            // JSON (typed — requires use "stdlib/json.hml")
            ("json_parse", Type::arrow(Type::string(), a.clone())),
            ("json_to_string", Type::arrow(a.clone(), Type::string())),
            // JSON (string-based convenience)
            (
                "json_get",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string()]),
                    Type::string(),
                ),
            ),
            (
                "json_keys",
                Type::arrow(Type::string(), Type::list(Type::string())),
            ),
            ("json_length", Type::arrow(Type::string(), Type::int())),
            // Environment & string utils
            ("getenv", Type::arrow(Type::string(), Type::string())),
            (
                "starts_with",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string()]),
                    Type::bool(),
                ),
            ),
            (
                "ends_with",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::string()]),
                    Type::bool(),
                ),
            ),
            ("to_upper", Type::arrow(Type::string(), Type::string())),
            ("to_lower", Type::arrow(Type::string(), Type::string())),
            ("epoch", Type::arrow(Type::unit(), Type::int())),
            ("epoch_ms", Type::arrow(Type::unit(), Type::int())),
            ("monotonic_ms", Type::arrow(Type::unit(), Type::int())),
            ("sleep", Type::arrow(Type::int(), Type::unit())),
            (
                "string_join",
                Type::arrow(
                    Type::Tuple(vec![Type::list(Type::string()), Type::string()]),
                    Type::string(),
                ),
            ),
            // Exec
            (
                "exec",
                Type::arrow(
                    Type::Tuple(vec![Type::string(), Type::list(Type::string())]),
                    Type::Tuple(vec![Type::int(), Type::string(), Type::string()]),
                ),
            ),
            // System
            // Process operations
            (
                "spawn",
                Type::arrow(Type::arrow(Type::unit(), a.clone()), Type::pid()),
            ),
            ("await_process", Type::arrow(Type::pid(), a.clone())),
            ("exit", Type::arrow(Type::int(), Type::unit())),
            ("panic", Type::arrow(Type::string(), a.clone())),
            // Testing
            (
                "assert",
                Type::arrow(
                    Type::Tuple(vec![Type::bool(), Type::string()]),
                    Type::unit(),
                ),
            ),
            (
                "assert_eq",
                Type::arrow(
                    Type::Tuple(vec![a.clone(), a, Type::string()]),
                    Type::unit(),
                ),
            ),
        ];
        for &(name, ref ty) in builtins {
            ctx.bind(name.to_string(), Scheme::mono(ty.clone()));
        }
        ctx
    }

    // ── Fresh variables ──────────────────────────────────────────────

    fn fresh(&mut self) -> Type {
        let v = self.next_var;
        self.next_var += 1;
        Type::Var(v)
    }

    // ── Substitution ─────────────────────────────────────────────────

    fn apply(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(v) => {
                if let Some(t) = self.subst.get(v) {
                    self.apply(t)
                } else {
                    ty.clone()
                }
            }
            Type::Arrow(a, b) => Type::arrow(self.apply(a), self.apply(b)),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.apply(t)).collect()),
            Type::App(n, args) => {
                Type::App(n.clone(), args.iter().map(|t| self.apply(t)).collect())
            }
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
            (Type::Var(v), _) => {
                if self.occurs(*v, &t2) {
                    return Err(self.err("infinite type (occurs check failed)", span));
                }
                self.subst.insert(*v, t2);
                Ok(())
            }
            (_, Type::Var(v)) => {
                if self.occurs(*v, &t1) {
                    return Err(self.err("infinite type (occurs check failed)", span));
                }
                self.subst.insert(*v, t1);
                Ok(())
            }
            (Type::Con(a), Type::Con(b)) if a == b => Ok(()),
            (Type::Arrow(a1, b1), Type::Arrow(a2, b2)) => {
                self.unify(a1, a2, span)?;
                self.unify(b1, b2, span)
            }
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

    fn occurs(&self, v: u32, ty: &Type) -> bool {
        match ty {
            Type::Var(u) => {
                if let Some(t) = self.subst.get(u) {
                    self.occurs(v, t)
                } else {
                    v == *u
                }
            }
            Type::Arrow(a, b) => self.occurs(v, a) || self.occurs(v, b),
            Type::Tuple(ts) | Type::App(_, ts) => ts.iter().any(|t| self.occurs(v, t)),
            Type::Con(_) => false,
        }
    }

    // ── Generalization and instantiation ─────────────────────────────

    fn generalize(&self, ty: &Type, exclude_scope: bool) -> Scheme {
        let ty = self.apply(ty);
        let env_vars = self.free_vars_in_env(exclude_scope);
        let ty_vars = ty.free_vars();
        let vars: Vec<u32> = ty_vars
            .into_iter()
            .filter(|v| !env_vars.contains(v))
            .collect();
        Scheme { vars, ty }
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Type {
        let scheme = self.apply_scheme(scheme);
        let mapping: HashMap<u32, Type> = scheme.vars.iter().map(|&v| (v, self.fresh())).collect();
        self.substitute_vars(&scheme.ty, &mapping)
    }

    fn substitute_vars(&self, ty: &Type, mapping: &HashMap<u32, Type>) -> Type {
        #![allow(clippy::only_used_in_recursion)]
        match ty {
            Type::Var(v) => mapping.get(v).cloned().unwrap_or_else(|| ty.clone()),
            Type::Arrow(a, b) => Type::arrow(
                self.substitute_vars(a, mapping),
                self.substitute_vars(b, mapping),
            ),
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

    fn free_vars_in_env(&self, exclude_top_scope: bool) -> HashSet<u32> {
        let mut vars = HashSet::new();
        let limit = if exclude_top_scope {
            self.scopes.len() - 1
        } else {
            self.scopes.len()
        };
        for scope in &self.scopes[..limit] {
            for scheme in scope.values() {
                let scheme = self.apply_scheme(scheme);
                let fv = scheme.ty.free_vars();
                for v in fv {
                    if !scheme.vars.contains(&v) {
                        vars.insert(v);
                    }
                }
            }
        }
        vars
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
        match &decl.kind {
            DeclKind::Val(pat, expr) => {
                let expr_ty = self.infer_expr(expr)?;
                let (pat_ty, bindings) = self.infer_pat(pat)?;
                self.unify(&expr_ty, &pat_ty, decl.span)?;
                let can_gen = is_syntactic_value(expr);
                for (name, ty) in bindings {
                    if self.constructors.contains_key(&name) {
                        return Err(self.err(
                            &format!("cannot shadow constructor '{name}' with a value binding"),
                            decl.span,
                        ));
                    }
                    let scheme = if can_gen {
                        self.generalize(&ty, false)
                    } else {
                        Scheme::mono(self.apply(&ty))
                    };
                    self.bind(name, scheme);
                }
                Ok(())
            }
            DeclKind::ValRec(sym, expr) => {
                // Push scope for the monomorphic self-binding, generalize excluding it
                let name = self.interner.resolve(*sym).to_string();
                self.push_scope();
                let func_var = self.fresh();
                self.bind(name.clone(), Scheme::mono(func_var.clone()));
                let expr_ty = self.infer_expr(expr)?;
                self.unify(&func_var, &expr_ty, decl.span)?;
                let scheme = self.generalize(&func_var, true);
                self.pop_scope();
                self.bind(name, scheme);
                Ok(())
            }
            DeclKind::Fun(bindings) => self.infer_fun_bindings(bindings, decl.span),
            DeclKind::Datatype(dt) => self.register_datatype(dt),
            DeclKind::TypeAlias(ta) => self.register_type_alias(ta),
            DeclKind::AbstractType(dt) => self.register_abstract_type(dt),
            DeclKind::ExportVal {
                public_name,
                internal_name,
                ty,
            } => {
                let internal = self.interner.resolve(*internal_name).to_string();
                if self.lookup(&internal).is_none() {
                    return Err(self.err(
                        &format!(
                            "unbound variable: {}",
                            self.interner.resolve(*internal_name)
                        ),
                        decl.span,
                    ));
                }

                let mut tyvar_map = HashMap::new();
                let export_ty = self.resolve_type_expr(ty, &mut tyvar_map)?;
                let scheme = Scheme {
                    vars: export_ty.free_vars(),
                    ty: export_ty,
                };
                let public = self.interner.resolve(*public_name).to_string();
                if self.constructors.contains_key(&public) {
                    return Err(self.err(
                        &format!("cannot shadow constructor '{public}' with a value binding"),
                        decl.span,
                    ));
                }
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
            DeclKind::Use(_) => Ok(()),
            DeclKind::Signature(_) => unreachable!("signatures must be removed before inference"),
            DeclKind::Structure { .. } => {
                unreachable!("structures must be flattened before inference")
            }
            DeclKind::Effect(sym, payload) => {
                let name = self.interner.resolve(*sym).to_string();
                let arg_type = if let Some(ty_expr) = payload {
                    let mut tyvar_map = HashMap::new();
                    self.resolve_type_expr(ty_expr, &mut tyvar_map)?
                } else {
                    Type::unit()
                };
                let result_type = self.fresh();
                self.effect_sigs.insert(name, (arg_type, result_type));
                Ok(())
            }
        }
    }

    fn infer_fun_bindings(&mut self, bindings: &[FunBinding], span: Span) -> Result<(), TypeError> {
        self.push_scope();

        // Add monomorphic bindings for all functions (enables mutual recursion)
        let func_vars: Vec<Type> = bindings
            .iter()
            .map(|b| {
                let v = self.fresh();
                let name = self.interner.resolve(b.name).to_string();
                self.bind(name, Scheme::mono(v.clone()));
                v
            })
            .collect();

        // Infer each function's type
        for (binding, func_var) in bindings.iter().zip(func_vars.iter()) {
            let func_ty = self.infer_fun_binding(binding)?;
            self.unify(func_var, &func_ty, binding.span)?;
        }

        // Generalize (exclude the current scope with monomorphic bindings)
        let schemes: Vec<(String, Scheme)> = bindings
            .iter()
            .zip(func_vars.iter())
            .map(|(b, v)| {
                let name = self.interner.resolve(b.name).to_string();
                (name, self.generalize(v, true))
            })
            .collect();

        self.pop_scope();

        for (name, scheme) in schemes {
            self.bind(name, scheme);
        }
        let _ = span;
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
            ty = Type::arrow(arg, ty);
        }
        Ok(ty)
    }

    fn register_datatype(&mut self, dt: &DatatypeDecl) -> Result<(), TypeError> {
        let dt_name = self.interner.resolve(dt.name).to_string();
        if self.type_arities.contains_key(&dt_name) {
            return Err(self.err(&format!("duplicate type name: {dt_name}"), dt.span));
        }
        let arity = dt.tyvars.len();
        self.type_arities.insert(dt_name.clone(), arity);

        // Create type variables for the parameters
        let param_vars: Vec<(String, u32)> = dt
            .tyvars
            .iter()
            .map(|tv| {
                let v = self.next_var;
                self.next_var += 1;
                (self.interner.resolve(*tv).to_string(), v)
            })
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

        let all_vars: Vec<u32> = param_vars.iter().map(|(_, v)| *v).collect();

        for con in &dt.constructors {
            let con_name = self.interner.resolve(con.name).to_string();
            if self.constructors.contains_key(&con_name) {
                return Err(self.err(&format!("duplicate constructor name: {con_name}"), con.span));
            }
            let con_ty = if let Some(ref payload_texpr) = con.payload {
                let tyvar_map: HashMap<String, u32> = param_vars.iter().cloned().collect();
                let payload_ty =
                    self.resolve_type_expr_strict(payload_texpr, &mut tyvar_map.clone())?;
                Type::arrow(payload_ty, result_ty.clone())
            } else {
                result_ty.clone()
            };
            let scheme = Scheme {
                vars: all_vars.clone(),
                ty: con_ty,
            };
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
        if self.type_arities.contains_key(&ta_name) || self.type_aliases.contains_key(&ta_name) {
            return Err(self.err(&format!("duplicate type name: {ta_name}"), ta.span));
        }
        let mut tyvar_map: HashMap<String, u32> = HashMap::new();
        let mut param_vars = Vec::new();
        for tv in &ta.tyvars {
            let v = self.next_var;
            self.next_var += 1;
            tyvar_map.insert(self.interner.resolve(*tv).to_string(), v);
            param_vars.push(v);
        }
        let body = self.resolve_type_expr_strict(&ta.ty, &mut tyvar_map)?;
        self.type_aliases
            .insert(ta_name.clone(), (param_vars, body));
        self.type_arities.insert(ta_name, ta.tyvars.len());
        Ok(())
    }

    fn register_abstract_type(&mut self, dt: &AbstractTypeDecl) -> Result<(), TypeError> {
        let name = self.interner.resolve(dt.name).to_string();
        if self.type_arities.contains_key(&name) || self.type_aliases.contains_key(&name) {
            return Err(self.err(&format!("duplicate type name: {name}"), dt.span));
        }

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
        }

        self.type_arities.insert(name, dt.tyvars.len());
        Ok(())
    }

    // ── Expression inference ─────────────────────────────────────────

    fn infer_expr(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        match &expr.kind {
            ExprKind::IntLit(_) => Ok(Type::int()),
            ExprKind::FloatLit(_) => Ok(Type::float()),
            ExprKind::StringLit(_) => Ok(Type::string()),
            ExprKind::CharLit(_) => Ok(Type::char()),
            ExprKind::BoolLit(_) => Ok(Type::bool()),
            ExprKind::Unit => Ok(Type::unit()),

            ExprKind::Var(sym) => {
                let name = self.interner.resolve(*sym);
                if let Some(scheme) = self.lookup(name).cloned() {
                    Ok(self.instantiate(&scheme))
                } else {
                    Err(self.err(&format!("unbound variable: {name}"), expr.span))
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
                assert!(
                    elems.is_empty(),
                    "non-empty list should be desugared to Cons"
                );
                Ok(Type::list(self.fresh()))
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
                let expected = Type::arrow(arg_ty, result.clone());
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
                Ok(Type::arrow(param_ty, body_ty))
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
                let mut tyvar_map = HashMap::new();
                let declared = self.resolve_type_expr(ty_expr, &mut tyvar_map)?;
                self.unify(&inferred, &declared, expr.span)?;
                Ok(declared)
            }

            ExprKind::Paren(e) => self.infer_expr(e),

            ExprKind::Perform(sym, arg) => {
                let name = self.interner.resolve(*sym).to_string();
                let arg_ty = self.infer_expr(arg)?;
                if let Some((declared_arg, _declared_res)) = self.effect_sigs.get(&name).cloned() {
                    self.unify(&arg_ty, &declared_arg, expr.span)?;
                    Ok(self.fresh())
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
                let _body_ty = self.infer_expr(body)?;
                self.push_scope();
                let ret_var = self.fresh();
                let return_var_name = self.interner.resolve(*return_var).to_string();
                self.bind(return_var_name, Scheme::mono(ret_var));
                let return_ty = self.infer_expr(return_body)?;
                self.pop_scope();
                for handler in handlers {
                    self.push_scope();
                    let effect_name = self.interner.resolve(handler.effect_name).to_string();
                    let payload_ty = if let Some((declared_arg, _)) =
                        self.effect_sigs.get(&effect_name).cloned()
                    {
                        declared_arg
                    } else {
                        self.fresh()
                    };
                    let payload_var_name = self.interner.resolve(handler.payload_var).to_string();
                    self.bind(payload_var_name, Scheme::mono(payload_ty));
                    let resume_arg = self.fresh();
                    let cont_ty = Type::arrow(resume_arg, return_ty.clone());
                    let cont_var_name = self.interner.resolve(handler.cont_var).to_string();
                    self.bind(cont_var_name, Scheme::mono(cont_ty));
                    let handler_ty = self.infer_expr(&handler.body)?;
                    self.unify(&handler_ty, &return_ty, handler.body.span)?;
                    self.pop_scope();
                }
                Ok(return_ty)
            }

            ExprKind::Resume(cont, arg) => {
                let _cont_ty = self.infer_expr(cont)?;
                let _arg_ty = self.infer_expr(arg)?;
                Ok(self.fresh())
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
            BinOp::AddInt | BinOp::SubInt | BinOp::MulInt | BinOp::DivInt | BinOp::ModInt => {
                (Type::int(), Type::int(), Type::int())
            }
            BinOp::AddFloat | BinOp::SubFloat | BinOp::MulFloat | BinOp::DivFloat => {
                (Type::float(), Type::float(), Type::float())
            }
            BinOp::ConcatStr => (Type::string(), Type::string(), Type::string()),
            BinOp::LtInt | BinOp::GtInt | BinOp::LeInt | BinOp::GeInt => {
                (Type::int(), Type::int(), Type::bool())
            }
            BinOp::LtFloat | BinOp::GtFloat | BinOp::LeFloat | BinOp::GeFloat => {
                (Type::float(), Type::float(), Type::bool())
            }
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
                        let expected = Type::arrow(payload_ty, result.clone());
                        self.unify(&con_ty, &expected, pat.span)?;
                        Ok((result, bindings))
                    }
                }
            }

            PatKind::Ann(p, ty_expr) => {
                let (pat_ty, bindings) = self.infer_pat_inner(p)?;
                let mut tyvar_map = HashMap::new();
                let declared = self.resolve_type_expr(ty_expr, &mut tyvar_map)?;
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

    fn resolve_type_expr_strict(
        &mut self,
        ty_expr: &TypeExpr,
        tyvar_map: &mut HashMap<String, u32>,
    ) -> Result<Type, TypeError> {
        self.resolve_type_expr_inner(ty_expr, tyvar_map, true)
    }

    fn resolve_type_expr(
        &mut self,
        ty_expr: &TypeExpr,
        tyvar_map: &mut HashMap<String, u32>,
    ) -> Result<Type, TypeError> {
        self.resolve_type_expr_inner(ty_expr, tyvar_map, false)
    }

    fn resolve_type_expr_inner(
        &mut self,
        ty_expr: &TypeExpr,
        tyvar_map: &mut HashMap<String, u32>,
        strict_tyvars: bool,
    ) -> Result<Type, TypeError> {
        match &ty_expr.kind {
            TypeExprKind::Named(sym) => {
                let name = self.interner.resolve(*sym);
                // Check built-in types
                match name {
                    "int" => return Ok(Type::int()),
                    "float" => return Ok(Type::float()),
                    "bool" => return Ok(Type::bool()),
                    "string" => return Ok(Type::string()),
                    "char" => return Ok(Type::char()),
                    "unit" => return Ok(Type::unit()),
                    "bytes" => return Ok(Type::bytes()),
                    "rng" => return Ok(Type::rng()),
                    "pid" => return Ok(Type::pid()),
                    _ => {}
                }
                if let Some((_, lowercase)) = UPPERCASE_PRIMITIVE_ALIASES
                    .iter()
                    .find(|(uppercase, _)| *uppercase == name)
                {
                    return Err(self.err(
                        &format!(
                            "primitive type '{name}' is no longer supported; use '{lowercase}'"
                        ),
                        ty_expr.span,
                    ));
                }
                // Check type aliases (0-arg)
                if let Some((params, body)) = self.type_aliases.get(name).cloned() {
                    if params.is_empty() {
                        return Ok(body);
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
                if let Some(&v) = tyvar_map.get(name) {
                    Ok(Type::Var(v))
                } else if strict_tyvars {
                    Err(self.err(&format!("unbound type variable: {name}"), ty_expr.span))
                } else {
                    let v = self.next_var;
                    self.next_var += 1;
                    tyvar_map.insert(name.to_string(), v);
                    Ok(Type::Var(v))
                }
            }
            TypeExprKind::App(sym, args) => {
                let name = self.interner.resolve(*sym).to_string();
                let resolved_args: Vec<Type> = args
                    .iter()
                    .map(|a| self.resolve_type_expr_inner(a, tyvar_map, strict_tyvars))
                    .collect::<Result<_, _>>()?;
                // Check alias
                if let Some((param_vars, body)) = self.type_aliases.get(&name).cloned() {
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
                    let mapping: HashMap<u32, Type> = param_vars
                        .iter()
                        .zip(resolved_args.iter())
                        .map(|(&v, arg)| (v, arg.clone()))
                        .collect();
                    return Ok(self.substitute_vars(&body, &mapping));
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
                let a = self.resolve_type_expr_inner(a, tyvar_map, strict_tyvars)?;
                let b = self.resolve_type_expr_inner(b, tyvar_map, strict_tyvars)?;
                Ok(Type::arrow(a, b))
            }
            TypeExprKind::Tuple(ts) => {
                let resolved: Vec<Type> = ts
                    .iter()
                    .map(|t| self.resolve_type_expr_inner(t, tyvar_map, strict_tyvars))
                    .collect::<Result<_, _>>()?;
                Ok(Type::Tuple(resolved))
            }
            TypeExprKind::Paren(t) => self.resolve_type_expr_inner(t, tyvar_map, strict_tyvars),
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn type_info_for(&self, ty: &Type) -> TypeInfo {
        match ty {
            Type::Con(name) => match name.as_str() {
                "Bool" => TypeInfo::bool_type(),
                "Unit" => TypeInfo::unit_type(),
                "Int" | "Float" | "String" | "Char" => TypeInfo::infinite(),
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
            Type::Arrow(_, _) => TypeInfo::infinite(),
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

    /// Get the inferred type of a binding by name (for testing/REPL).
    pub fn lookup_type(&self, name: &str) -> Option<Scheme> {
        self.lookup(name).map(|s| self.apply_scheme(s))
    }
}

fn is_syntactic_value(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::CharLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Unit
        | ExprKind::Var(_)
        | ExprKind::Constructor(_)
        | ExprKind::Fn(_, _) => true,
        ExprKind::Tuple(elems) => elems.iter().all(is_syntactic_value),
        ExprKind::List(elems) => elems.is_empty(),
        ExprKind::Cons(hd, tl) => is_syntactic_value(hd) && is_syntactic_value(tl),
        ExprKind::App(func, arg) => {
            matches!(&func.kind, ExprKind::Constructor(_)) && is_syntactic_value(arg)
        }
        ExprKind::Ann(e, _) => is_syntactic_value(e),
        _ => false,
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
        let ctx = infer("val x = 1.0 +. 2.0");
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
    fn test_equality_polymorphic_allowed() {
        let ctx = infer("fun f x y = x = y");
        let ty = type_of(&ctx, "f");
        // Type variable names vary; check the shape: 'X -> 'X -> bool
        assert!(ty.ends_with("-> bool"), "got: {ty}");
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
