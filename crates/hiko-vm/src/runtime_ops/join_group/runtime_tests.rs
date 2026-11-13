use super::*;
use crate::io_backend::{IoBackend, IoRequest, IoResult, IoToken};
use crate::runtime::Runtime;
use crate::threaded::ThreadedRuntime;
use std::time::{Duration, Instant};

/// Re-exec only this test. Hostile scheduling/cleanup regressions have an
/// external hard stop; kill is always followed by wait/reap.
pub(super) fn in_bounded_subprocess(name: &str) -> bool {
    let name = name.strip_prefix("hiko_vm::").unwrap_or(name);
    if std::env::var("HIKO_JOIN_PROBE").as_deref() == Ok(name) {
        return true;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("HIKO_JOIN_PROBE", name)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "bulk child probe failed: {name}");
            return false;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let status = child.wait().unwrap();
            panic!("bulk child probe timed out, killed and reaped: {name}: {status}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// First statement of a bounded test: re-exec the enclosing test in a child
/// and return in the parent. Deriving the probe name from the enclosing
/// function keeps the `--exact` filter from drifting and silently passing.
macro_rules! bounded {
    () => {
        fn probe() {}
        let name = std::any::type_name_of_val(&probe);
        let name = name.strip_suffix("::probe").unwrap();
        if !crate::runtime_ops::join_group::runtime_tests::in_bounded_subprocess(name) {
            return;
        }
    };
}
pub(super) use bounded;

fn assert_report(report: crate::lifecycle_tests::Report, expected: &str) {
    assert_eq!(report.status, expected, "{report:?}");
    assert!(report.leftovers.is_empty(), "{report:?}");
}

fn assert_runtimes(source: &str) {
    let program = crate::test_support::compile_source_hermetic(source);
    for vm in VM::test_engine_vms(&program) {
        let budget = vm.join_group_budget().clone();
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();
        assert_report(runtime.lifecycle_report(pid), "Done");
        assert_eq!(budget.used(), 0);
    }
    for run in 0..40 {
        for vm in VM::test_engine_vms(&program) {
            let budget = vm.join_group_budget().clone();
            let runtime = ThreadedRuntime::new(1 + run % 4);
            let pid = runtime.spawn_root_vm(vm);
            runtime.run_to_completion().unwrap();
            assert_report(runtime.lifecycle_report(pid), "Done");
            assert_eq!(budget.used(), 0);
        }
    }
}

#[test]
fn group_local_invalidity_preserves_unlisted_children_and_reaps_named_members() {
    bounded!();
    assert_runtimes(
        r#"
        import __Builtin.Fiber
        import __Builtin.Testing
        fun forever n = forever (n + 1)
        val unlisted = BuiltinFiber.spawn_raw (fn () => 42)
        val named = BuiltinFiber.spawn_raw (fn () => forever 0)
        val _ = case BuiltinFiber.await_all [named, named] of
            BuiltinFiber.Err BuiltinFiber.AlreadyJoined => ()
          | _ => BuiltinTesting.panic "wrong admission precedence"
        val _ = case BuiltinFiber.await_result unlisted of
            BuiltinFiber.Ok 42 => ()
          | _ => BuiltinTesting.panic "unlisted child was touched"
        val _ = case BuiltinFiber.await_result named of
            BuiltinFiber.Err BuiltinFiber.AlreadyJoined => ()
          | _ => BuiltinTesting.panic "named child was not reaped"
    "#,
    );
}

#[test]
fn foreign_outcomes_remain_with_their_parent_and_reaped_grandchildren_are_truthful() {
    bounded!();
    assert_runtimes(
        r#"
        import __Builtin.Fiber
        import __Builtin.Testing
        val foreign = BuiltinFiber.spawn_raw (fn () => 42)
        val requester = BuiltinFiber.spawn_raw (fn () =>
          case BuiltinFiber.await_all [foreign] of
              BuiltinFiber.Err (BuiltinFiber.RuntimeError _) => 7
            | _ => BuiltinTesting.panic "foreign accepted")
        val _ = if BuiltinFiber.await requester = 7 then () else BuiltinTesting.panic "requester"
        val _ = if BuiltinFiber.await foreign = 42 then () else BuiltinTesting.panic "foreign consumed"
        val owner = BuiltinFiber.spawn_raw (fn () => BuiltinFiber.spawn_raw (fn () => 43))
        val reaped = BuiltinFiber.await owner
        fun check n =
          case BuiltinFiber.await_all [reaped] of
              BuiltinFiber.Err BuiltinFiber.AlreadyJoined => ()
            | BuiltinFiber.Err (BuiltinFiber.RuntimeError _) =>
                if n = 0 then BuiltinTesting.panic "grandchild did not finish cleanup" else check (n - 1)
            | _ => BuiltinTesting.panic "grandchild joined"
        val _ = check 10000
    "#,
    );
}

#[test]
fn admission_quota_parent_failure_cancels_all_children_including_unlisted() {
    bounded!();
    let program = crate::test_support::compile_source_hermetic(
        r#"
        import __Builtin.Fiber
        fun forever n = forever (n + 1)
        val unlisted = BuiltinFiber.spawn_raw (fn () => forever 0)
        val named = BuiltinFiber.spawn_raw (fn () => forever 0)
        val _ = BuiltinFiber.await_all [named]
    "#,
    );
    for mut vm in VM::test_engine_vms(&program) {
        vm.configure_join_group_bytes(0);
        let budget = vm.join_group_budget().clone();
        let mut runtime = Runtime::new();
        let pid = runtime.spawn_root_vm(vm);
        runtime.run_to_completion().unwrap();
        let report = runtime.lifecycle_report(pid);
        assert!(report.status.contains("max_join_group_bytes"), "{report:?}");
        assert!(report.leftovers.is_empty(), "{report:?}");
        assert_eq!(budget.used(), 0);
    }
    for run in 0..20 {
        for mut vm in VM::test_engine_vms(&program) {
            vm.configure_join_group_bytes(0);
            let budget = vm.join_group_budget().clone();
            let runtime = ThreadedRuntime::new(1 + run % 4);
            let pid = runtime.spawn_root_vm(vm);
            runtime.run_to_completion().unwrap();
            let report = runtime.lifecycle_report(pid);
            assert!(report.status.contains("max_join_group_bytes"), "{report:?}");
            assert!(report.leftovers.is_empty(), "{report:?}");
            assert_eq!(budget.used(), 0);
        }
    }
}

/// Release the root's timer ONLY after a child has actually admitted a group.
/// No sleeps/assumed worker ordering are used to prove cancellation coverage.
struct AdmittedGroupIo {
    budget: Arc<SharedBudget>,
    timer: Mutex<Option<IoToken>>,
}

impl IoBackend for AdmittedGroupIo {
    fn register(&self, token: IoToken, request: IoRequest) -> Result<(), String> {
        if !matches!(request, IoRequest::Sleep(_)) {
            return Err("unexpected request".into());
        }
        *self.timer.lock().unwrap() = Some(token);
        Ok(())
    }
    fn poll(&self) -> Vec<(IoToken, IoResult)> {
        if self.budget.used() == 0 {
            return Vec::new();
        }
        self.timer
            .lock()
            .unwrap()
            .take()
            .map(|token| {
                (
                    token,
                    IoResult::Ok {
                        value: SendableValue::Unit,
                        io_bytes: 0,
                    },
                )
            })
            .into_iter()
            .collect()
    }
    fn cancel(&self, token: IoToken) {
        let mut timer = self.timer.lock().unwrap();
        if *timer == Some(token) {
            *timer = None;
        }
    }
}

#[cfg(all(unix, feature = "builtin-exec"))]
mod exec_barrier {
    use super::*;
    use crate::io_backend::ThreadPoolIoBackend;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Backend {
        inner: ThreadPoolIoBackend,
        exec_token: Mutex<Option<IoToken>>,
        failure_timer: Mutex<Option<IoToken>>,
        registered: AtomicBool,
        cleaned: Arc<AtomicBool>,
    }
    impl IoBackend for Backend {
        fn register(&self, token: IoToken, request: IoRequest) -> Result<(), String> {
            match request {
                IoRequest::Exec(job) => {
                    *self.exec_token.lock().unwrap() = Some(token);
                    self.inner.register(token, IoRequest::Exec(job))?;
                    self.registered.store(true, Ordering::SeqCst);
                    Ok(())
                }
                IoRequest::Sleep(_) => {
                    *self.failure_timer.lock().unwrap() = Some(token);
                    Ok(())
                }
                request => self.inner.register(token, request),
            }
        }
        fn poll(&self) -> Vec<(IoToken, IoResult)> {
            let mut completions = self.inner.poll();
            if self.registered.load(Ordering::SeqCst)
                && let Some(token) = self.failure_timer.lock().unwrap().take()
            {
                completions.push((
                    token,
                    IoResult::Ok {
                        value: SendableValue::Unit,
                        io_bytes: 0,
                    },
                ));
            }
            completions
        }
        fn cancel(&self, token: IoToken) {
            self.inner.cancel(token); // real kill/reap/output-reader barrier
            if *self.exec_token.lock().unwrap() == Some(token) {
                self.cleaned.store(true, Ordering::SeqCst);
            }
            let mut timer = self.failure_timer.lock().unwrap();
            if *timer == Some(token) {
                *timer = None;
            }
        }
    }
    struct Sink(Arc<AtomicBool>);
    impl crate::vm::OutputSink for Sink {
        fn write(&self, _: &str) -> std::io::Result<()> {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(std::io::Error::other(
                    "parent resumed before exec cleanup barrier",
                ))
            }
        }
    }

    #[test]
    fn group_failure_waits_for_real_exec_backend_cleanup_before_parent_resumes() {
        bounded!();
        let program = crate::test_support::compile_source_hermetic(
            r#"
            import __Builtin.Fiber
            import __Builtin.Exec
            import __Builtin.Time
            import __Builtin.Testing
            import __Builtin.Stdio
            val slow = BuiltinFiber.spawn_raw (fn () => BuiltinExec.exec ("/bin/sleep", ["5"]))
            val failed = BuiltinFiber.spawn_raw (fn () => let val _ = BuiltinTime.sleep 0 in BuiltinTesting.panic "chosen" end)
            val _ = case BuiltinFiber.await_all [slow, failed] of
                BuiltinFiber.Err (BuiltinFiber.RuntimeError "chosen") => BuiltinStdio.println "resumed"
              | _ => BuiltinTesting.panic "wrong cause"
        "#,
        );
        for workers in [1, 4] {
            for mut vm in VM::test_engine_vms(&program) {
                vm.set_exec_policy(vec!["/bin/sleep".into()], vec![]);
                let cleaned = Arc::new(AtomicBool::new(false));
                vm.set_output_sink(Arc::new(Sink(cleaned.clone())));
                let backend = Arc::new(Backend {
                    inner: ThreadPoolIoBackend::new(2),
                    exec_token: Mutex::new(None),
                    failure_timer: Mutex::new(None),
                    registered: AtomicBool::new(false),
                    cleaned: cleaned.clone(),
                });
                let runtime = ThreadedRuntime::new(workers).with_io_backend(backend);
                let pid = runtime.spawn_root_vm(vm);
                let start = Instant::now();
                runtime.run_to_completion().unwrap();
                assert_report(runtime.lifecycle_report(pid), "Done");
                assert!(cleaned.load(Ordering::SeqCst));
                assert!(
                    start.elapsed() < Duration::from_secs(2),
                    "bulk failed to cancel exec promptly"
                );
            }
        }
    }
}

#[test]
fn parent_cancellation_drains_active_group_and_unlisted_children() {
    bounded!();
    let program = crate::test_support::compile_source_hermetic(
        r#"
        import __Builtin.Fiber
        import __Builtin.Time
        import __Builtin.Testing
        fun forever n = forever (n + 1)
        val owner = BuiltinFiber.spawn_raw (fn () => let
            val unlisted = BuiltinFiber.spawn_raw (fn () => forever 0)
            val named = BuiltinFiber.spawn_raw (fn () => forever 0)
          in BuiltinFiber.await_all [named] end)
        val _ = BuiltinTime.sleep 0
        val _ = BuiltinFiber.cancel_raw owner
        val _ = case BuiltinFiber.await_result owner of
            BuiltinFiber.Err BuiltinFiber.Cancelled => ()
          | _ => BuiltinTesting.panic "owner did not cancel"
    "#,
    );
    for run in 0..20 {
        for vm in VM::test_engine_vms(&program) {
            let budget = vm.join_group_budget().clone();
            let backend = Arc::new(AdmittedGroupIo {
                budget: budget.clone(),
                timer: Mutex::new(None),
            });
            let runtime = ThreadedRuntime::new(1 + run % 4).with_io_backend(backend);
            let pid = runtime.spawn_root_vm(vm);
            runtime.run_to_completion().unwrap();
            assert_report(runtime.lifecycle_report(pid), "Done");
            assert_eq!(budget.used(), 0);
        }
    }
}
