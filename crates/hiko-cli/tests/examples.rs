//! The examples that must fail: the `should_fail_io_*` scripts under the
//! `should-fail-io` policy, and the `use` cycle in `cycle_a.hml`.
//! `tools/run_all.hml` skips them, so this is what checks them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs `script` under `policy` with `stdin` and returns (success, stderr).
fn run(script: &str, policy: &str, stdin: &[u8]) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hiko-cli"))
        .current_dir(repo_root())
        .env("NO_COLOR", "1")
        .args(["run", "--config", "hiko.toml", "--policy", policy, script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hiko-cli should start");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn should_fail_io_examples_fail_on_the_io_limit() {
    let cases = [
        ("glob", "glob: io limit exceeded"),
        ("list_dir", "list_dir: io limit exceeded"),
        ("read_file", "read_file: result exceeds max_io_bytes"),
        (
            "read_file_bytes",
            "read_file_bytes: result exceeds max_io_bytes",
        ),
        ("stdin", "read_stdin: io limit exceeded"),
        ("stdout", "stdout: io limit exceeded"),
        ("walk_dir", "walk_dir: io limit exceeded"),
        ("write_file", "write_file: io limit exceeded"),
    ];
    let output = repo_root().join("target/should_fail_io_output.txt");
    let _ = std::fs::remove_file(&output);

    for (name, expected) in cases {
        let script = format!("examples/should_fail_io_{name}.hml");
        let (ok, stderr) = run(&script, "should-fail-io", b"abcdef");
        assert!(!ok, "{script} should fail");
        assert!(stderr.contains(expected), "{script}: {stderr}");
    }
    assert!(!output.exists(), "write_file must fail before writing");
}

#[test]
fn use_cycle_example_fails() {
    let (ok, stderr) = run("examples/cycle_a.hml", "test-runner", b"");
    assert!(!ok, "cycle_a.hml should fail");
    assert!(stderr.contains("circular import detected"), "{stderr}");
}

#[test]
fn formerly_core_only_examples_use_the_named_core_policy() {
    for script in [
        "examples/hello.hml",
        "examples/factorial.hml",
        "examples/closures.hml",
    ] {
        let (ok, stderr) = run(script, "core", b"");
        assert!(ok, "{script}: {stderr}");
    }
}
