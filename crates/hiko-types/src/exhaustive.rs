//! Maranget-style exhaustiveness and usefulness checking for pattern matching.
//!
//! Based on "Warnings for pattern matching" (Maranget, JFP 2007).
//!
//! The algorithm works on a pattern matrix: each row is a pattern vector
//! from a case branch. A pattern vector is "useful" with respect to the
//! matrix if there exists some value matched by the vector but not by any
//! existing row.
//!
//! - A match is exhaustive iff the wildcard vector is NOT useful.
//! - A clause is redundant iff its row is NOT useful against preceding rows.

use std::collections::HashSet;

use hiko_syntax::ast::{Pat, PatKind};
use hiko_syntax::intern::StringInterner;

/// A simplified pattern for the exhaustiveness algorithm.
#[derive(Debug, Clone)]
enum SPat {
    Wild,
    Con(Constructor, Vec<SPat>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Constructor {
    Adt(String, u16),
    BoolTrue,
    BoolFalse,
    Unit,
    Nil,
    Cons,
    Tuple(usize),
    /// Distinct literal value. Each unique value is a separate constructor,
    /// treated as an open (infinite) domain requiring a wildcard fallback.
    IntLit(i64),
    FloatBits(u64), // use bits for Eq/Hash
    StringLit(String),
    CharLit(char),
}

/// Information about a type's constructors for exhaustiveness checking.
#[derive(Debug, Clone)]
pub struct TypeInfo {
    /// All constructors of this type, with their arities.
    constructors: Vec<(Constructor, usize)>,
    /// Whether the type has a finite set of constructors.
    is_finite: bool,
}

impl TypeInfo {
    pub fn bool_type() -> Self {
        TypeInfo {
            constructors: vec![(Constructor::BoolTrue, 0), (Constructor::BoolFalse, 0)],
            is_finite: true,
        }
    }

    pub fn unit_type() -> Self {
        TypeInfo {
            constructors: vec![(Constructor::Unit, 0)],
            is_finite: true,
        }
    }

    pub fn list_type() -> Self {
        TypeInfo {
            constructors: vec![(Constructor::Nil, 0), (Constructor::Cons, 2)],
            is_finite: true,
        }
    }

    pub fn tuple_type(arity: usize) -> Self {
        TypeInfo {
            constructors: vec![(Constructor::Tuple(arity), arity)],
            is_finite: true,
        }
    }

    pub fn adt_type(_type_name: &str, constructors: &[(String, usize)]) -> Self {
        TypeInfo {
            constructors: constructors
                .iter()
                .enumerate()
                .map(|(i, (name, arity))| (Constructor::Adt(name.clone(), i as u16), *arity))
                .collect(),
            is_finite: true,
        }
    }

    /// Infinite domain: Int, Float, String, Char.
    pub fn infinite() -> Self {
        TypeInfo {
            constructors: vec![],
            is_finite: false,
        }
    }
}

/// Result of checking a case expression.
pub struct CheckResult {
    /// True if the match is exhaustive.
    pub exhaustive: bool,
    /// Indices of redundant clauses (0-based).
    pub redundant_clauses: Vec<usize>,
}

/// Convert a surface pattern to a simplified pattern.
fn simplify_pat(
    pat: &Pat,
    con_tags: &std::collections::HashMap<String, u16>,
    interner: &StringInterner,
) -> SPat {
    match &pat.kind {
        PatKind::Wildcard | PatKind::Var(_) => SPat::Wild,

        PatKind::BoolLit(true) => SPat::Con(Constructor::BoolTrue, vec![]),
        PatKind::BoolLit(false) => SPat::Con(Constructor::BoolFalse, vec![]),
        PatKind::Unit => SPat::Con(Constructor::Unit, vec![]),

        PatKind::IntLit(n) => SPat::Con(Constructor::IntLit(*n), vec![]),
        PatKind::FloatLit(f) => SPat::Con(Constructor::FloatBits(f.to_bits()), vec![]),
        PatKind::StringLit(s) => SPat::Con(Constructor::StringLit(s.clone()), vec![]),
        PatKind::CharLit(c) => SPat::Con(Constructor::CharLit(*c), vec![]),

        PatKind::Constructor(sym, payload) => {
            let name = interner.resolve(*sym);
            let tag = con_tags.get(name).copied().unwrap_or(0);
            let type_name = name.to_string();
            let sub_pats = match payload {
                Some(p) => vec![simplify_pat(p, con_tags, interner)],
                None => vec![],
            };
            SPat::Con(Constructor::Adt(type_name, tag), sub_pats)
        }

        PatKind::Tuple(pats) => {
            let sub = pats
                .iter()
                .map(|p| simplify_pat(p, con_tags, interner))
                .collect();
            SPat::Con(Constructor::Tuple(pats.len()), sub)
        }

        PatKind::Cons(hd, tl) => {
            let sub = vec![
                simplify_pat(hd, con_tags, interner),
                simplify_pat(tl, con_tags, interner),
            ];
            SPat::Con(Constructor::Cons, sub)
        }

        PatKind::List(pats) => {
            if pats.is_empty() {
                SPat::Con(Constructor::Nil, vec![])
            } else {
                // [p1, p2, ...] = p1 :: p2 :: ... :: []
                let mut result = SPat::Con(Constructor::Nil, vec![]);
                for p in pats.iter().rev() {
                    result = SPat::Con(
                        Constructor::Cons,
                        vec![simplify_pat(p, con_tags, interner), result],
                    );
                }
                result
            }
        }

        PatKind::As(_, p) => simplify_pat(p, con_tags, interner),
        PatKind::Paren(p) | PatKind::Ann(p, _) => simplify_pat(p, con_tags, interner),
    }
}

type PatternMatrix = Vec<Vec<SPat>>;

/// Check exhaustiveness and redundancy of a pattern match.
///
/// `type_info` describes the constructors of the scrutinee's type.
/// `con_tags` maps constructor names to their tags.
/// `dt_constructors` maps type names to their full constructor lists.
pub fn check_match(
    patterns: &[&Pat],
    type_info: &TypeInfo,
    con_tags: &std::collections::HashMap<String, u16>,
    dt_constructors: &std::collections::HashMap<String, Vec<(String, usize)>>,
    interner: &StringInterner,
) -> CheckResult {
    let matrix: PatternMatrix = patterns
        .iter()
        .map(|p| vec![simplify_pat(p, con_tags, interner)])
        .collect();

    let wildcard = vec![SPat::Wild];
    let exhaustive = !is_useful(
        &matrix,
        &wildcard,
        std::slice::from_ref(type_info),
        dt_constructors,
    );

    let mut redundant_clauses = Vec::new();
    for i in 0..matrix.len() {
        let preceding: PatternMatrix = matrix[..i].to_vec();
        if !is_useful(
            &preceding,
            &matrix[i],
            std::slice::from_ref(type_info),
            dt_constructors,
        ) {
            redundant_clauses.push(i);
        }
    }

    CheckResult {
        exhaustive,
        redundant_clauses,
    }
}

/// Is the pattern vector `q` useful with respect to the matrix `p`?
/// Returns true if there exists a value matched by `q` but not by any row of `p`.
type DtMap = std::collections::HashMap<String, Vec<(String, usize)>>;

fn is_useful(matrix: &PatternMatrix, q: &[SPat], type_infos: &[TypeInfo], dt: &DtMap) -> bool {
    let n = q.len();

    if n == 0 {
        return matrix.is_empty();
    }

    match &q[0] {
        SPat::Con(c, sub_pats) => {
            let s_matrix = specialize_matrix(matrix, c, sub_pats.len());
            let mut s_q: Vec<SPat> = sub_pats.clone();
            s_q.extend_from_slice(&q[1..]);

            let mut s_types = Vec::new();
            let arity = sub_pats.len();
            for i in 0..arity {
                s_types.push(infer_type_info_from_column(&s_matrix, i, dt));
            }
            s_types.extend_from_slice(&type_infos[1..]);

            is_useful(&s_matrix, &s_q, &s_types, dt)
        }

        SPat::Wild => {
            let ti = &type_infos[0];

            if ti.is_finite {
                let head_cons = collect_head_constructors(matrix);
                let all_covered = ti.constructors.iter().all(|(c, _)| head_cons.contains(c));

                if all_covered {
                    for (c, arity) in &ti.constructors {
                        let s_matrix = specialize_matrix(matrix, c, *arity);
                        let mut s_q: Vec<SPat> = (0..*arity).map(|_| SPat::Wild).collect();
                        s_q.extend_from_slice(&q[1..]);

                        let mut s_types = Vec::new();
                        for i in 0..*arity {
                            s_types.push(infer_type_info_from_column(&s_matrix, i, dt));
                        }
                        s_types.extend_from_slice(&type_infos[1..]);

                        if is_useful(&s_matrix, &s_q, &s_types, dt) {
                            return true;
                        }
                    }
                    false
                } else {
                    let d_matrix = default_matrix(matrix);
                    let d_q: Vec<SPat> = q[1..].to_vec();
                    let d_types: Vec<TypeInfo> = type_infos[1..].to_vec();
                    is_useful(&d_matrix, &d_q, &d_types, dt)
                }
            } else {
                // Infinite domain: wildcard is useful iff default matrix says so
                let d_matrix = default_matrix(matrix);
                let d_q: Vec<SPat> = q[1..].to_vec();
                let d_types: Vec<TypeInfo> = type_infos[1..].to_vec();
                is_useful(&d_matrix, &d_q, &d_types, dt)
            }
        }
    }
}

/// Infer TypeInfo for a column, using the datatype map for full constructor lists.
fn infer_type_info_from_column(matrix: &PatternMatrix, col: usize, dt: &DtMap) -> TypeInfo {
    for row in matrix {
        if let Some(SPat::Con(c, _)) = row.get(col) {
            match c {
                Constructor::BoolTrue | Constructor::BoolFalse => return TypeInfo::bool_type(),
                Constructor::Unit => return TypeInfo::unit_type(),
                Constructor::Nil | Constructor::Cons => return TypeInfo::list_type(),
                Constructor::Tuple(n) => return TypeInfo::tuple_type(*n),
                Constructor::IntLit(_)
                | Constructor::FloatBits(_)
                | Constructor::StringLit(_)
                | Constructor::CharLit(_) => return TypeInfo::infinite(),
                Constructor::Adt(con_name, _) => {
                    // Look up the parent type in the datatype map
                    for cons in dt.values() {
                        if cons.iter().any(|(n, _)| n == con_name) {
                            return TypeInfo::adt_type("", cons);
                        }
                    }
                    // Fallback: collect from matrix only
                    let mut adt_cons = Vec::new();
                    for r in matrix {
                        if let Some(SPat::Con(Constructor::Adt(name, tag), sub)) = r.get(col)
                            && !adt_cons
                                .iter()
                                .any(|(n, t, _): &(String, u16, usize)| n == name && *t == *tag)
                        {
                            adt_cons.push((name.clone(), *tag, sub.len()));
                        }
                    }
                    if !adt_cons.is_empty() {
                        return TypeInfo {
                            constructors: adt_cons
                                .into_iter()
                                .map(|(name, tag, arity)| (Constructor::Adt(name, tag), arity))
                                .collect(),
                            is_finite: true,
                        };
                    }
                }
            }
        }
    }
    TypeInfo::infinite()
}

/// Collect all constructors that appear as heads of rows in the matrix.
fn collect_head_constructors(matrix: &PatternMatrix) -> HashSet<Constructor> {
    let mut result = HashSet::new();
    for row in matrix {
        if let SPat::Con(c, _) = &row[0] {
            result.insert(c.clone());
        }
    }
    result
}

/// Specialize the matrix for constructor `c` with given arity.
/// For each row:
///   - If head is Con(c, sub_pats): replace head with sub_pats
///   - If head is Con(c', _) where c' != c: drop the row
///   - If head is Wild: replace head with `arity` wildcards
fn specialize_matrix(matrix: &PatternMatrix, c: &Constructor, arity: usize) -> PatternMatrix {
    let mut result = Vec::new();
    for row in matrix {
        match &row[0] {
            SPat::Con(rc, sub_pats) if rc == c => {
                let mut new_row: Vec<SPat> = sub_pats.clone();
                new_row.extend_from_slice(&row[1..]);
                result.push(new_row);
            }
            SPat::Con(_, _) => {
                // Different constructor, skip this row
            }
            SPat::Wild => {
                let mut new_row: Vec<SPat> = (0..arity).map(|_| SPat::Wild.clone()).collect();
                new_row.extend_from_slice(&row[1..]);
                result.push(new_row);
            }
        }
    }
    result
}

/// Default matrix: rows whose head is a wildcard, with the head removed.
fn default_matrix(matrix: &PatternMatrix) -> PatternMatrix {
    let mut result = Vec::new();
    for row in matrix {
        if matches!(&row[0], SPat::Wild) {
            result.push(row[1..].to_vec());
        }
    }
    result
}
