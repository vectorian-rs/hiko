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

// Exit is tested only in subprocesses with a private integrity cache.
#[cfg(all(feature = "cli-lock", feature = "loader-named-imports"))]
mod thin_modules {
    use std::fs;
    use std::path::PathBuf;
    use std::process::{Command, Output};

    struct Project(PathBuf);

    impl Project {
        fn new(name: &str, modules: &[&str], source: &str, policy: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("hiko-thin-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            let root = fs::canonicalize(root).unwrap();
            fs::write(root.join("hiko.toml"), "[defaults]\npolicy = \"test\"\nlib_cache = \"cache\"\n[policies.test]\npath = \"policy.toml\"\n[registries.test]\nurl = \"http://127.0.0.1:9\"\n[dependencies]\nStd = { version = \"0.1.0\", registry = \"test\" }\n").unwrap();
            fs::write(root.join("main.hml"), source).unwrap();
            fs::write(root.join("policy.toml"), policy).unwrap();
            let base = "http://127.0.0.1:9/Std-v0.1.0";
            let mut lock = format!(
                "schema_version = 1\n[packages.Std]\nversion = \"0.1.0\"\nbase_url = {base:?}\n[packages.Std.modules]\n"
            );
            for module in modules {
                let module_source = fs::read(format!(
                    "{}/../../libraries/Std-v0.1.0/modules/{module}.hml",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .unwrap();
                let hash = hiko_common::blake3_hex(&module_source);
                lock.push_str(&format!("{module} = \"blake3:{hash}\"\n"));
                let cache = hiko_compile::compiler::module_cache_path(
                    &root.join("cache"),
                    &format!("{base}/modules/{module}.hml"),
                    &hash,
                );
                fs::create_dir_all(cache.parent().unwrap()).unwrap();
                fs::write(cache, module_source).unwrap();
            }
            fs::write(root.join("hiko.lock.toml"), lock).unwrap();
            Self(root)
        }

        fn hiko(&self, args: &[&str]) -> Output {
            Command::new(env!("CARGO_BIN_EXE_hiko-cli"))
                .current_dir(&self.0)
                .env("NO_COLOR", "1")
                .args(args)
                .output()
                .unwrap()
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn stderr(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    #[cfg(all(feature = "builtin-system", feature = "builtin-testing"))]
    #[test]
    fn system_exit_and_panic_preserve_host_and_policy_outcomes() {
        for (name, body, policy, code, message) in [
            (
                "exit",
                "System.exit 3",
                "[capabilities.system.exit]\nenabled = true",
                Some(3),
                "",
            ),
            (
                "denied-exit",
                "System.exit 3",
                "",
                None,
                "builtin 'exit' was not registered by the active policy-filtered VM",
            ),
            (
                "panic",
                "System.panic \"sentinel\"",
                "[capabilities.testing.panic]\nenabled = true",
                None,
                "sentinel",
            ),
            (
                "denied-panic",
                "System.panic \"sentinel\"",
                "",
                None,
                "builtin 'panic' was not registered by the active policy-filtered VM",
            ),
        ] {
            let project = Project::new(
                name,
                &["System"],
                &format!("import Std.System\nval x = {body}\n"),
                policy,
            );
            let output = project.hiko(&["run", "main.hml"]);
            assert!(!output.status.success(), "{name} unexpectedly succeeded");
            if let Some(code) = code {
                assert_eq!(output.status.code(), Some(code), "{}", stderr(&output));
            } else {
                assert_ne!(output.status.code(), Some(3));
                assert!(
                    stderr(&output).contains(message),
                    "{name}: {}",
                    stderr(&output)
                );
            }
        }
        let project = Project::new(
            "import-only",
            &["System"],
            "import Std.System\nval x = 1",
            "",
        );
        let output = project.hiko(&["run", "main.hml"]);
        assert!(output.status.success(), "{}", stderr(&output));
    }

    #[cfg(all(feature = "builtin-system", feature = "builtin-testing"))]
    #[test]
    fn strict_system_import_checks_both_unused_bodies() {
        let source = "import Std.System\nval x = 1";
        for (name, policy, allowed) in [
            ("strict-system-none", "", false),
            (
                "strict-system-exit",
                "[capabilities.system.exit]\nenabled = true",
                false,
            ),
            (
                "strict-system-panic",
                "[capabilities.testing.panic]\nenabled = true",
                false,
            ),
            (
                "strict-system-both",
                "[capabilities.system.exit]\nenabled = true\n[capabilities.testing.panic]\nenabled = true",
                true,
            ),
        ] {
            let project = Project::new(name, &["System"], source, policy);
            let output = project.hiko(&["check", "--strict", "main.hml"]);
            assert_eq!(
                output.status.success(),
                allowed,
                "{name}: {}",
                stderr(&output)
            );
            if !allowed {
                assert!(
                    stderr(&output).contains("capabilities."),
                    "{}",
                    stderr(&output)
                );
            }
        }
    }

    #[test]
    fn strict_noun_import_checks_every_unused_leaf() {
        let cases: &[(&str, &str, &[&str], bool)] = &[
            (
                "Int",
                "convert",
                &["string_to_int", "int_to_string"],
                cfg!(feature = "builtin-convert"),
            ),
            (
                "Float",
                "convert",
                &["int_to_float", "float_to_string"],
                cfg!(feature = "builtin-convert"),
            ),
            (
                "Char",
                "convert",
                &["int_to_char", "char_to_int"],
                cfg!(feature = "builtin-convert"),
            ),
            (
                "Word",
                "convert",
                &[
                    "int_to_word",
                    "word_to_int",
                    "string_to_word",
                    "word_to_string",
                ],
                cfg!(feature = "builtin-convert"),
            ),
            (
                "Path",
                "path",
                &["path_join"],
                cfg!(feature = "builtin-path"),
            ),
        ];
        for &(module, family, leaves, enabled) in cases {
            if !enabled {
                continue;
            }
            // All leaves permit the import. Omitting each leaf in turn must
            // reject it, despite no public wrapper being called.
            for omitted in 0..=leaves.len() {
                let policy = leaves
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != omitted)
                    .map(|(_, leaf)| format!("[capabilities.{family}.{leaf}]\nenabled = true\n"))
                    .collect::<String>();
                let project = Project::new(
                    &format!("strict-{module}-{omitted}"),
                    &[module],
                    &format!("import Std.{module}\nval x = 1"),
                    &policy,
                );
                let output = project.hiko(&["check", "--strict", "main.hml"]);
                assert_eq!(
                    output.status.success(),
                    omitted == leaves.len(),
                    "{module}/{omitted}: {}",
                    stderr(&output)
                );
                if omitted < leaves.len() {
                    assert!(
                        stderr(&output).contains(leaves[omitted]),
                        "{}",
                        stderr(&output)
                    );
                }
            }
        }
    }

    #[cfg(all(
        feature = "builtin-convert",
        feature = "builtin-path",
        feature = "builtin-system",
        feature = "builtin-testing",
        feature = "builtin-stdio"
    ))]
    #[test]
    fn all_thin_module_signatures_typecheck_from_installed_sources() {
        let project = Project::new(
            "signatures",
            &["Int", "Float", "Char", "Word", "Path", "System"],
            include_str!("../../../tests/run/test_thin_modules.hml"),
            "",
        );
        let output = project.hiko(&["check", "main.hml"]);
        assert!(output.status.success(), "{}", stderr(&output));
    }

    #[test]
    fn system_import_requires_both_cargo_features() {
        if !cfg!(feature = "builtin-system") || !cfg!(feature = "builtin-testing") {
            let project = Project::new(
                "missing-system-feature",
                &["System"],
                "import Std.System\nval x = 1",
                "[capabilities.system.exit]\nenabled = true\n[capabilities.testing.panic]\nenabled = true",
            );
            let output = project.hiko(&["run", "main.hml"]);
            let message = stderr(&output);
            let missing = if !cfg!(feature = "builtin-system") {
                "builtin-system"
            } else {
                "builtin-testing"
            };
            assert!(!output.status.success());
            assert!(
                message.contains("not available in this build") && message.contains(missing),
                "{message}"
            );
        }
    }
}
