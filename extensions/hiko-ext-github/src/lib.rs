//! GitHub issue builtins for Hiko, as an extension.
//!
//! The builtins shell out to the `gh` CLI, which must be installed and
//! authenticated. The run config turns the extension on and lists, per
//! operation, the repositories it may touch:
//!
//! ```toml
//! [extensions.github]
//! create = ["vectorian-rs/hiko"]
//! view = ["vectorian-rs/hiko"]
//! update = []
//! list = ["vectorian-rs/hiko"]
//! ```
//!
//! An operation without a list may not touch any repository. Scripts use the
//! `Github.Issue` module, or the raw builtins, which return
//! `(ok, payload, error)`.

use std::process::Command;
use std::sync::Arc;

use hiko_builtin_meta::BuiltinTypeSignature;
use hiko_vm::extension::{Extension, ExtensionBuiltin, ExtensionFn, ExtensionState};
use hiko_vm::heap::Heap;
use hiko_vm::value::{HeapObject, Value};
use serde::Deserialize;
use smallvec::smallvec;

/// The extension, to link into a host and pass to
/// `RunConfig::build_vm_with_extensions`.
pub static GITHUB: Extension = Extension {
    name: "github",
    types: &[],
    builtins: &[
        builtin(
            "github_issue_create",
            "string * string * string -> bool * string * string",
            issue_create,
        ),
        builtin(
            "github_issue_view",
            "string * int -> bool * string * string",
            issue_view,
        ),
        builtin(
            "github_issue_update",
            "string * int * string -> bool * string * string",
            issue_update,
        ),
        builtin(
            "github_issue_list",
            "string -> bool * string * string",
            issue_list,
        ),
    ],
    modules: &[("Github.Issue", include_str!("Issue.hml"))],
    requires: &[],
    configure,
};

const fn builtin(
    name: &'static str,
    ty: &'static str,
    func: hiko_vm::value::BuiltinFn,
) -> ExtensionBuiltin {
    ExtensionBuiltin {
        name,
        signature: BuiltinTypeSignature { vars: &[], ty },
        func: ExtensionFn::Sync(func),
    }
}

/// Repositories each operation may use, from `[extensions.github]`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    #[serde(default)]
    create: Vec<String>,
    #[serde(default)]
    view: Vec<String>,
    #[serde(default)]
    update: Vec<String>,
    #[serde(default)]
    list: Vec<String>,
}

fn configure(table: &toml::Value) -> Result<ExtensionState, String> {
    let policy: Policy = table.clone().try_into().map_err(|e| e.to_string())?;
    Ok(Arc::new(policy))
}

/// Fail unless the run config lets `builtin` use `repo`.
fn check_repo(heap: &Heap, builtin: &str, repo: &str) -> Result<(), String> {
    let policy = heap
        .extension_state::<Policy>("github")
        .ok_or_else(|| format!("{builtin}: the github extension is not configured"))?;
    let allowed = match builtin {
        "github_issue_create" => &policy.create,
        "github_issue_view" => &policy.view,
        "github_issue_update" => &policy.update,
        _ => &policy.list,
    };
    if allowed.iter().any(|r| r == repo) {
        Ok(())
    } else {
        Err(format!(
            "{builtin}: repo '{repo}' is not in the allowed repos {allowed:?}"
        ))
    }
}

fn tuple_fields(
    args: &[Value],
    heap: &Heap,
    name: &str,
    arity: usize,
) -> Result<Vec<Value>, String> {
    match args.first() {
        Some(Value::Heap(r)) => match heap.get(*r) {
            Ok(HeapObject::Tuple(t)) if t.len() == arity => Ok(t.to_vec()),
            _ => Err(format!("{name}: expected a tuple of arity {arity}")),
        },
        _ => Err(format!("{name}: expected a tuple argument")),
    }
}

fn string_field(heap: &Heap, value: Value, name: &str, field: &str) -> Result<String, String> {
    match value {
        Value::Heap(r) => match heap.get(r) {
            Ok(HeapObject::String(s)) => Ok(s.to_string()),
            _ => Err(format!("{name}: expected a string for {field}")),
        },
        _ => Err(format!("{name}: expected a string for {field}")),
    }
}

fn int_field(value: Value, name: &str, field: &str) -> Result<i64, String> {
    match value {
        Value::Int(n) => Ok(n),
        _ => Err(format!("{name}: expected an int for {field}")),
    }
}

/// `(ok, payload, error)`.
fn result(heap: &mut Heap, ok: bool, payload: String, error: String) -> Result<Value, String> {
    let alloc = |heap: &mut Heap, obj| heap.alloc(obj).map(Value::Heap).map_err(|e| e.to_string());
    let payload = alloc(heap, HeapObject::String(payload.into()))?;
    let error = alloc(heap, HeapObject::String(error.into()))?;
    alloc(
        heap,
        HeapObject::Tuple(smallvec![Value::Bool(ok), payload, error]),
    )
}

/// Run `gh` and return `(stdout, stderr, success)`.
fn run_gh(args: &[&str]) -> Result<(String, String, bool), String> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run 'gh': {e}"))?;
    Ok((
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
        output.status.success(),
    ))
}

/// Run `gh` and turn its outcome into `(ok, stdout, stderr)`.
fn gh_result(heap: &mut Heap, args: &[&str]) -> Result<Value, String> {
    let (stdout, stderr, ok) = run_gh(args)?;
    if ok {
        result(heap, true, stdout, String::new())
    } else {
        result(heap, false, String::new(), stderr)
    }
}

const VIEW_FIELDS: &str = "number,title,state,url";

fn issue_create(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let name = "github_issue_create";
    let fields = tuple_fields(args, heap, name, 3)?;
    let repo = string_field(heap, fields[0], name, "repo")?;
    let title = string_field(heap, fields[1], name, "title")?;
    let body = string_field(heap, fields[2], name, "body")?;
    check_repo(heap, name, &repo)?;
    let (url, stderr, ok) = run_gh(&[
        "issue", "create", "--repo", &repo, "--title", &title, "--body", &body,
    ])?;
    match (ok, url.is_empty()) {
        (true, false) => result(heap, true, url, String::new()),
        (true, true) => result(
            heap,
            false,
            String::new(),
            "gh issue create returned an empty URL".into(),
        ),
        (false, _) => result(heap, false, String::new(), stderr),
    }
}

fn issue_view(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let name = "github_issue_view";
    let fields = tuple_fields(args, heap, name, 2)?;
    let repo = string_field(heap, fields[0], name, "repo")?;
    let number = int_field(fields[1], name, "number")?.to_string();
    check_repo(heap, name, &repo)?;
    gh_result(
        heap,
        &[
            "issue",
            "view",
            &number,
            "--repo",
            &repo,
            "--json",
            VIEW_FIELDS,
        ],
    )
}

fn issue_list(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let name = "github_issue_list";
    let repo = string_field(heap, args[0], name, "repo")?;
    check_repo(heap, name, &repo)?;
    gh_result(
        heap,
        &[
            "issue",
            "list",
            "--repo",
            &repo,
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            VIEW_FIELDS,
        ],
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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

fn issue_update(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let name = "github_issue_update";
    let fields = tuple_fields(args, heap, name, 3)?;
    let repo = string_field(heap, fields[0], name, "repo")?;
    let number = int_field(fields[1], name, "number")?.to_string();
    let spec = string_field(heap, fields[2], name, "spec")?;
    check_repo(heap, name, &repo)?;

    let update: Update = match serde_json::from_str(&spec) {
        Ok(update) => update,
        Err(e) => {
            return result(
                heap,
                false,
                String::new(),
                format!("invalid update spec: {e}"),
            );
        }
    };

    let mut edit = vec!["issue", "edit", number.as_str(), "--repo", repo.as_str()];
    let base_len = edit.len();
    for (flag, value) in [
        ("--title", &update.set_title),
        ("--body", &update.set_body),
        ("--add-label", &update.add_label),
        ("--remove-label", &update.remove_label),
    ] {
        if let Some(value) = value {
            edit.push(flag);
            edit.push(value);
        }
    }
    let mut steps = Vec::new();
    if edit.len() > base_len {
        steps.push(edit);
    }
    if update.close == Some(true) {
        steps.push(vec!["issue", "close", &number, "--repo", &repo]);
    }
    if update.reopen == Some(true) {
        steps.push(vec!["issue", "reopen", &number, "--repo", &repo]);
    }
    for step in steps {
        let (_, stderr, ok) = run_gh(&step)?;
        if !ok {
            return result(heap, false, String::new(), stderr);
        }
    }

    gh_result(
        heap,
        &[
            "issue",
            "view",
            &number,
            "--repo",
            &repo,
            "--json",
            VIEW_FIELDS,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_lists_every_library_import() {
        assert_eq!(GITHUB.undeclared_imports(), Ok(vec![]));
    }
}
