use serde::Deserialize;
use smallvec::smallvec;
use std::process::Command;

use super::*;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    #[cfg(not(feature = "builtin-github"))]
    {
        &[]
    }
    #[cfg(feature = "builtin-github")]
    {
        &[
            ("github_issue_create", github_issue_create as BuiltinFn),
            ("github_issue_view", github_issue_view as BuiltinFn),
            ("github_issue_update", github_issue_update as BuiltinFn),
        ]
    }
}

#[cfg(feature = "builtin-github")]
fn extract_tuple_fields(
    args: &[Value],
    heap: &Heap,
    name: &str,
    expected_arity: usize,
) -> Result<Vec<Value>, String> {
    match &args.first() {
        Some(Value::Heap(r)) => match heap.get(*r) {
            Ok(HeapObject::Tuple(t)) if t.len() == expected_arity => {
                Ok(t.iter().copied().collect())
            }
            _ => Err(format!("{name}: expected tuple of arity {expected_arity}")),
        },
        _ => Err(format!("{name}: expected tuple argument")),
    }
}

#[cfg(feature = "builtin-github")]
fn field_string(heap: &Heap, val: Value, name: &str, field: &str) -> Result<String, String> {
    match val {
        Value::Heap(r) => match heap.get(r) {
            Ok(HeapObject::String(s)) => Ok(s.clone()),
            _ => Err(format!("{name}: expected String for {field}")),
        },
        _ => Err(format!("{name}: expected String for {field}")),
    }
}

#[cfg(feature = "builtin-github")]
fn field_int(val: Value, name: &str, field: &str) -> Result<i64, String> {
    match val {
        Value::Int(n) => Ok(n),
        _ => Err(format!("{name}: expected Int for {field}")),
    }
}

#[cfg(feature = "builtin-github")]
fn repo_to_tuple(
    heap: &mut Heap,
    ok: bool,
    payload: String,
    errmsg: String,
) -> Result<Value, String> {
    let ok_val = Value::Bool(ok);
    let payload_ref = heap_alloc(heap, HeapObject::String(payload))?;
    let err_ref = heap_alloc(heap, HeapObject::String(errmsg))?;
    heap_alloc(
        heap,
        HeapObject::Tuple(smallvec![ok_val, payload_ref, err_ref]),
    )
}

/// Run `gh` with the given arguments and return (stdout, stderr, exit_code).
#[cfg(feature = "builtin-github")]
fn run_gh(args: &[&str]) -> Result<(String, String, i32), String> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run 'gh': {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    Ok((stdout, stderr, code))
}

#[cfg(feature = "builtin-github")]
pub(super) fn github_issue_create(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let fields = extract_tuple_fields(args, heap, "github_issue_create", 3)?;
    let repo = field_string(heap, fields[0], "github_issue_create", "repo")?;
    let title = field_string(heap, fields[1], "github_issue_create", "title")?;
    let body = field_string(heap, fields[2], "github_issue_create", "body")?;

    heap.check_github_repo_for("github_issue_create", &repo)
        .map_err(|e| format!("github_issue_create: {e}"))?;

    let (stdout, stderr, code) = run_gh(&[
        "issue", "create", "--repo", &repo, "--title", &title, "--body", &body,
    ])?;

    if code != 0 {
        return repo_to_tuple(heap, false, String::new(), stderr.trim().to_string());
    }

    let url = stdout.trim().to_string();
    if url.is_empty() {
        return repo_to_tuple(
            heap,
            false,
            String::new(),
            "gh issue create returned an empty URL".to_string(),
        );
    }

    repo_to_tuple(heap, true, url, String::new())
}

#[cfg(feature = "builtin-github")]
pub(super) fn github_issue_view(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let fields = extract_tuple_fields(args, heap, "github_issue_view", 2)?;
    let repo = field_string(heap, fields[0], "github_issue_view", "repo")?;
    let number = field_int(fields[1], "github_issue_view", "number")?;

    heap.check_github_repo_for("github_issue_view", &repo)
        .map_err(|e| format!("github_issue_view: {e}"))?;

    let num_str = number.to_string();
    let (stdout, stderr, code) = run_gh(&[
        "issue",
        "view",
        &num_str,
        "--repo",
        &repo,
        "--json",
        "number,title,state,url",
    ])?;

    if code != 0 {
        return repo_to_tuple(heap, false, String::new(), stderr.trim().to_string());
    }

    repo_to_tuple(heap, true, stdout.trim().to_string(), String::new())
}

#[cfg(feature = "builtin-github")]
pub(super) fn github_issue_update(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let fields = extract_tuple_fields(args, heap, "github_issue_update", 3)?;
    let repo = field_string(heap, fields[0], "github_issue_update", "repo")?;
    let number = field_int(fields[1], "github_issue_update", "number")?;
    let update_json = field_string(heap, fields[2], "github_issue_update", "spec")?;

    heap.check_github_repo_for("github_issue_update", &repo)
        .map_err(|e| format!("github_issue_update: {e}"))?;

    #[derive(Deserialize)]
    struct Update {
        #[serde(rename = "SetTitle")]
        set_title: Option<String>,
        #[serde(rename = "SetBody")]
        set_body: Option<String>,
        #[serde(rename = "AddLabel")]
        add_label: Option<String>,
        #[serde(rename = "RemoveLabel")]
        remove_label: Option<String>,
        #[serde(rename = "Close")]
        close: Option<bool>,
        #[serde(rename = "Reopen")]
        reopen: Option<bool>,
    }

    let update: Update = match serde_json::from_str(&update_json) {
        Ok(v) => v,
        Err(e) => {
            return repo_to_tuple(
                heap,
                false,
                String::new(),
                format!("invalid update spec: {e}"),
            );
        }
    };

    let num_str = number.to_string();
    let mut gh_args = vec!["issue", "edit", &num_str, "--repo", &repo];

    if let Some(t) = &update.set_title {
        gh_args.push("--title");
        gh_args.push(t);
    }
    if let Some(b) = &update.set_body {
        gh_args.push("--body");
        gh_args.push(b);
    }
    if let Some(l) = &update.add_label {
        gh_args.push("--add-label");
        gh_args.push(l);
    }
    if let Some(l) = &update.remove_label {
        gh_args.push("--remove-label");
        gh_args.push(l);
    }
    if gh_args.len() > 5 {
        let (_stdout, stderr, code) = run_gh(&gh_args)?;
        if code != 0 {
            return repo_to_tuple(heap, false, String::new(), stderr.trim().to_string());
        }
    }

    if update.close == Some(true) {
        let (_stdout, stderr, code) = run_gh(&["issue", "close", &num_str, "--repo", &repo])?;
        if code != 0 {
            return repo_to_tuple(heap, false, String::new(), stderr.trim().to_string());
        }
    }

    if update.reopen == Some(true) {
        let (_stdout, stderr, code) = run_gh(&["issue", "reopen", &num_str, "--repo", &repo])?;
        if code != 0 {
            return repo_to_tuple(heap, false, String::new(), stderr.trim().to_string());
        }
    }

    // Re-fetch the issue to return current state
    let (out, err2, code2) = run_gh(&[
        "issue",
        "view",
        &num_str,
        "--repo",
        &repo,
        "--json",
        "number,title,state,url",
    ])?;

    if code2 != 0 {
        // Update succeeded but re-fetch failed; return minimal success
        return repo_to_tuple(
            heap,
            true,
            String::new(),
            format!("updated but could not re-fetch: {err2}"),
        );
    }

    repo_to_tuple(heap, true, out.trim().to_string(), String::new())
}
