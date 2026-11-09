//! Maranget-style exhaustiveness/usefulness checking, sharing inference's budget.
//! Every visit and expanded copy is charged before work/allocation. A whole
//! wildcard row covers the query; a wildcard in only one column does not.

use crate::infer::TypeError;
use hiko_syntax::ast::{Pat, PatKind};
use hiko_syntax::intern::StringInterner;
use std::collections::{HashMap, HashSet};

type Checked<T> = Result<T, TypeError>;
type Charge<'a> = &'a dyn Fn() -> Checked<()>;
type DtMap = HashMap<String, Vec<(String, usize)>>;
type PatternMatrix = Vec<Vec<SPat>>;

#[derive(Debug)]
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
    IntLit(i64),
    FloatBits(u64),
    WordLit(u64),
    StringLit(String),
    CharLit(char),
}

/// ADT metadata is borrowed, not deep-copied for every match. Constructor
/// names are copied (and charged) only when specialization actually needs them.
#[derive(Debug)]
pub struct TypeInfo<'a> {
    constructors: Vec<(Constructor, usize)>,
    adt: Option<&'a [(String, usize)]>,
    is_finite: bool,
}
impl<'a> TypeInfo<'a> {
    fn known(constructors: Vec<(Constructor, usize)>) -> Self {
        Self {
            constructors,
            adt: None,
            is_finite: true,
        }
    }
    pub fn bool_type() -> Self {
        Self::known(vec![
            (Constructor::BoolTrue, 0),
            (Constructor::BoolFalse, 0),
        ])
    }
    pub fn unit_type() -> Self {
        Self::known(vec![(Constructor::Unit, 0)])
    }
    pub fn list_type() -> Self {
        Self::known(vec![(Constructor::Nil, 0), (Constructor::Cons, 2)])
    }
    pub fn tuple_type(arity: usize) -> Self {
        Self::known(vec![(Constructor::Tuple(arity), arity)])
    }
    pub fn adt_type(_type_name: &str, constructors: &'a [(String, usize)]) -> Self {
        Self {
            constructors: Vec::new(),
            adt: Some(constructors),
            is_finite: true,
        }
    }
    pub fn infinite() -> Self {
        Self {
            constructors: Vec::new(),
            adt: None,
            is_finite: false,
        }
    }
    fn len(&self) -> usize {
        self.adt.map_or(self.constructors.len(), <[_]>::len)
    }
    fn constructor(&self, index: usize, charge: Charge<'_>) -> Checked<(Constructor, usize)> {
        charge()?;
        if let Some(adt) = self.adt {
            let (name, arity) = &adt[index];
            pay(charge, name.len())?;
            Ok((Constructor::Adt(name.clone(), index as u16), *arity))
        } else {
            let (constructor, arity) = &self.constructors[index];
            Ok((copy_constructor(constructor, charge)?, *arity))
        }
    }
    fn copy(&self, charge: Charge<'_>) -> Checked<Self> {
        charge()?;
        pay(charge, self.constructors.len())?;
        let mut constructors = Vec::with_capacity(self.constructors.len());
        for (c, arity) in &self.constructors {
            constructors.push((copy_constructor(c, charge)?, *arity));
        }
        Ok(Self {
            constructors,
            adt: self.adt,
            is_finite: self.is_finite,
        })
    }
}

pub struct CheckResult {
    pub exhaustive: bool,
    pub redundant_clauses: Vec<usize>,
}

fn pay(charge: Charge<'_>, count: usize) -> Checked<()> {
    for _ in 0..count {
        charge()?;
    }
    Ok(())
}
fn copy_constructor(c: &Constructor, charge: Charge<'_>) -> Checked<Constructor> {
    charge()?;
    if let Constructor::Adt(name, _) | Constructor::StringLit(name) = c {
        pay(charge, name.len())?;
    }
    Ok(c.clone())
}
fn copy_patterns(patterns: &[SPat], charge: Charge<'_>) -> Checked<Vec<SPat>> {
    pay(charge, patterns.len())?;
    let mut copied = Vec::with_capacity(patterns.len());
    for p in patterns {
        charge()?;
        copied.push(match p {
            SPat::Wild => SPat::Wild,
            SPat::Con(c, sub) => {
                SPat::Con(copy_constructor(c, charge)?, copy_patterns(sub, charge)?)
            }
        });
    }
    Ok(copied)
}
fn wildcards(arity: usize, charge: Charge<'_>) -> Checked<Vec<SPat>> {
    pay(charge, arity)?;
    Ok((0..arity).map(|_| SPat::Wild).collect())
}
fn simplify_pat(
    pat: &Pat,
    tags: &HashMap<String, u16>,
    aliases: &HashMap<String, String>,
    interner: &StringInterner,
    charge: Charge<'_>,
) -> Checked<SPat> {
    charge()?;
    Ok(match &pat.kind {
        PatKind::Wildcard | PatKind::Var(_) => SPat::Wild,
        PatKind::BoolLit(true) => SPat::Con(Constructor::BoolTrue, vec![]),
        PatKind::BoolLit(false) => SPat::Con(Constructor::BoolFalse, vec![]),
        PatKind::Unit => SPat::Con(Constructor::Unit, vec![]),
        PatKind::IntLit(n) => SPat::Con(Constructor::IntLit(*n), vec![]),
        PatKind::WordLit(n) => SPat::Con(Constructor::WordLit(*n), vec![]),
        PatKind::FloatLit(n) => SPat::Con(Constructor::FloatBits(n.to_bits()), vec![]),
        PatKind::StringLit(s) => {
            pay(charge, s.len())?;
            SPat::Con(Constructor::StringLit(s.clone()), vec![])
        }
        PatKind::CharLit(c) => SPat::Con(Constructor::CharLit(*c), vec![]),
        PatKind::Constructor(sym, payload) => {
            let name = interner.resolve(*sym);
            let canonical = aliases.get(name).map_or(name, String::as_str);
            pay(charge, canonical.len())?;
            let tag = tags.get(canonical).copied().unwrap_or(0);
            let sub = if let Some(p) = payload {
                charge()?;
                vec![simplify_pat(p, tags, aliases, interner, charge)?]
            } else {
                vec![]
            };
            SPat::Con(Constructor::Adt(canonical.to_string(), tag), sub)
        }
        PatKind::Tuple(pats) => {
            pay(charge, pats.len())?;
            let mut sub = Vec::with_capacity(pats.len());
            for p in pats {
                sub.push(simplify_pat(p, tags, aliases, interner, charge)?);
            }
            SPat::Con(Constructor::Tuple(pats.len()), sub)
        }
        PatKind::Cons(head, tail) => {
            pay(charge, 2)?;
            SPat::Con(
                Constructor::Cons,
                vec![
                    simplify_pat(head, tags, aliases, interner, charge)?,
                    simplify_pat(tail, tags, aliases, interner, charge)?,
                ],
            )
        }
        PatKind::List(pats) => {
            charge()?;
            let mut result = SPat::Con(Constructor::Nil, vec![]);
            for p in pats.iter().rev() {
                pay(charge, 3)?;
                result = SPat::Con(
                    Constructor::Cons,
                    vec![simplify_pat(p, tags, aliases, interner, charge)?, result],
                );
            }
            result
        }
        PatKind::As(_, p) | PatKind::Paren(p) | PatKind::Ann(p, _) => {
            simplify_pat(p, tags, aliases, interner, charge)?
        }
    })
}

pub fn check_match(
    patterns: &[&Pat],
    type_info: &TypeInfo<'_>,
    tags: &HashMap<String, u16>,
    aliases: &HashMap<String, String>,
    dt: &DtMap,
    interner: &StringInterner,
    charge: Charge<'_>,
) -> Checked<CheckResult> {
    pay(charge, patterns.len())?;
    let mut matrix = Vec::with_capacity(patterns.len());
    for p in patterns {
        charge()?;
        matrix.push(vec![simplify_pat(p, tags, aliases, interner, charge)?]);
    }
    charge()?;
    let exhaustive = !is_useful(
        &matrix,
        &[SPat::Wild],
        std::slice::from_ref(type_info),
        dt,
        charge,
    )?;
    let mut redundant_clauses = Vec::new();
    for i in 0..matrix.len() {
        charge()?;
        if !is_useful(
            &matrix[..i],
            &matrix[i],
            std::slice::from_ref(type_info),
            dt,
            charge,
        )? {
            charge()?;
            redundant_clauses.push(i);
        }
    }
    Ok(CheckResult {
        exhaustive,
        redundant_clauses,
    })
}

fn is_useful(
    matrix: &[Vec<SPat>],
    q: &[SPat],
    types: &[TypeInfo<'_>],
    dt: &DtMap,
    charge: Charge<'_>,
) -> Checked<bool> {
    charge()?;
    let Some(head) = q.first() else {
        return Ok(matrix.is_empty());
    };
    for row in matrix {
        charge()?;
        let mut all_wild = true;
        for p in row {
            charge()?;
            if !matches!(p, SPat::Wild) {
                all_wild = false;
                break;
            }
        }
        if all_wild {
            return Ok(false);
        }
    }
    match head {
        SPat::Con(c, sub) => {
            is_useful_specialized(matrix, c, copy_patterns(sub, charge)?, q, types, dt, charge)
        }
        SPat::Wild => {
            let ti = &types[0];
            let heads = collect_head_constructors(matrix, charge)?;
            // Cardinality can disprove complete coverage without copying/scanning
            // a large ADT catalog for each single-constructor-plus-wildcard match.
            let mut covered = ti.is_finite && heads.len() >= ti.len();
            if covered {
                for index in 0..ti.len() {
                    let (c, _) = ti.constructor(index, charge)?;
                    if !heads.contains(&c) {
                        covered = false;
                        break;
                    }
                }
            }
            if covered {
                for index in 0..ti.len() {
                    let (c, arity) = ti.constructor(index, charge)?;
                    if is_useful_specialized(
                        matrix,
                        &c,
                        wildcards(arity, charge)?,
                        q,
                        types,
                        dt,
                        charge,
                    )? {
                        return Ok(true);
                    }
                }
                Ok(false)
            } else {
                is_useful(
                    &default_matrix(matrix, charge)?,
                    &q[1..],
                    &types[1..],
                    dt,
                    charge,
                )
            }
        }
    }
}
fn is_useful_specialized(
    matrix: &[Vec<SPat>],
    c: &Constructor,
    mut sub: Vec<SPat>,
    q: &[SPat],
    types: &[TypeInfo<'_>],
    dt: &DtMap,
    charge: Charge<'_>,
) -> Checked<bool> {
    charge()?;
    let arity = sub.len();
    let matrix = specialize_matrix(matrix, c, arity, charge)?;
    sub.extend(copy_patterns(&q[1..], charge)?);
    pay(charge, arity + types.len().saturating_sub(1))?;
    let mut specialized = Vec::with_capacity(arity + types.len().saturating_sub(1));
    for i in 0..arity {
        specialized.push(infer_type_info_from_column(&matrix, i, dt, charge)?);
    }
    for ty in &types[1..] {
        specialized.push(ty.copy(charge)?);
    }
    is_useful(&matrix, &sub, &specialized, dt, charge)
}
fn infer_type_info_from_column<'a>(
    matrix: &[Vec<SPat>],
    col: usize,
    dt: &'a DtMap,
    charge: Charge<'_>,
) -> Checked<TypeInfo<'a>> {
    for row in matrix {
        charge()?;
        if let Some(SPat::Con(c, _)) = row.get(col) {
            pay(charge, 2)?;
            match c {
                Constructor::BoolTrue | Constructor::BoolFalse => return Ok(TypeInfo::bool_type()),
                Constructor::Unit => return Ok(TypeInfo::unit_type()),
                Constructor::Nil | Constructor::Cons => return Ok(TypeInfo::list_type()),
                Constructor::Tuple(n) => return Ok(TypeInfo::tuple_type(*n)),
                Constructor::IntLit(_)
                | Constructor::WordLit(_)
                | Constructor::FloatBits(_)
                | Constructor::StringLit(_)
                | Constructor::CharLit(_) => return Ok(TypeInfo::infinite()),
                Constructor::Adt(name, _) => {
                    for cons in dt.values() {
                        charge()?;
                        for (candidate, _) in cons {
                            charge()?;
                            if candidate == name {
                                return Ok(TypeInfo::adt_type("", cons));
                            }
                        }
                    }
                    let mut constructors = Vec::new();
                    for r in matrix {
                        charge()?;
                        if let Some(SPat::Con(c @ Constructor::Adt(_, _), sub)) = r.get(col) {
                            let mut seen = false;
                            for (prior, _) in &constructors {
                                charge()?;
                                if prior == c {
                                    seen = true;
                                    break;
                                }
                            }
                            if !seen {
                                constructors.push((copy_constructor(c, charge)?, sub.len()));
                            }
                        }
                    }
                    if !constructors.is_empty() {
                        return Ok(TypeInfo::known(constructors));
                    }
                }
            }
        }
    }
    Ok(TypeInfo::infinite())
}
fn collect_head_constructors(
    matrix: &[Vec<SPat>],
    charge: Charge<'_>,
) -> Checked<HashSet<Constructor>> {
    let mut result = HashSet::new();
    for row in matrix {
        charge()?;
        if let SPat::Con(c, _) = &row[0] {
            result.insert(copy_constructor(c, charge)?);
        }
    }
    Ok(result)
}
fn specialize_matrix(
    matrix: &[Vec<SPat>],
    c: &Constructor,
    arity: usize,
    charge: Charge<'_>,
) -> Checked<PatternMatrix> {
    let mut result = Vec::new();
    for row in matrix {
        charge()?;
        let mut copied = match &row[0] {
            SPat::Con(rc, sub) if rc == c => copy_patterns(sub, charge)?,
            SPat::Con(_, _) => continue,
            SPat::Wild => wildcards(arity, charge)?,
        };
        copied.extend(copy_patterns(&row[1..], charge)?);
        charge()?;
        result.push(copied);
    }
    Ok(result)
}
fn default_matrix(matrix: &[Vec<SPat>], charge: Charge<'_>) -> Checked<PatternMatrix> {
    let mut result = Vec::new();
    for row in matrix {
        charge()?;
        if matches!(&row[0], SPat::Wild) {
            let copied = copy_patterns(&row[1..], charge)?;
            charge()?;
            result.push(copied);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiko_syntax::span::Span;
    use std::cell::Cell;

    fn meter(used: &Cell<u64>, limit: u64) -> impl Fn() -> Checked<()> + '_ {
        move || {
            if used.get() == limit {
                return Err(TypeError {
                    message: "test work limit".into(),
                    span: Span::dummy(),
                });
            }
            used.set(used.get() + 1);
            Ok(())
        }
    }
    #[test]
    fn whole_wildcard_row_stops_before_boolean_specialization() {
        let width = 80;
        let mut matrix = vec![(0..width).map(|_| SPat::Wild).collect::<Vec<_>>()];
        for col in 0..width {
            for c in [Constructor::BoolTrue, Constructor::BoolFalse] {
                let mut row: Vec<_> = (0..width).map(|_| SPat::Wild).collect();
                row[col] = SPat::Con(c, vec![]);
                matrix.push(row);
            }
        }
        let q: Vec<_> = (0..width).map(|_| SPat::Wild).collect();
        let types: Vec<_> = (0..width).map(|_| TypeInfo::bool_type()).collect();
        let used = Cell::new(0);
        assert!(!is_useful(&matrix, &q, &types, &DtMap::new(), &meter(&used, 100)).unwrap());
        assert_eq!(used.get(), 82);
    }
    #[test]
    fn one_wildcard_column_does_not_cover_the_entire_row() {
        let matrix = vec![vec![SPat::Wild, SPat::Con(Constructor::BoolTrue, vec![])]];
        let query = [SPat::Wild, SPat::Wild];
        let types = [TypeInfo::bool_type(), TypeInfo::bool_type()];
        let used = Cell::new(0);
        assert!(
            is_useful(
                &matrix,
                &query,
                &types,
                &DtMap::new(),
                &meter(&used, 10_000)
            )
            .unwrap()
        );
    }
    #[test]
    fn specialization_has_zero_exact_and_one_short_limits() {
        let matrix = vec![vec![SPat::Con(Constructor::BoolTrue, vec![])]];
        let query = [SPat::Wild];
        let types = [TypeInfo::bool_type()];
        let used = Cell::new(0);
        assert!(
            is_useful(
                &matrix,
                &query,
                &types,
                &DtMap::new(),
                &meter(&used, 10_000)
            )
            .unwrap()
        );
        let exact = used.get();
        for limit in [0, exact - 1, exact] {
            let used = Cell::new(0);
            let result = is_useful(&matrix, &query, &types, &DtMap::new(), &meter(&used, limit));
            assert_eq!(result.is_ok(), limit == exact);
            assert!(used.get() <= limit);
        }
    }
}
