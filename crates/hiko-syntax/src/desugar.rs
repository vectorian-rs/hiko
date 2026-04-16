use crate::ast::*;
use crate::intern::{StringInterner, Symbol};
use crate::span::Span;
use std::collections::{HashMap, HashSet};

pub fn desugar_program(mut program: Program) -> Program {
    let mut interner = std::mem::take(&mut program.interner);
    let signatures = collect_signatures(&program.decls);
    let mut decls = Vec::new();
    for decl in program.decls {
        match decl.kind {
            DeclKind::Signature(_) => {}
            DeclKind::Structure {
                name,
                signature,
                opaque: _,
                decls: body,
            } => {
                let module_name = interner.resolve(name).to_string();
                let signature = signature.and_then(|sig| signatures.get(&sig));
                decls.extend(desugar_structure(body, &[module_name], signature, &mut interner));
            }
            _ => decls.push(desugar_decl(decl, &mut interner)),
        }
    }
    program.decls = decls;
    program.interner = interner;
    program
}

fn collect_signatures(decls: &[Decl]) -> HashMap<Symbol, SignatureDecl> {
    let mut signatures = HashMap::new();
    for decl in decls {
        if let DeclKind::Signature(sig) = &decl.kind {
            signatures.insert(sig.name, sig.clone());
        }
    }
    signatures
}

pub fn desugar_decl(decl: Decl, interner: &mut StringInterner) -> Decl {
    let span = decl.span;
    let kind = match decl.kind {
        DeclKind::Val(pat, expr) => {
            DeclKind::Val(desugar_pat(pat), desugar_expr(expr, interner))
        }
        DeclKind::ValRec(name, expr) => DeclKind::ValRec(name, desugar_expr(expr, interner)),
        DeclKind::Fun(bindings) => {
            let bindings = bindings
                .into_iter()
                .map(|b| desugar_fun_binding(b, interner))
                .collect();
            DeclKind::Fun(bindings)
        }
        DeclKind::Datatype(dt) => DeclKind::Datatype(dt),
        DeclKind::TypeAlias(ta) => DeclKind::TypeAlias(ta),
        DeclKind::Local(locals, body) => DeclKind::Local(
            locals
                .into_iter()
                .map(|decl| desugar_decl(decl, interner))
                .collect(),
            body.into_iter()
                .map(|decl| desugar_decl(decl, interner))
                .collect(),
        ),
        DeclKind::Use(path) => DeclKind::Use(path),
        DeclKind::Signature(_) => unreachable!("signatures are removed before decl desugaring"),
        DeclKind::Structure { .. } => {
            unreachable!("structures are flattened before decl desugaring")
        }
        DeclKind::Effect(name, ty) => DeclKind::Effect(name, ty),
    };
    Decl { kind, span }
}

#[derive(Clone, Default)]
struct ModuleScope {
    values: HashMap<Symbol, Symbol>,
    constructors: HashMap<Symbol, Symbol>,
    types: HashMap<Symbol, Symbol>,
    effects: HashMap<Symbol, Symbol>,
}

fn desugar_structure(
    decls: Vec<Decl>,
    module_path: &[String],
    signature: Option<&SignatureDecl>,
    interner: &mut StringInterner,
) -> Vec<Decl> {
    let scope = collect_module_scope(&decls, module_path, interner, signature.is_some());
    let mut out = Vec::new();
    let empty_values = HashSet::new();
    for decl in decls {
        out.extend(rename_module_decl(
            decl,
            &scope,
            &empty_values,
            signature.is_some(),
            !signature.is_some(),
        ));
    }
    let mut out: Vec<Decl> = out.into_iter().map(|decl| desugar_decl(decl, interner)).collect();
    if let Some(signature) = signature {
        out.extend(signature_assertions(signature, &scope, module_path, interner));
    }
    out
}

fn collect_module_scope(
    decls: &[Decl],
    module_path: &[String],
    interner: &mut StringInterner,
    hide_exports: bool,
) -> ModuleScope {
    let mut scope = ModuleScope::default();
    for decl in decls {
        collect_decl_exports(decl, &mut scope, module_path, interner, hide_exports);
    }
    scope
}

fn collect_decl_exports(
    decl: &Decl,
    scope: &mut ModuleScope,
    module_path: &[String],
    interner: &mut StringInterner,
    hide_exports: bool,
) {
    match &decl.kind {
        DeclKind::Val(pat, _) => {
            for sym in pat_bound_names(pat) {
                scope.values
                    .insert(sym, mangle_symbol(module_path, sym, interner, hide_exports));
            }
        }
        DeclKind::ValRec(sym, _) => {
            scope
                .values
                .insert(*sym, mangle_symbol(module_path, *sym, interner, hide_exports));
        }
        DeclKind::Fun(bindings) => {
            for binding in bindings {
                scope.values.insert(
                    binding.name,
                    mangle_symbol(module_path, binding.name, interner, hide_exports),
                );
            }
        }
        DeclKind::Datatype(dt) => {
            scope
                .types
                .insert(dt.name, mangle_symbol(module_path, dt.name, interner, hide_exports));
            for con in &dt.constructors {
                scope.constructors.insert(
                    con.name,
                    mangle_symbol(module_path, con.name, interner, hide_exports),
                );
            }
        }
        DeclKind::TypeAlias(ta) => {
            scope
                .types
                .insert(ta.name, mangle_symbol(module_path, ta.name, interner, hide_exports));
        }
        DeclKind::Local(_, body) => {
            for decl in body {
                collect_decl_exports(decl, scope, module_path, interner, hide_exports);
            }
        }
        DeclKind::Signature(_) => {}
        DeclKind::Use(_) => {}
        DeclKind::Structure { .. } => {}
        DeclKind::Effect(sym, _) => {
            scope
                .effects
                .insert(*sym, mangle_symbol(module_path, *sym, interner, hide_exports));
        }
    }
}

fn mangle_symbol(
    module_path: &[String],
    sym: Symbol,
    interner: &mut StringInterner,
    hidden: bool,
) -> Symbol {
    let mut qualified = module_path.join(".");
    if !qualified.is_empty() {
        qualified.push('.');
    }
    if hidden {
        qualified.push('$');
    }
    qualified.push_str(interner.resolve(sym));
    interner.intern(&qualified)
}

fn signature_assertions(
    signature: &SignatureDecl,
    scope: &ModuleScope,
    module_path: &[String],
    interner: &mut StringInterner,
) -> Vec<Decl> {
    let mut out = Vec::new();
    for spec in &signature.specs {
        match spec {
            SignatureSpec::Val { name, ty, span } => {
                let public_sym = mangle_symbol(module_path, *name, interner, false);
                let checked_sym = scope.values.get(name).copied().unwrap_or(public_sym);
                let checked_expr = Expr {
                    kind: ExprKind::Ann(
                        Box::new(Expr {
                            kind: ExprKind::Var(checked_sym),
                            span: *span,
                        }),
                        ty.clone(),
                    ),
                    span: *span,
                };
                out.push(Decl {
                    kind: DeclKind::Val(
                        Pat {
                            kind: PatKind::Wildcard,
                            span: *span,
                        },
                        checked_expr,
                    ),
                    span: *span,
                });
                if let Some(internal_sym) = scope.values.get(name).copied() {
                    out.push(Decl {
                        kind: DeclKind::Val(
                            Pat {
                                kind: PatKind::Var(public_sym),
                                span: *span,
                            },
                            Expr {
                                kind: ExprKind::Var(internal_sym),
                                span: *span,
                            },
                        ),
                        span: *span,
                    });
                }
            }
        }
    }
    out
}

fn rename_module_decl(
    decl: Decl,
    scope: &ModuleScope,
    local_values: &HashSet<Symbol>,
    module_hidden: bool,
    export_names: bool,
) -> Vec<Decl> {
    match decl.kind {
            DeclKind::Structure { .. } => unreachable!("nested structures are not supported yet"),
        DeclKind::Val(pat, expr) => vec![Decl {
            kind: DeclKind::Val(
                rename_pat(pat, scope, export_names || module_hidden),
                rename_expr(expr, scope, local_values),
            ),
            span: decl.span,
        }],
        DeclKind::ValRec(sym, expr) => {
            let mut env = local_values.clone();
            if !export_names && !module_hidden {
                env.insert(sym);
            }
            vec![Decl {
                kind: DeclKind::ValRec(
                    if export_names || module_hidden {
                        *scope.values.get(&sym).unwrap_or(&sym)
                    } else {
                        sym
                    },
                    rename_expr(expr, scope, &env),
                ),
                span: decl.span,
            }]
        }
        DeclKind::Fun(bindings) => {
            let recursive_values = if export_names || module_hidden {
                HashSet::new()
            } else {
                bindings.iter().map(|binding| binding.name).collect()
            };
            let bindings = bindings
                .into_iter()
                .map(|binding| {
                    rename_fun_binding(
                        binding,
                        scope,
                        local_values,
                        &recursive_values,
                        export_names || module_hidden,
                    )
                })
                .collect();
            vec![Decl {
                kind: DeclKind::Fun(bindings),
                span: decl.span,
            }]
        }
        DeclKind::Datatype(mut dt) => {
            if export_names || module_hidden {
                dt.name = *scope.types.get(&dt.name).unwrap_or(&dt.name);
                for con in &mut dt.constructors {
                    con.name = *scope.constructors.get(&con.name).unwrap_or(&con.name);
                    if let Some(payload) = con.payload.take() {
                        con.payload = Some(rename_type_expr(payload, scope));
                    }
                }
            } else {
                for con in &mut dt.constructors {
                    if let Some(payload) = con.payload.take() {
                        con.payload = Some(rename_type_expr(payload, scope));
                    }
                }
            }
            vec![Decl {
                kind: DeclKind::Datatype(dt),
                span: decl.span,
            }]
        }
        DeclKind::TypeAlias(mut ta) => {
            if export_names || module_hidden {
                ta.name = *scope.types.get(&ta.name).unwrap_or(&ta.name);
            }
            ta.ty = rename_type_expr(ta.ty, scope);
            vec![Decl {
                kind: DeclKind::TypeAlias(ta),
                span: decl.span,
            }]
        }
        DeclKind::Local(locals, body) => {
            let locals = rename_local_decl_seq(locals, scope, local_values);
            let body = if export_names {
                rename_export_decl_seq(body, scope, local_values, false, true)
            } else if module_hidden {
                rename_export_decl_seq(body, scope, local_values, true, false)
            } else {
                rename_local_decl_seq(body, scope, local_values)
            };
            vec![Decl {
                kind: DeclKind::Local(locals, body),
                span: decl.span,
            }]
        }
        DeclKind::Use(path) => vec![Decl {
            kind: DeclKind::Use(path),
            span: decl.span,
        }],
        DeclKind::Signature(_) => vec![],
        DeclKind::Effect(sym, ty) => vec![Decl {
            kind: DeclKind::Effect(
                if export_names || module_hidden {
                    *scope.effects.get(&sym).unwrap_or(&sym)
                } else {
                    sym
                },
                ty.map(|ty| rename_type_expr(ty, scope)),
            ),
            span: decl.span,
        }],
    }
}

fn rename_export_decl_seq(
    decls: Vec<Decl>,
    scope: &ModuleScope,
    local_values: &HashSet<Symbol>,
    module_hidden: bool,
    export_names: bool,
) -> Vec<Decl> {
    let mut out = Vec::new();
    for decl in decls {
        out.extend(rename_module_decl(
            decl,
            scope,
            local_values,
            module_hidden,
            export_names,
        ));
    }
    out
}

fn rename_local_decl_seq(
    decls: Vec<Decl>,
    scope: &ModuleScope,
    local_values: &HashSet<Symbol>,
) -> Vec<Decl> {
    let mut out = Vec::new();
    let mut env = local_values.clone();
    for decl in decls {
        let bound = local_decl_value_names(&decl);
        out.extend(rename_module_decl(decl, scope, &env, false, false));
        env.extend(bound);
    }
    out
}

fn rename_fun_binding(
    mut binding: FunBinding,
    scope: &ModuleScope,
    local_values: &HashSet<Symbol>,
    recursive_values: &HashSet<Symbol>,
    export_names: bool,
) -> FunBinding {
    let renamed_name = if export_names {
        *scope.values.get(&binding.name).unwrap_or(&binding.name)
    } else {
        binding.name
    };
    for clause in &mut binding.clauses {
        let mut env = local_values.clone();
        env.extend(recursive_values.iter().copied());
        for pat in &clause.pats {
            env.extend(pat_bound_names(pat));
        }
        clause.body = rename_expr(
            std::mem::replace(&mut clause.body, unit_expr(clause.span)),
            scope,
            &env,
        );
        clause.pats = clause
            .pats
            .drain(..)
            .map(|pat| rename_pat(pat, scope, false))
            .collect();
    }
    binding.name = renamed_name;
    binding
}

fn rename_expr(expr: Expr, scope: &ModuleScope, local_values: &HashSet<Symbol>) -> Expr {
    let span = expr.span;
    let kind = match expr.kind {
        ExprKind::Var(sym) => ExprKind::Var(rename_value_symbol(sym, scope, local_values)),
        ExprKind::Constructor(sym) => {
            ExprKind::Constructor(*scope.constructors.get(&sym).unwrap_or(&sym))
        }
        ExprKind::Tuple(elems) => ExprKind::Tuple(
            elems
                .into_iter()
                .map(|expr| rename_expr(expr, scope, local_values))
                .collect(),
        ),
        ExprKind::List(elems) => ExprKind::List(
            elems
                .into_iter()
                .map(|expr| rename_expr(expr, scope, local_values))
                .collect(),
        ),
        ExprKind::Cons(hd, tl) => ExprKind::Cons(
            Box::new(rename_expr(*hd, scope, local_values)),
            Box::new(rename_expr(*tl, scope, local_values)),
        ),
        ExprKind::BinOp(op, lhs, rhs) => ExprKind::BinOp(
            op,
            Box::new(rename_expr(*lhs, scope, local_values)),
            Box::new(rename_expr(*rhs, scope, local_values)),
        ),
        ExprKind::UnaryNeg(expr) => {
            ExprKind::UnaryNeg(Box::new(rename_expr(*expr, scope, local_values)))
        }
        ExprKind::Not(expr) => ExprKind::Not(Box::new(rename_expr(*expr, scope, local_values))),
        ExprKind::App(func, arg) => ExprKind::App(
            Box::new(rename_expr(*func, scope, local_values)),
            Box::new(rename_expr(*arg, scope, local_values)),
        ),
        ExprKind::Fn(pat, body) => {
            let mut env = local_values.clone();
            env.extend(pat_bound_names(&pat));
            ExprKind::Fn(
                rename_pat(pat, scope, false),
                Box::new(rename_expr(*body, scope, &env)),
            )
        }
        ExprKind::If(cond, then_br, else_br) => ExprKind::If(
            Box::new(rename_expr(*cond, scope, local_values)),
            Box::new(rename_expr(*then_br, scope, local_values)),
            Box::new(rename_expr(*else_br, scope, local_values)),
        ),
        ExprKind::Let(decls, body) => {
            let decls = rename_local_decl_seq(decls, scope, local_values);
            let mut env = local_values.clone();
            for decl in &decls {
                env.extend(local_decl_value_names(decl));
            }
            ExprKind::Let(decls, Box::new(rename_expr(*body, scope, &env)))
        }
        ExprKind::Case(scrutinee, arms) => ExprKind::Case(
            Box::new(rename_expr(*scrutinee, scope, local_values)),
            arms.into_iter()
                .map(|(pat, body)| {
                    let mut env = local_values.clone();
                    env.extend(pat_bound_names(&pat));
                    (rename_pat(pat, scope, false), rename_expr(body, scope, &env))
                })
                .collect(),
        ),
        ExprKind::Ann(expr, ty) => {
            ExprKind::Ann(Box::new(rename_expr(*expr, scope, local_values)), rename_type_expr(ty, scope))
        }
        ExprKind::Perform(sym, arg) => ExprKind::Perform(
            *scope.effects.get(&sym).unwrap_or(&sym),
            Box::new(rename_expr(*arg, scope, local_values)),
        ),
        ExprKind::Handle {
            body,
            return_var,
            return_body,
            handlers,
        } => {
            let body = Box::new(rename_expr(*body, scope, local_values));
            let mut return_env = local_values.clone();
            return_env.insert(return_var);
            let return_body = Box::new(rename_expr(*return_body, scope, &return_env));
            let handlers = handlers
                .into_iter()
                .map(|handler| {
                    let mut env = local_values.clone();
                    env.insert(handler.payload_var);
                    env.insert(handler.cont_var);
                    EffectHandler {
                        effect_name: *scope
                            .effects
                            .get(&handler.effect_name)
                            .unwrap_or(&handler.effect_name),
                        body: rename_expr(handler.body, scope, &env),
                        ..handler
                    }
                })
                .collect();
            ExprKind::Handle {
                body,
                return_var,
                return_body,
                handlers,
            }
        }
        ExprKind::Resume(cont, arg) => ExprKind::Resume(
            Box::new(rename_expr(*cont, scope, local_values)),
            Box::new(rename_expr(*arg, scope, local_values)),
        ),
        other => other,
    };
    Expr { kind, span }
}

fn rename_pat(pat: Pat, scope: &ModuleScope, export_names: bool) -> Pat {
    let span = pat.span;
    let kind = match pat.kind {
        PatKind::Var(sym) => PatKind::Var(if export_names {
            *scope.values.get(&sym).unwrap_or(&sym)
        } else {
            sym
        }),
        PatKind::Tuple(pats) => {
            PatKind::Tuple(pats.into_iter().map(|pat| rename_pat(pat, scope, export_names)).collect())
        }
        PatKind::Constructor(sym, payload) => PatKind::Constructor(
            *scope.constructors.get(&sym).unwrap_or(&sym),
            payload.map(|payload| Box::new(rename_pat(*payload, scope, false))),
        ),
        PatKind::Cons(hd, tl) => PatKind::Cons(
            Box::new(rename_pat(*hd, scope, export_names)),
            Box::new(rename_pat(*tl, scope, export_names)),
        ),
        PatKind::List(pats) => {
            PatKind::List(pats.into_iter().map(|pat| rename_pat(pat, scope, export_names)).collect())
        }
        PatKind::Ann(pat, ty) => PatKind::Ann(
            Box::new(rename_pat(*pat, scope, export_names)),
            rename_type_expr(ty, scope),
        ),
        PatKind::As(sym, pat) => PatKind::As(
            if export_names {
                *scope.values.get(&sym).unwrap_or(&sym)
            } else {
                sym
            },
            Box::new(rename_pat(*pat, scope, export_names)),
        ),
        other => other,
    };
    Pat { kind, span }
}

fn rename_type_expr(ty: TypeExpr, scope: &ModuleScope) -> TypeExpr {
    let span = ty.span;
    let kind = match ty.kind {
        TypeExprKind::Named(sym) => TypeExprKind::Named(*scope.types.get(&sym).unwrap_or(&sym)),
        TypeExprKind::Var(sym) => TypeExprKind::Var(sym),
        TypeExprKind::App(sym, args) => TypeExprKind::App(
            *scope.types.get(&sym).unwrap_or(&sym),
            args.into_iter().map(|arg| rename_type_expr(arg, scope)).collect(),
        ),
        TypeExprKind::Arrow(lhs, rhs) => TypeExprKind::Arrow(
            Box::new(rename_type_expr(*lhs, scope)),
            Box::new(rename_type_expr(*rhs, scope)),
        ),
        TypeExprKind::Tuple(elems) => TypeExprKind::Tuple(
            elems.into_iter().map(|arg| rename_type_expr(arg, scope)).collect(),
        ),
        TypeExprKind::Paren(inner) => {
            TypeExprKind::Paren(Box::new(rename_type_expr(*inner, scope)))
        }
    };
    TypeExpr { kind, span }
}

fn rename_value_symbol(sym: Symbol, scope: &ModuleScope, local_values: &HashSet<Symbol>) -> Symbol {
    if local_values.contains(&sym) {
        sym
    } else {
        *scope.values.get(&sym).unwrap_or(&sym)
    }
}

fn pat_bound_names(pat: &Pat) -> HashSet<Symbol> {
    let mut out = HashSet::new();
    collect_pat_bound_names(pat, &mut out);
    out
}

fn collect_pat_bound_names(pat: &Pat, out: &mut HashSet<Symbol>) {
    match &pat.kind {
        PatKind::Var(sym) => {
            out.insert(*sym);
        }
        PatKind::Tuple(pats) | PatKind::List(pats) => {
            for pat in pats {
                collect_pat_bound_names(pat, out);
            }
        }
        PatKind::Constructor(_, payload) => {
            if let Some(payload) = payload {
                collect_pat_bound_names(payload, out);
            }
        }
        PatKind::Cons(hd, tl) => {
            collect_pat_bound_names(hd, out);
            collect_pat_bound_names(tl, out);
        }
        PatKind::Ann(inner, _) | PatKind::Paren(inner) => collect_pat_bound_names(inner, out),
        PatKind::As(sym, inner) => {
            out.insert(*sym);
            collect_pat_bound_names(inner, out);
        }
        _ => {}
    }
}

fn local_decl_value_names(decl: &Decl) -> HashSet<Symbol> {
    let mut out = HashSet::new();
    match &decl.kind {
        DeclKind::Val(pat, _) => out.extend(pat_bound_names(pat)),
        DeclKind::ValRec(sym, _) => {
            out.insert(*sym);
        }
        DeclKind::Fun(bindings) => {
            for binding in bindings {
                out.insert(binding.name);
            }
        }
        DeclKind::Local(_, body) => {
            for decl in body {
                out.extend(local_decl_value_names(decl));
            }
        }
        _ => {}
    }
    out
}

fn unit_expr(span: Span) -> Expr {
    Expr {
        kind: ExprKind::Unit,
        span,
    }
}

fn desugar_expr(expr: Expr, interner: &mut StringInterner) -> Expr {
    let span = expr.span;
    let kind = match expr.kind {
        ExprKind::Paren(e) => return desugar_expr(*e, interner),
        ExprKind::List(elems) if !elems.is_empty() => {
            let mut result = Expr {
                kind: ExprKind::List(vec![]),
                span,
            };
            for elem in elems.into_iter().rev() {
                result = Expr {
                    kind: ExprKind::Cons(Box::new(desugar_expr(elem, interner)), Box::new(result)),
                    span,
                };
            }
            return result;
        }
        ExprKind::BinOp(BinOp::Andalso, lhs, rhs) => ExprKind::If(
            Box::new(desugar_expr(*lhs, interner)),
            Box::new(desugar_expr(*rhs, interner)),
            Box::new(Expr {
                kind: ExprKind::BoolLit(false),
                span,
            }),
        ),
        ExprKind::BinOp(BinOp::Orelse, lhs, rhs) => ExprKind::If(
            Box::new(desugar_expr(*lhs, interner)),
            Box::new(Expr {
                kind: ExprKind::BoolLit(true),
                span,
            }),
            Box::new(desugar_expr(*rhs, interner)),
        ),
        ExprKind::Not(e) => ExprKind::If(
            Box::new(desugar_expr(*e, interner)),
            Box::new(Expr {
                kind: ExprKind::BoolLit(false),
                span,
            }),
            Box::new(Expr {
                kind: ExprKind::BoolLit(true),
                span,
            }),
        ),
        ExprKind::BinOp(op, lhs, rhs) => ExprKind::BinOp(
            op,
            Box::new(desugar_expr(*lhs, interner)),
            Box::new(desugar_expr(*rhs, interner)),
        ),
        ExprKind::UnaryNeg(e) => ExprKind::UnaryNeg(Box::new(desugar_expr(*e, interner))),
        ExprKind::App(f, arg) => ExprKind::App(
            Box::new(desugar_expr(*f, interner)),
            Box::new(desugar_expr(*arg, interner)),
        ),
        ExprKind::Fn(pat, body) => ExprKind::Fn(
            desugar_pat(pat),
            Box::new(desugar_expr(*body, interner)),
        ),
        ExprKind::If(c, t, e) => ExprKind::If(
            Box::new(desugar_expr(*c, interner)),
            Box::new(desugar_expr(*t, interner)),
            Box::new(desugar_expr(*e, interner)),
        ),
        ExprKind::Let(decls, body) => ExprKind::Let(
            decls
                .into_iter()
                .map(|decl| desugar_decl(decl, interner))
                .collect(),
            Box::new(desugar_expr(*body, interner)),
        ),
        ExprKind::Case(scrutinee, arms) => ExprKind::Case(
            Box::new(desugar_expr(*scrutinee, interner)),
            arms.into_iter()
                .map(|(pat, body)| (desugar_pat(pat), desugar_expr(body, interner)))
                .collect(),
        ),
        ExprKind::Tuple(elems) => ExprKind::Tuple(
            elems
                .into_iter()
                .map(|expr| desugar_expr(expr, interner))
                .collect(),
        ),
        ExprKind::Cons(hd, tl) => ExprKind::Cons(
            Box::new(desugar_expr(*hd, interner)),
            Box::new(desugar_expr(*tl, interner)),
        ),
        ExprKind::Ann(expr, ty) => ExprKind::Ann(Box::new(desugar_expr(*expr, interner)), ty),
        ExprKind::Perform(name, arg) => {
            ExprKind::Perform(name, Box::new(desugar_expr(*arg, interner)))
        }
        ExprKind::Handle {
            body,
            return_var,
            return_body,
            handlers,
        } => ExprKind::Handle {
            body: Box::new(desugar_expr(*body, interner)),
            return_var,
            return_body: Box::new(desugar_expr(*return_body, interner)),
            handlers: handlers
                .into_iter()
                .map(|handler| EffectHandler {
                    body: desugar_expr(handler.body, interner),
                    ..handler
                })
                .collect(),
        },
        ExprKind::Resume(cont, arg) => ExprKind::Resume(
            Box::new(desugar_expr(*cont, interner)),
            Box::new(desugar_expr(*arg, interner)),
        ),
        other => other,
    };
    Expr { kind, span }
}

fn desugar_pat(pat: Pat) -> Pat {
    let span = pat.span;
    let kind = match pat.kind {
        PatKind::Paren(pat) => return desugar_pat(*pat),
        PatKind::List(pats) if !pats.is_empty() => {
            let mut result = Pat {
                kind: PatKind::List(vec![]),
                span,
            };
            for pat in pats.into_iter().rev() {
                result = Pat {
                    kind: PatKind::Cons(Box::new(desugar_pat(pat)), Box::new(result)),
                    span,
                };
            }
            return result;
        }
        PatKind::Tuple(pats) => PatKind::Tuple(pats.into_iter().map(desugar_pat).collect()),
        PatKind::Constructor(name, payload) => {
            PatKind::Constructor(name, payload.map(|payload| Box::new(desugar_pat(*payload))))
        }
        PatKind::Cons(hd, tl) => {
            PatKind::Cons(Box::new(desugar_pat(*hd)), Box::new(desugar_pat(*tl)))
        }
        PatKind::Ann(pat, ty) => PatKind::Ann(Box::new(desugar_pat(*pat)), ty),
        PatKind::As(name, pat) => PatKind::As(name, Box::new(desugar_pat(*pat))),
        other => other,
    };
    Pat { kind, span }
}

fn desugar_fun_binding(mut binding: FunBinding, interner: &mut StringInterner) -> FunBinding {
    for clause in &mut binding.clauses {
        clause.pats = clause.pats.drain(..).map(desugar_pat).collect();
        clause.body = desugar_expr(
            std::mem::replace(&mut clause.body, unit_expr(clause.span)),
            interner,
        );
    }

    if binding.clauses.len() == 1 {
        return binding;
    }

    let span = binding.span;
    let arity = binding.clauses[0].pats.len();
    let arg_names: Vec<_> = (0..arity)
        .map(|i| interner.intern(&format!("_arg{i}")))
        .collect();

    let arms: Vec<(Pat, Expr)> = binding
        .clauses
        .drain(..)
        .map(|clause| {
            let pat = if arity == 1 {
                clause.pats.into_iter().next().unwrap()
            } else {
                Pat {
                    kind: PatKind::Tuple(clause.pats),
                    span: clause.span,
                }
            };
            (pat, clause.body)
        })
        .collect();

    let scrutinee = if arity == 1 {
        Expr {
            kind: ExprKind::Var(arg_names[0]),
            span,
        }
    } else {
        Expr {
            kind: ExprKind::Tuple(
                arg_names
                    .iter()
                    .map(|name| Expr {
                        kind: ExprKind::Var(*name),
                        span,
                    })
                    .collect(),
            ),
            span,
        }
    };

    let case_expr = Expr {
        kind: ExprKind::Case(Box::new(scrutinee), arms),
        span,
    };

    let pats = arg_names
        .into_iter()
        .map(|name| Pat {
            kind: PatKind::Var(name),
            span,
        })
        .collect();

    binding.clauses = vec![FunClause {
        pats,
        body: case_expr,
        span,
    }];

    binding
}
