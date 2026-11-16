//! The documentation is a checked mapping, never a runtime registry.
use std::collections::{BTreeMap, BTreeSet};

const AUDIT: &str = include_str!("../../../docs/builtins/builtin-audit.md");

fn validate_audit(text: &str) -> Result<(), String> {
    let expected: BTreeMap<_, _> = super::ALL_FAMILIES
        .iter()
        .flat_map(|(feature, family)| family.iter().map(move |meta| (meta.name, (*feature, meta))))
        .collect();
    let mut seen = BTreeSet::new();
    for line in text.lines().filter(|line| line.starts_with("| ")) {
        let cells: Vec<_> = line.trim_matches('|').split('|').map(str::trim).collect();
        if matches!(cells[0], "Name" | "---") {
            continue;
        }
        if cells.len() != 6 {
            return Err(format!("expected six audit columns: {line}"));
        }
        let name = cells[0];
        let (feature, meta) = expected
            .get(name)
            .ok_or_else(|| format!("unknown audit name: {name}"))?;
        if !seen.insert(name) {
            return Err(format!("duplicate audit name: {name}"));
        }
        if cells[1] != *feature || cells[2] != meta.capability_path.unwrap_or("-") {
            return Err(format!("metadata mismatch for {name}"));
        }
        if !["Std.", "Http.", "Hash."]
            .iter()
            .any(|prefix| cells[3].starts_with(prefix))
            || cells[3].split('.').any(str::is_empty)
        {
            return Err(format!("missing module function for {name}"));
        }
        if (meta.capability_path.is_none() && cells[4] != "-")
            || (meta.capability_path.is_some() && !cells[4].starts_with("capabilities."))
        {
            return Err(format!("missing proposed key for {name}"));
        }
        if !cells[5].starts_with("primitive: ") && !cells[5].starts_with("composed: ") {
            return Err(format!("missing primitive/composed rationale for {name}"));
        }
    }
    let missing: Vec<_> = expected
        .keys()
        .filter(|name| !seen.contains(**name))
        .collect();
    if !missing.is_empty() {
        return Err(format!("missing audit names: {missing:?}"));
    }
    Ok(())
}

#[test]
fn builtin_audit_covers_every_family_even_when_disabled() {
    validate_audit(AUDIT).unwrap();
}

#[test]
fn builtin_audit_rejects_optional_and_runtime_alias_omissions() {
    for name in [
        "json_parse",
        "regex_match",
        "date_utc_tz",
        "numeric_int32_add",
        "await_process_result",
    ] {
        let marker = format!("| {name} |");
        let mutated = AUDIT
            .lines()
            .filter(|line| !line.starts_with(&marker))
            .collect::<Vec<_>>()
            .join("\n");
        let error = validate_audit(&mutated).unwrap_err();
        assert!(
            error.contains("missing audit names") && error.contains(name),
            "{error}"
        );
    }
}

#[test]
fn builtin_audit_rejects_duplicate_unknown_and_drifted_rows() {
    let row = AUDIT
        .lines()
        .find(|line| line.starts_with("| int_to_string |"))
        .unwrap();
    let duplicated = format!("{AUDIT}\n{row}\n");
    assert!(
        validate_audit(&duplicated)
            .unwrap_err()
            .contains("duplicate audit name")
    );
    let unknown = AUDIT.replace("| int_to_string |", "| no_such_builtin |");
    assert!(
        validate_audit(&unknown)
            .unwrap_err()
            .contains("unknown audit name")
    );
    let drifted = AUDIT.replace(
        "| capabilities.int.to_string |",
        "| capabilities.convert.int_to_string |",
    );
    assert!(
        validate_audit(&drifted)
            .unwrap_err()
            .contains("metadata mismatch")
    );
}
