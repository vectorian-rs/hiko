//! The examples that must fail: the `should_fail_io_*` scripts under the
//! `should-fail-io` policy, and the `use` cycle in `cycle_a.hml`.
//! `tools/run_all.hml` skips them, so this is what checks them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Populate the repository's `.hiko-cache` from `libraries/`, exactly as
/// `hiko install` would from the local registry, so example runs resolve
/// named imports without network or machine-global state. Idempotent.
fn populate_module_cache() {
    let root = repo_root();
    let cache = root.join(".hiko-cache");
    for package in std::fs::read_dir(root.join("libraries")).unwrap() {
        let package = package.unwrap();
        let name = package.file_name().to_string_lossy().into_owned();
        let Some((_, _version)) = name.split_once("-v") else {
            continue; // _assets and loose files are not packages
        };
        let modules = package.path().join("modules");
        if !modules.is_dir() {
            continue;
        }
        let base = format!("http://127.0.0.1:8000/{name}/modules");
        let mut pending = vec![modules.clone()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "hml") {
                    continue;
                }
                let bytes = std::fs::read(&path).unwrap();
                let hash = hiko_common::blake3_hex(&bytes);
                let relative = path
                    .strip_prefix(&modules)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let url = format!("{base}/{relative}");
                let target = hiko_compile::compiler::module_cache_path(&cache, &url, &hash);
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::write(target, bytes).unwrap();
            }
        }
    }
}

/// Runs `script` under `policy` with `stdin` and returns (success, stderr).
fn run(script: &str, policy: &str, stdin: &[u8]) -> (bool, String) {
    populate_module_cache();
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
                "[capabilities.system.panic]\nenabled = true",
                None,
                "error: sentinel",
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
                "[capabilities.system.panic]\nenabled = true",
                false,
            ),
            (
                "strict-system-both",
                "[capabilities.system.exit]\nenabled = true\n[capabilities.system.panic]\nenabled = true",
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
        let cases: &[(&str, &[(&str, &str)], bool)] = &[
            (
                "Int",
                &[
                    ("int.abs", "abs_int"),
                    ("int.of_string", "string_to_int"),
                    ("int.to_string", "int_to_string"),
                ],
                cfg!(feature = "builtin-convert"),
            ),
            (
                "Float",
                &[
                    ("float.abs", "abs_float"),
                    ("float.ceil", "ceil"),
                    ("float.floor", "floor"),
                    ("float.of_int", "int_to_float"),
                    ("float.sqrt", "sqrt"),
                    ("float.to_string", "float_to_string"),
                ],
                cfg!(feature = "builtin-convert"),
            ),
            (
                "Char",
                &[
                    ("char.of_int", "int_to_char"),
                    ("char.to_int", "char_to_int"),
                ],
                cfg!(feature = "builtin-convert"),
            ),
            (
                "Word",
                &[
                    ("word.of_int", "int_to_word"),
                    ("word.to_int", "word_to_int"),
                    ("word.of_string", "string_to_word"),
                    ("word.to_string", "word_to_string"),
                ],
                cfg!(feature = "builtin-convert"),
            ),
            (
                "Path",
                &[("path.join", "path_join")],
                cfg!(feature = "builtin-path"),
            ),
        ];
        for &(module, leaves, enabled) in cases {
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
                    .map(|(_, (key, _))| format!("[capabilities.{key}]\nenabled = true\n"))
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
                        stderr(&output).contains(leaves[omitted].1),
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
            &[
                "Int", "Float", "Char", "Word", "Path", "System", "Stdio", "Testing",
            ],
            include_str!("../../../tests/run/test_thin_modules.hml"),
            "",
        );
        let output = project.hiko(&["check", "main.hml"]);
        assert!(output.status.success(), "{}", stderr(&output));
    }

    /// All bulk runtime probes have a hard deadline and kill AND reap on
    /// expiry; they cannot hang the test runner on a lost-wakeup regression.
    fn hiko_bounded(project: &Project, args: &[&str]) -> Output {
        use std::process::Stdio;
        use std::time::{Duration, Instant};
        let mut child = Command::new(env!("CARGO_BIN_EXE_hiko-cli"))
            .current_dir(&project.0)
            .env("NO_COLOR", "1")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "bulk probe timed out and was killed/reaped: {}",
                    stderr(&output)
                );
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[cfg(feature = "builtin-process")]
    #[test]
    fn bulk_only_fiber_import_and_calls_do_not_grant_individual_leaves() {
        let policy = "[capabilities.fiber.spawn]\nenabled = true\n[capabilities.fiber.join_all]\nenabled = true\n";
        let source = "import Std.Fiber\nval result = case Fiber.all [fn () => 10, fn () => 20] of Result.Ok [10, 20] => 42 | _ => 1 / 0\nval both : (int * string, Fiber.error) Result.result = Fiber.both (fn () => 3, fn () => \"different type\")\n";
        let project = Project::new(
            "fiber-bulk-only",
            &["Fiber", "Either", "Result"],
            source,
            policy,
        );
        for workers in ["1", "4"] {
            let output = hiko_bounded(&project, &["run", "--workers", workers, "main.hml"]);
            assert!(output.status.success(), "{}", stderr(&output));
        }
        // The exact same imported bodies remain subject to strict checks.
        let output = hiko_bounded(&project, &["check", "--strict", "main.hml"]);
        assert!(
            !output.status.success(),
            "strict must reject denied unused leaves"
        );
        assert!(
            stderr(&output).contains("await_process") || stderr(&output).contains("cancel"),
            "{}",
            stderr(&output)
        );
        let spawn_only = "[capabilities.fiber.spawn]\nenabled = true\n";
        for (name, call, expected, policy) in [
            (
                "no-bulk",
                "Fiber.all [fn () => 10]",
                "fiber_await_all",
                spawn_only,
            ),
            (
                "no-await",
                "Fiber.join (Fiber.spawn (fn () => 10))",
                "await_process_result",
                policy,
            ),
            (
                "no-cancel",
                "Fiber.cancel (Fiber.spawn (fn () => 10))",
                "cancel",
                policy,
            ),
            (
                "no-wait",
                "Fiber.first (fn () => 10, fn () => 20)",
                "wait_any",
                policy,
            ),
        ] {
            let project = Project::new(
                name,
                &["Fiber", "Either", "Result"],
                &format!("import Std.Fiber\nval result = {call}\n"),
                policy,
            );
            let output = hiko_bounded(&project, &["run", "main.hml"]);
            assert!(!output.status.success());
            assert!(
                stderr(&output).contains(expected),
                "{name}: {}",
                stderr(&output)
            );
        }
    }

    #[cfg(all(feature = "builtin-process", feature = "builtin-testing"))]
    #[test]
    fn bulk_ceiling_does_not_change_any_and_all_checks_before_first_spawn() {
        let limits = "[limits]\nmax_fuel = 1000000000\nmax_heap = 2000000\nmax_host_work = 100000000\nmax_live_processes = 20000\nmax_spawns = 10001\nmax_join_group_bytes = 0\n";
        let leaves = "[capabilities.fiber.spawn]\nenabled = true\n[capabilities.fiber.join_or_fail]\nenabled = true\n[capabilities.fiber.join]\nenabled = true\n[capabilities.fiber.cancel]\nenabled = true\n[capabilities.fiber.wait_any]\nenabled = true\n[capabilities.system.panic]\nenabled = true\n";
        let build = "import Std.Fiber\nimport Std.System\nfun build n acc = if n = 0 then acc else build (n - 1) ((fn () => 1) :: acc)\n";
        // No bulk grant, and zero group bytes: any retains its old policy.
        let project = Project::new(
            "fiber-any-10001",
            &["Fiber", "Either", "Result", "System"],
            &format!(
                "{build}val result = case Fiber.any (build 10001 []) of Result.Ok 1 => 42 | _ => System.panic \"any failed\"\n"
            ),
            &format!("{limits}{leaves}"),
        );
        let output = hiko_bounded(&project, &["run", "--workers", "1", "main.hml"]);
        assert!(
            output.status.success(),
            "any must not inherit bulk ceiling: {}",
            stderr(&output)
        );
        // Even with zero spawn allowance, oversized all fails on the bulk
        // input precheck before invoking spawn or reserving a group.
        let project = Project::new(
            "fiber-all-10001",
            &["Fiber", "Either", "Result", "System"],
            &format!("{build}val result = Fiber.all (build 10001 [])\n"),
            &format!(
                "{}{}[capabilities.fiber.join_all]\nenabled = true\n",
                limits.replace("max_spawns = 10001", "max_spawns = 0"),
                leaves
            ),
        );
        let output = hiko_bounded(&project, &["run", "--workers", "1", "main.hml"]);
        assert!(!output.status.success());
        assert!(
            stderr(&output).contains("Fiber.all: at most 10000 entries allowed"),
            "{}",
            stderr(&output)
        );
        assert!(
            !stderr(&output).contains("max_spawns"),
            "spawn occurred before all precheck: {}",
            stderr(&output)
        );
        // The exact boundary is accepted when the independent group policy
        // and existing spawn/work/heap policies all permit it.
        let project = Project::new(
            "fiber-all-10000",
            &["Fiber", "Either", "Result", "System"],
            &format!(
                "{build}fun count xs n = case xs of [] => n | _ :: rest => count rest (n + 1)\nval result = case Fiber.all (build 10000 []) of Result.Ok values => if count values 0 = 10000 then 42 else System.panic \"length\" | _ => System.panic \"all failed\"\n"
            ),
            &format!(
                "{}{}[capabilities.fiber.join_all]\nenabled = true\n",
                limits.replace(
                    "max_join_group_bytes = 0",
                    "max_join_group_bytes = 67108864"
                ),
                leaves
            ),
        );
        let output = hiko_bounded(&project, &["run", "--workers", "1", "main.hml"]);
        assert!(
            output.status.success(),
            "exact bulk boundary rejected: {}",
            stderr(&output)
        );
    }

    #[cfg(all(
        feature = "builtin-process",
        feature = "builtin-testing",
        feature = "builtin-convert"
    ))]
    #[test]
    fn strict_fiber_registration_checks_every_leaf_including_bulk() {
        let leaves = [
            ("fiber.spawn", "spawn"),
            ("fiber.join_or_fail", "await_process"),
            ("fiber.join", "await_process_result"),
            ("fiber.cancel", "cancel"),
            ("fiber.wait_any", "wait_any"),
            ("fiber.join_all", "fiber_await_all"),
            ("system.panic", "panic"),
            ("int.abs", "abs_int"),
            ("int.of_string", "string_to_int"),
            ("int.to_string", "int_to_string"),
        ];
        for omitted in 0..=leaves.len() {
            let policy = leaves
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != omitted)
                .map(|(_, (leaf, _))| format!("[capabilities.{leaf}]\nenabled = true\n"))
                .collect::<String>();
            let project = Project::new(
                &format!("strict-fiber-{omitted}"),
                &["Fiber", "Either", "Result"],
                "import Std.Fiber\nval x = 1",
                &policy,
            );
            let output = hiko_bounded(&project, &["check", "--strict", "main.hml"]);
            assert_eq!(
                output.status.success(),
                omitted == leaves.len(),
                "{omitted}: {}",
                stderr(&output)
            );
            if omitted < leaves.len() {
                assert!(
                    stderr(&output).contains(leaves[omitted].1),
                    "{}",
                    stderr(&output)
                );
            }
        }
    }

    #[test]
    fn system_import_requires_both_cargo_features() {
        if !cfg!(feature = "builtin-system") || !cfg!(feature = "builtin-testing") {
            let project = Project::new(
                "missing-system-feature",
                &["System"],
                "import Std.System\nval x = 1",
                "[capabilities.system.exit]\nenabled = true\n[capabilities.system.panic]\nenabled = true",
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
