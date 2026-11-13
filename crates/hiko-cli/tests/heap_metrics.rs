//! Every execution is an isolated process; even exit(0) must not export.
#![cfg(all(
    feature = "builtin-stdio",
    feature = "builtin-system",
    feature = "builtin-testing",
    feature = "builtin-process"
))]
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
static ID: AtomicU64 = AtomicU64::new(0);
// The eager __Builtin.Stdio module body names every stdio builtin.
const STDIO: &str = "[capabilities.stdio.print]\nenabled = true\n[capabilities.stdio.println]\nenabled = true\n[capabilities.stdio.eprint]\nenabled = true\n[capabilities.stdio.eprintln]\nenabled = true\n[capabilities.stdio.read_line]\nenabled = true\n[capabilities.stdio.read_stdin]\nenabled = true\n";
const STDIO_IMPORT: &str = "import __Builtin.Stdio\n";
struct Project(PathBuf);
impl Project {
    fn new(source: &str, policy: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "hiko-heap-metrics-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("hiko.toml"),
            "[defaults]\npolicy = \"test\"\n[policies.test]\npath = \"policy.toml\"\n",
        )
        .unwrap();
        fs::write(path.join("policy.toml"), policy).unwrap();
        fs::write(path.join("main.hml"), source).unwrap();
        Self(path)
    }
    fn artifact(&self) -> PathBuf {
        self.0.join("metrics.json")
    }
    fn command(&self, collect: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hiko-cli"));
        command
            .current_dir(&self.0)
            .args(["run", "main.hml"])
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in [
            "HIKO_HEAP_METRICS",
            "HIKO_HEAP_METRICS_NONCE",
            "HIKO_HEAP_METRICS_PROFILE",
        ] {
            command.env_remove(name);
        }
        if collect {
            command
                .env("HIKO_HEAP_METRICS", self.artifact())
                .env("HIKO_HEAP_METRICS_NONCE", "fresh-test-232")
                .env("HIKO_HEAP_METRICS_PROFILE", "untimed-test");
        }
        command
    }
    fn run(&self, collect: bool) -> Output {
        run(self.command(collect))
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn run(mut command: Command) -> Output {
    wait(command.spawn().unwrap())
}
fn wait(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "CLI timed out (killed/reaped): {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(not(feature = "heap-metrics"))]
#[test]
fn feature_off_ignores_metrics_environment_and_preserves_stdout() {
    let project = Project::new(
        &format!("{STDIO_IMPORT}val _ = BuiltinStdio.println \"metrics-ok\"\n"),
        STDIO,
    );
    let plain = project.run(false);
    let configured = project.run(true);
    success(&plain);
    success(&configured);
    assert_eq!(plain.stdout, configured.stdout);
    assert_eq!(plain.stderr, configured.stderr);
    assert!(!project.artifact().exists());
}

#[cfg(feature = "heap-metrics")]
mod enabled {
    use super::*;
    use hiko_vm::heap_metrics::{Artifact, read_artifact};
    fn wait_ready(child: &mut Child, path: &std::path::Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while fs::metadata(path).unwrap().len() == 0 && Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        fs::metadata(path).unwrap().len() > 0
    }
    fn artifact(project: &Project) -> Artifact {
        read_artifact(&project.artifact(), "fresh-test-232", "untimed-test").unwrap()
    }
    #[test]
    fn normal_return_exports_one_balanced_root_without_stdout_changes() {
        let project = Project::new(
            &format!("{STDIO_IMPORT}val p = (1, 2)\nval _ = BuiltinStdio.println \"metrics-ok\"\n"),
            STDIO,
        );
        let plain = project.run(false);
        let output = project.run(true);
        success(&plain);
        success(&output);
        assert_eq!(output.stdout, b"metrics-ok\n");
        assert_eq!(output.stdout, plain.stdout);
        assert_eq!(output.stderr, plain.stderr);
        let metrics = artifact(&project);
        assert_eq!(metrics.schema_version, 1);
        assert!(metrics.complete);
        assert_eq!(metrics.heaps_created, 1);
        assert_eq!(metrics.heaps_dropped, 1);
        assert_eq!(metrics.heaps_active, 0);
        assert!(metrics.successful_gc_object_allocations > 0);
        assert!(metrics.max_per_heap_estimated_retained_bytes > 0);
        assert_eq!(
            metrics.heaps[0].successful_gc_object_allocations,
            metrics.successful_gc_object_allocations
        );
        assert!(read_artifact(&project.artifact(), "stale-nonce", "untimed-test").is_err());
        assert!(read_artifact(&project.artifact(), "fresh-test-232", "wrong-profile").is_err());
    }
    const PROCESS: &str = "[capabilities.fiber.spawn]\nenabled=true\n[capabilities.fiber.join_or_fail]\nenabled=true\n[capabilities.fiber.join]\nenabled=true\n[capabilities.fiber.cancel]\nenabled=true\n[capabilities.system.panic]\nenabled=true\n[capabilities.testing.check]\nenabled=true\n[capabilities.testing.equal]\nenabled=true\n";
    // Per-fixture heap accounting lives in the hiko-vm integration test; this
    // proves the CLI reaps an abandoned child before exporting balanced evidence.
    #[test]
    fn threaded_run_reaps_abandoned_child_and_exports_balanced_evidence() {
        for workers in [1, 2] {
            let source = "import __Builtin.Fiber\nfun loop n = loop (n + 1)\nval p = BuiltinFiber.spawn_raw (fn () => loop 0)";
            let project = Project::new(source, PROCESS);
            let mut command = project.command(true);
            command.args(["--workers", &workers.to_string()]);
            success(&run(command));
            let metrics = artifact(&project);
            assert_eq!(metrics.heaps_created, 2, "{source}");
            assert_eq!(metrics.heaps_dropped, 2);
            assert_eq!(metrics.heaps_active, 0);
        }
    }
    #[test]
    fn failures_and_early_exit_never_leave_old_complete_evidence() {
        for (source, policy, expected) in [
            (
                "import __Builtin.System\nval _ = BuiltinSystem.exit 0",
                "[capabilities.system.exit]\nenabled=true\n",
                Some(0),
            ),
            (
                "import __Builtin.System\nval _ = BuiltinSystem.exit 3",
                "[capabilities.system.exit]\nenabled=true\n",
                Some(3),
            ),
            (
                "import __Builtin.Testing\nval _ = BuiltinTesting.panic \"sentinel\"",
                "[capabilities.system.panic]\nenabled=true\n[capabilities.testing.check]\nenabled=true\n[capabilities.testing.equal]\nenabled=true\n",
                Some(1),
            ),
            ("val x =", "", Some(1)),
            (
                "import __Builtin.Fiber\nimport __Builtin.Testing\nval p = BuiltinFiber.spawn_raw (fn () => BuiltinTesting.panic \"child\")\nval _ = BuiltinFiber.await p",
                PROCESS,
                Some(1),
            ),
        ] {
            let project = Project::new(source, policy);
            // Reuse exactly the same identity to show startup invalidation,
            // rather than relying only on a caller noticing a nonce mismatch.
            fs::write(project.0.join("main.hml"), "val x = 1").unwrap();
            success(&project.run(true));
            artifact(&project);
            fs::write(project.0.join("main.hml"), source).unwrap();
            let output = project.run(true);
            assert_eq!(
                output.status.code(),
                expected,
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                !project.artifact().exists(),
                "{source} retained complete evidence"
            );
        }
    }
    #[test]
    fn invalid_partial_configuration_and_missing_parent_fail_actionably() {
        let project = Project::new("val x = 1", "");
        let mut partial = project.command(false);
        partial.env("HIKO_HEAP_METRICS", project.artifact());
        let output = run(partial);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("together"));
        for (nonce, profile) in [("", "ok"), ("ok", "bad profile"), (&"x".repeat(129), "ok")] {
            let mut command = project.command(true);
            command
                .env("HIKO_HEAP_METRICS_NONCE", nonce)
                .env("HIKO_HEAP_METRICS_PROFILE", profile);
            assert!(!run(command).status.success());
        }
        let mut missing = project.command(true);
        missing.env("HIKO_HEAP_METRICS", project.0.join("missing/output.json"));
        let output = run(missing);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("parent"));
        fs::create_dir(project.artifact()).unwrap();
        assert!(!project.run(true).status.success());
    }
    #[cfg(unix)]
    #[test]
    fn destination_symlink_is_rejected_without_touching_target() {
        let project = Project::new("val x = 1", "");
        let target = project.0.join("untouched.json");
        fs::write(&target, "keep").unwrap();
        std::os::unix::fs::symlink(&target, project.artifact()).unwrap();
        let output = project.run(true);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"));
        assert_eq!(fs::read_to_string(target).unwrap(), "keep");
    }
    #[test]
    fn post_run_export_errors_are_nonzero_and_actionable() {
        for rename_error in [false, true] {
            let project = Project::new(
                "import __Builtin.Stdio\nval _ = BuiltinStdio.println \"ready\"\nval _ = BuiltinStdio.read_line ()",
                STDIO,
            );
            let ready = project.0.join("ready.log");
            let destination = if rename_error {
                project.artifact()
            } else {
                fs::create_dir(project.0.join("output")).unwrap();
                project.0.join("output/result.json")
            };
            let mut command = project.command(true);
            command
                .env("HIKO_HEAP_METRICS", &destination)
                .stdin(Stdio::piped())
                .stdout(fs::File::create(&ready).unwrap());
            let mut child = command.spawn().unwrap();
            if !wait_ready(&mut child, &ready) {
                let _ = child.kill();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "no ready handshake: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            if rename_error {
                fs::create_dir(&destination).unwrap();
            } else {
                fs::remove_dir(destination.parent().unwrap()).unwrap();
            }
            use std::io::Write;
            child.stdin.take().unwrap().write_all(b"release\n").unwrap();
            let output = wait(child);
            assert!(!output.status.success());
            let message = String::from_utf8_lossy(&output.stderr);
            assert!(message.contains("heap-metrics:"), "{message}");
            assert!(
                message.contains(if rename_error { "rename" } else { "create" }),
                "{message}"
            );
            assert!(read_artifact(&destination, "fresh-test-232", "untimed-test").is_err());
        }
    }
    #[test]
    fn signal_and_controller_timeout_kill_and_reap_without_export() {
        for timeout in [false, true] {
            let project = Project::new(
                "import __Builtin.Stdio\nval _ = BuiltinStdio.println \"ready\"\nfun loop n = loop (n + 1)\nval _ = loop 0",
                STDIO,
            );
            let ready = project.0.join("ready.log");
            let mut command = project.command(true);
            command.stdout(fs::File::create(&ready).unwrap());
            let mut child = command.spawn().unwrap();
            let observed_ready = wait_ready(&mut child, &ready);
            if timeout {
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            assert!(
                observed_ready,
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!output.status.success());
            assert!(!project.artifact().exists());
        }
    }
    #[test]
    fn collection_does_not_grant_script_capabilities_or_hide_policy_failure() {
        let project = Project::new(
            "import __Builtin.Stdio\nval _ = BuiltinStdio.println \"not-permitted\"",
            "",
        );
        let plain = project.run(false);
        let collected = project.run(true);
        assert!(!plain.status.success());
        assert_eq!(collected.status.code(), plain.status.code());
        assert_eq!(collected.stdout, plain.stdout);
        assert_eq!(collected.stderr, plain.stderr);
        assert!(!project.artifact().exists());
    }
    #[test]
    fn scalar_run_includes_an_empty_root_heap() {
        let project = Project::new("val x = 1\n", "");
        success(&project.run(true));
        let metrics = artifact(&project);
        assert_eq!(metrics.heaps_created, 1);
        assert_eq!(metrics.heaps[0].successful_gc_object_allocations, 0);
        assert_eq!(metrics.heaps[0].estimated_retained_bytes_peak, 0);
    }
}
