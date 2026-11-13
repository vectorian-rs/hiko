#![cfg(all(unix, feature = "builtin-default"))]

mod common;

use hiko_vm::builder::{ExecPolicy, VMBuilder};
use hiko_vm::threaded::ThreadedRuntime;
use std::time::{Duration, Instant};

fn exec_vm(source: &str) -> VMBuilder {
    VMBuilder::new(common::compile_source(source))
        .with_core()
        .register_builtin_name("spawn")
        .register_builtin_name("await_process")
        .register_builtin_name("cancel")
        .with_exec(ExecPolicy {
            allowed: vec!["/bin/sleep".into(), "/usr/bin/printf".into()],
            rules: vec![],
        })
        // Bound a regression independently of the prompt-cancellation assertion.
        .exec_timeout_ms(5_000)
}

#[test]
fn failed_child_cancels_a_sixty_second_exec_sibling_promptly() {
    for workers in [1, 4] {
        let vm = exec_vm(
            "import __Builtin.Exec
             import __Builtin.Fiber
             import __Builtin.Testing
             import __Builtin.Time
             val slow = BuiltinFiber.spawn_raw (fn () => BuiltinExec.exec (\"/bin/sleep\", [\"60\"]))
             val failed = BuiltinFiber.spawn_raw (fn () => let val _ = BuiltinTime.sleep 100 in BuiltinTesting.panic \"expected failure\" end)
             val _ = BuiltinFiber.await failed",
        )
        .build();
        let runtime = ThreadedRuntime::new(workers);
        let root = runtime.spawn_root_vm(vm);
        let start = Instant::now();
        runtime.run_to_completion().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{workers} worker(s): cleanup delayed by exec"
        );
        assert!(format!("{:?}", runtime.failure(root)).contains("expected failure"));
    }
}

#[test]
fn threaded_exec_preserves_output_and_cumulative_limits() {
    for workers in [1, 4] {
        let source = "import __Builtin.Exec\nval result = BuiltinExec.exec (\"/usr/bin/printf\", [\"123456789\"])";
        for (output_limit, io_limit, expected) in [
            (8, 100, "max_exec_output_bytes"),
            (100, 8, "io limit exceeded"),
        ] {
            let vm = exec_vm(source)
                .max_exec_output_bytes(output_limit)
                .max_io_bytes(io_limit)
                .build();
            let runtime = ThreadedRuntime::new(workers);
            let root = runtime.spawn_root_vm(vm);
            runtime.run_to_completion().unwrap();
            let error = format!("{:?}", runtime.failure(root));
            assert!(error.contains(expected), "{error}");
        }
    }
}

#[test]
fn late_exec_completion_cannot_resume_a_cancelled_child() {
    for workers in [1, 4] {
        let vm = exec_vm(
            "import __Builtin.Exec
             import __Builtin.Fiber
             import __Builtin.Stdio
             import __Builtin.Time
             val child = BuiltinFiber.spawn_raw (fn () => let
                 val _ = BuiltinExec.exec (\"/bin/sleep\", [\"60\"])
                 val _ = BuiltinStdio.println \"must not resume\"
             in 1 end)
             val _ = BuiltinTime.sleep 50
             val _ = BuiltinFiber.cancel_raw child
             val _ = BuiltinTime.sleep 50
             val _ = BuiltinStdio.println \"done\"",
        )
        .build();
        let runtime = ThreadedRuntime::new(workers);
        let root = runtime.spawn_root_vm(vm);
        assert_eq!(runtime.run_to_completion().unwrap(), ["done\n"]);
        assert!(runtime.failure(root).is_none());
    }
}

#[test]
fn exec_does_not_block_a_vm_worker_or_unrelated_timer() {
    for workers in [1, 4] {
        let program = common::compile_source(
            "import __Builtin.Exec
             import __Builtin.Fiber
             import __Builtin.Int
             import __Builtin.Stdio
             import __Builtin.Time
             val started = BuiltinTime.monotonic_ms ()
             val slow = BuiltinFiber.spawn_raw (fn () => BuiltinExec.exec (\"/bin/sleep\", [\"1\"]))
             val timer = BuiltinFiber.spawn_raw (fn () => let val _ = BuiltinTime.sleep 10 in BuiltinTime.monotonic_ms () - started end)
             val _ = BuiltinStdio.println (BuiltinInt.to_string (BuiltinFiber.await timer))
             val _ = BuiltinFiber.await slow",
        );
        let vm = VMBuilder::new(program)
            .with_core()
            .register_builtin_name("exec")
            .register_builtin_name("spawn")
            .register_builtin_name("await_process")
            .with_exec(ExecPolicy {
                allowed: vec!["/bin/sleep".into()],
                rules: vec![],
            })
            .build();
        let runtime = ThreadedRuntime::new(workers);
        let root = runtime.spawn_root_vm(vm);
        let output = runtime.run_to_completion().unwrap();
        assert!(
            runtime.failure(root).is_none(),
            "{:?}",
            runtime.failure(root)
        );
        let elapsed: u64 = output[0].trim().parse().unwrap();
        assert!(
            elapsed < 500,
            "{workers} worker(s): timer delayed {elapsed}ms by exec"
        );
    }
}
