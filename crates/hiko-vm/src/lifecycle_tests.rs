//! Process lifecycle tests shared by the single-threaded and threaded runtimes.
//!
//! Each scenario runs the same program on both runtimes and asserts the same
//! observable result (root output and root status) and that no bookkeeping is
//! left behind. Threaded scenarios also run 200 times, cycling through 1 to 4
//! workers.
//!
//! Programs never depend on timing. A child that must still be running when
//! its parent acts is a [`leaf`]: a long stretch of straight-line work
//! followed by I/O. On the threaded runtime that I/O goes to a backend that
//! never completes it, so the leaf can only end by being cancelled. On the
//! single-threaded runtime the FIFO scheduler gives each process 1000
//! reductions per slice, so the leaf is still in its straight-line work when
//! it is cancelled.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;

use crate::io_backend::{IoBackend, IoRequest, IoResult, IoToken};
use crate::runtime::Runtime;
use crate::threaded::ThreadedRuntime;

/// What a test can observe about a finished run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    /// Output captured by the root process.
    pub(crate) output: Vec<String>,
    /// Debug rendering of the root's final `ProcessStatus`.
    pub(crate) status: String,
    /// Bookkeeping that should be empty after a run, as `name=len`, plus any
    /// invariant violation.
    pub(crate) leftovers: Vec<String>,
}

const THREADED_RUNS: usize = 200;

/// Compile on a thread with a large stack: [`leaf`] bodies are long `let`
/// blocks, and the front end recurses once per binding.
fn compile(source: &str) -> CompiledProgram {
    let source = source.to_string();
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let tokens = Lexer::new(&source, 0).tokenize().unwrap();
            let program = Parser::new(tokens).parse_program().unwrap();
            let (compiled, _) = Compiler::compile(program).unwrap();
            compiled
        })
        .unwrap()
        .join()
        .unwrap()
}

fn run_single(program: &CompiledProgram) -> Report {
    let mut runtime = Runtime::new();
    let pid = runtime.spawn_root(program.clone());
    runtime.run_to_completion().unwrap();
    runtime.lifecycle_report(pid)
}

/// Which I/O backend a threaded run uses.
#[derive(Clone, Copy)]
enum Io {
    /// The default thread-pool backend.
    Real,
    /// A backend that never completes a request.
    Stuck,
}

fn run_threaded(
    program: &CompiledProgram,
    workers: usize,
    backend: Option<Arc<dyn IoBackend>>,
) -> Report {
    let mut runtime = ThreadedRuntime::new(workers);
    if let Some(backend) = backend {
        runtime = runtime.with_io_backend(backend);
    }
    let pid = runtime.spawn_root(program.clone());
    runtime.run_to_completion().unwrap();
    runtime.lifecycle_report(pid)
}

/// Run the program built by `source` on both runtimes and check they agree
/// with `expected`. `source` receives the [`leaf`] body to use, which differs
/// between the runtimes only in the length of its straight-line prelude.
fn assert_scenario(
    source: impl Fn(&str) -> String,
    io: Io,
    expected_output: &[&str],
    expected_status: &str,
) {
    let expected = Report {
        output: expected_output
            .iter()
            .map(|line| line.to_string())
            .collect(),
        status: expected_status.to_string(),
        leftovers: Vec::new(),
    };

    let single = compile(&source(&leaf(SINGLE_THREADED_LEAF_WORK)));
    assert_eq!(run_single(&single), expected, "single-threaded runtime");

    let threaded = compile(&source(&leaf(THREADED_LEAF_WORK)));
    for run in 0..THREADED_RUNS {
        let workers = 1 + run % 4;
        let backend: Option<Arc<dyn IoBackend>> = match io {
            Io::Real => None,
            Io::Stuck => Some(Arc::new(StuckIoBackend::default())),
        };
        assert_eq!(
            run_threaded(&threaded, workers, backend),
            expected,
            "threaded runtime, run {run}, {workers} worker(s)"
        );
    }
}

/// Straight-line work in a single-threaded leaf, in units of roughly 250
/// reductions (a quarter slice). In these scenarios a leaf gets at most about
/// 6 slices before it is cancelled: 20 units fail, 40 pass, 80 leave margin.
const SINGLE_THREADED_LEAF_WORK: usize = 80;

/// On the threaded runtime the leaf's I/O never completes, so the prelude
/// only varies where cancellation finds it: runnable, in flight, or blocked
/// on I/O.
const THREADED_LEAF_WORK: usize = 4;

/// A child body that does not finish before its parent acts on it; see the
/// module docs. It does `work` units of straight-line work, then I/O, so its
/// length does not depend on the scheduler.
fn leaf(work: usize) -> String {
    let sum = vec!["k"; 100].join(" + ");
    let work: String = (0..work).map(|_| format!("val _ = {sum}\n")).collect();
    format!("(let val k = 1\n{work}in BuiltinTime.sleep 0 end)")
}

/// Wrap `body` in `depth` nested children, each awaiting the next with `wrap`.
fn nest(depth: usize, body: &str, wrap: impl Fn(usize, &str) -> String) -> String {
    (1..=depth).fold(body.to_string(), |inner, level| wrap(level, &inner))
}

#[test]
fn nested_raw_await_chain_propagates_deepest_panic() {
    // root -> c1 -> c2 -> c3 -> c4 -> c5; c5 panics. A raw await of a failed
    // child fails the awaiting process, so every level fails in turn.
    let chain = nest(5, "BuiltinTesting.panic \"deep boom\"", |_, inner| {
        format!("BuiltinFiber.await (BuiltinFiber.spawn_raw (fn () => {inner}))")
    });
    let source = format!(
        "import __Builtin.Fiber\nimport __Builtin.Testing\nimport __Builtin.Stdio\n\
         val _ = {chain}\nval _ = BuiltinStdio.println \"unreachable\"\n"
    );
    let status = nest(5, "RuntimeError(\"deep boom\")", |_, inner| {
        format!("ChildProcessFailed({inner})")
    });
    assert_scenario(
        |_| source.clone(),
        Io::Real,
        &[],
        &format!("Failed({status})"),
    );
}

#[test]
fn process_result_types_preserve_polymorphic_join_wrappers() {
    let source = r#"
        import __Builtin.Fiber
        import __Builtin.Testing
        import __Builtin.Stdio
        import __Builtin.Int
        fun join (p : 'a pid) = case BuiltinFiber.await_result p of
            BuiltinFiber.Ok v => v
          | BuiltinFiber.Err _ => BuiltinTesting.panic "unexpected failure"
        fun sum xs = case xs of [] => 0 | x :: rest => x + sum rest
        val _ = BuiltinStdio.println (BuiltinInt.to_string (join (BuiltinFiber.spawn_raw (fn () => 42))))
        val _ = BuiltinStdio.println (join (BuiltinFiber.spawn_raw (fn () => "text")))
        val _ = BuiltinStdio.println (BuiltinInt.to_string (sum (join (BuiltinFiber.spawn_raw (fn () => [1, 2, 3])))))
        val f = join (BuiltinFiber.spawn_raw (fn () => fn (x : int) => x + 1))
        val _ = BuiltinStdio.println (BuiltinInt.to_string (f 6))
    "#;
    assert_scenario(
        |_| source.into(),
        Io::Real,
        &["42\n", "text\n", "6\n", "7\n"],
        "Done",
    );
}

#[test]
fn process_result_types_do_not_allow_joining_a_dead_parents_child() {
    let source = r#"
        import __Builtin.Fiber
        import __Builtin.Testing
        import __Builtin.Stdio
        val q = BuiltinFiber.await (BuiltinFiber.spawn_raw (fn () => BuiltinFiber.spawn_raw (fn () => 41)))
        val _ = case BuiltinFiber.await_result q of
            BuiltinFiber.Err BuiltinFiber.AlreadyJoined => BuiltinStdio.println "reaped"
          | _ => BuiltinTesting.panic "a grandchild must not be joined"
        val _ = BuiltinFiber.await q
    "#;
    let program = compile(source);
    let assert_rejected = |report: Report| {
        // Threaded parent cleanup may still be cancelling/reaping q when
        // its parent result arrives. Both states reject the foreign join.
        if report.output.is_empty() {
            assert!(report.status.contains("not a child"), "{report:?}");
        } else {
            assert_eq!(report.output, ["reaped\n"]);
            assert!(
                report.status.contains("child result already consumed"),
                "{report:?}"
            );
        }
        assert!(report.status.starts_with("Failed("), "{report:?}");
        assert!(report.leftovers.is_empty(), "{report:?}");
    };
    assert_rejected(run_single(&program));
    for run in 0..THREADED_RUNS {
        assert_rejected(run_threaded(&program, 1 + run % 4, None));
    }
}

#[test]
fn process_result_types_do_not_allow_joining_a_live_parents_child() {
    // The sibling is live until it tries to await the root's child. Its
    // failure is observed by the actual parent, which can still join q.
    let source = r#"
        import __Builtin.Fiber
        import __Builtin.String
        import __Builtin.Testing
        import __Builtin.Stdio
        import __Builtin.Int
        val q = BuiltinFiber.spawn_raw (fn () => 41)
        val sibling = BuiltinFiber.spawn_raw (fn () => BuiltinFiber.await q)
        val _ = case BuiltinFiber.await_result sibling of
            BuiltinFiber.Err (BuiltinFiber.RuntimeError m) =>
                if BuiltinString.contains (m, "not a child") then BuiltinStdio.println "foreign"
                else BuiltinTesting.panic m
          | _ => BuiltinTesting.panic "a sibling must not join another parent's child"
        val _ = BuiltinStdio.println (BuiltinInt.to_string (BuiltinFiber.await q))
    "#;
    assert_scenario(|_| source.into(), Io::Real, &["foreign\n", "41\n"], "Done");
}

#[test]
fn nested_result_await_chain_returns_err_at_every_level() {
    // root -> c4 -> c3 -> c2 -> c1 -> c0; c0 panics. Every level sees `Err`
    // and fails with its own message, which its parent sees as `Err`.
    let chain = nest(4, "BuiltinTesting.panic \"deep boom\"", |level, inner| {
        format!(
            "(case BuiltinFiber.await_result (BuiltinFiber.spawn_raw (fn () => {inner})) of\n\
               BuiltinFiber.Ok v => v\n\
             | BuiltinFiber.Err (BuiltinFiber.RuntimeError msg) =>\n\
                 BuiltinTesting.panic (\"level {level} <- \" ^ msg)\n\
             | BuiltinFiber.Err _ => BuiltinTesting.panic \"unexpected error\")"
        )
    });
    let source = format!(
        "import __Builtin.Fiber\n\
         import __Builtin.Testing\n\
         import __Builtin.Stdio\n\
         val _ =\n\
           case BuiltinFiber.await_result (BuiltinFiber.spawn_raw (fn () => {chain})) of\n\
               BuiltinFiber.Ok _ => BuiltinStdio.println \"unexpected ok\"\n\
             | BuiltinFiber.Err (BuiltinFiber.RuntimeError msg) => BuiltinStdio.println msg\n\
             | BuiltinFiber.Err _ => BuiltinStdio.println \"unexpected error\"\n"
    );
    assert_scenario(
        |_| source.clone(),
        Io::Real,
        &["level 4 <- level 3 <- level 2 <- level 1 <- deep boom\n"],
        "Done",
    );
}

#[test]
fn parent_cancelled_while_awaiting_child_doing_io() {
    // p awaits c, which loops doing I/O forever. The root first waits for an
    // unrelated child d so that, under FIFO scheduling, p is already blocked
    // in its await when it is cancelled. Cancelling p must cancel c too;
    // otherwise c runs forever and the run never ends.
    let source = |leaf: &str| {
        format!(
            "import __Builtin.Fiber\n\
         import __Builtin.Stdio\n\
         import __Builtin.Time\n\
         val p = BuiltinFiber.spawn_raw (fn () => BuiltinFiber.await (BuiltinFiber.spawn_raw (fn () => {leaf})))\n\
         val d = BuiltinFiber.spawn_raw (fn () => 0)\n\
         val _ = BuiltinFiber.await d\n\
         val _ = BuiltinFiber.cancel_raw p\n\
         val _ =\n\
           case BuiltinFiber.await_result p of\n\
               BuiltinFiber.Ok _ => BuiltinStdio.println \"ok\"\n\
             | BuiltinFiber.Err BuiltinFiber.Cancelled => BuiltinStdio.println \"cancelled\"\n\
             | BuiltinFiber.Err _ => BuiltinStdio.println \"other\"\n"
        )
    };
    assert_scenario(source, Io::Stuck, &["cancelled\n"], "Done");
}

#[test]
fn wait_any_over_failed_cancelled_and_successful_children() {
    let source = |leaf: &str| {
        format!(
            "import __Builtin.Fiber\n\
         import __Builtin.Testing\n\
         import __Builtin.Stdio\n\
         import __Builtin.Int\n\
         import __Builtin.Time\n\
         fun show r =\n\
           case r of\n\
               BuiltinFiber.Ok v => \"ok \" ^ BuiltinInt.to_string v\n\
             | BuiltinFiber.Err (BuiltinFiber.RuntimeError m) => \"err \" ^ m\n\
             | BuiltinFiber.Err BuiltinFiber.Cancelled => \"cancelled\"\n\
             | BuiltinFiber.Err BuiltinFiber.AlreadyJoined => \"already joined\"\n\
             | BuiltinFiber.Err _ => \"other\"\n\
         val f = BuiltinFiber.spawn_raw (fn () => (BuiltinTesting.panic \"wait boom\" : int))\n\
         val c = BuiltinFiber.spawn_raw (fn () => let val _ = {leaf} in 0 end)\n\
         val s = BuiltinFiber.spawn_raw (fn () => 42)\n\
         val _ = BuiltinFiber.cancel_raw c\n\
         val w = BuiltinFiber.wait_any_raw [f, c, s]\n\
         val _ = BuiltinStdio.println (if w = f orelse w = c orelse w = s then \"winner listed\" else \"winner unknown\")\n\
         val _ = BuiltinStdio.println (show (BuiltinFiber.await_result f))\n\
         val _ = BuiltinStdio.println (show (BuiltinFiber.await_result c))\n\
         val _ = BuiltinStdio.println (show (BuiltinFiber.await_result s))\n\
         val w2 = BuiltinFiber.wait_any_raw [c, s, f]\n\
         val _ = BuiltinStdio.println (if w2 = c then \"joined children count as complete\" else \"bad\")\n\
         val _ = BuiltinStdio.println (show (BuiltinFiber.await_result s))\n"
        )
    };
    assert_scenario(
        source,
        Io::Stuck,
        &[
            "winner listed\n",
            "err wait boom\n",
            "cancelled\n",
            "ok 42\n",
            "joined children count as complete\n",
            "already joined\n",
        ],
        "Done",
    );
}

#[test]
fn wait_any_blocks_until_a_child_fails_or_is_cancelled() {
    // No listed child can finish on its own, so the outcome of each wait_any
    // is decided by a terminal transition: a cancellation in the root, a
    // failure in `nested`.
    let source = |leaf: &str| {
        format!(
            "import __Builtin.Fiber\n\
         import __Builtin.Testing\n\
         import __Builtin.Stdio\n\
         import __Builtin.Time\n\
         val slow = BuiltinFiber.spawn_raw (fn () => {leaf})\n\
         val cancelled = BuiltinFiber.spawn_raw (fn () => {leaf})\n\
         val other = BuiltinFiber.spawn_raw (fn () => {leaf})\n\
         val nested = BuiltinFiber.spawn_raw (fn () =>\n\
           let\n\
             val a = BuiltinFiber.spawn_raw (fn () => {leaf})\n\
             val b = BuiltinFiber.spawn_raw (fn () => BuiltinTesting.panic \"inner\")\n\
             val w = BuiltinFiber.wait_any_raw [a, b]\n\
           in\n\
             if w = b then 1 else 0\n\
           end)\n\
         val _ = BuiltinFiber.cancel_raw cancelled\n\
         val w = BuiltinFiber.wait_any_raw [slow, cancelled, other]\n\
         val _ = BuiltinStdio.println (if w = cancelled then \"cancelled child won\" else \"bad\")\n\
         val _ = BuiltinFiber.cancel_raw slow\n\
         val _ = BuiltinFiber.cancel_raw other\n\
         val _ = BuiltinStdio.println (if BuiltinFiber.await nested = 1 then \"failed child won\" else \"bad\")\n"
        )
    };
    assert_scenario(
        source,
        Io::Stuck,
        &["cancelled child won\n", "failed child won\n"],
        "Done",
    );
}

#[test]
fn parent_exit_cancels_unawaited_children() {
    // p spawns two children that never finish and returns without awaiting
    // them; the root also leaves a child behind. Scope exit must cancel all
    // of them, or the run never ends.
    let source = |leaf: &str| {
        format!(
            "import __Builtin.Fiber\n\
         import __Builtin.Stdio\n\
         import __Builtin.Int\n\
         import __Builtin.Time\n\
         val p = BuiltinFiber.spawn_raw (fn () =>\n\
           let\n\
             val _ = BuiltinFiber.spawn_raw (fn () => {leaf})\n\
             val _ = BuiltinFiber.spawn_raw (fn () => {leaf})\n\
           in\n\
             7\n\
           end)\n\
         val _ = BuiltinFiber.spawn_raw (fn () => {leaf})\n\
         val _ = BuiltinStdio.println (BuiltinInt.to_string (BuiltinFiber.await p))\n"
        )
    };
    assert_scenario(source, Io::Stuck, &["7\n"], "Done");
}

#[test]
fn child_failing_during_io_is_observed_by_awaiters() {
    // On the threaded runtime the failure arrives as an I/O completion; on
    // the single-threaded runtime the builtin fails inline.
    let source = "\
        import __Builtin.Fiber\n\
        import __Builtin.String\n\
        import __Builtin.Filesystem\n\
        import __Builtin.Stdio\n\
        fun show r =\n\
          case r of\n\
              BuiltinFiber.Ok _ => \"ok\"\n\
            | BuiltinFiber.Err (BuiltinFiber.RuntimeError m) =>\n\
                if BuiltinString.contains (m, \"read_file\") then \"io error\" else \"other error: \" ^ m\n\
            | BuiltinFiber.Err _ => \"other\"\n\
        val direct = BuiltinFiber.spawn_raw (fn () => BuiltinFilesystem.read_text \"/definitely/not/here.txt\")\n\
        val _ = BuiltinStdio.println (show (BuiltinFiber.await_result direct))\n\
        val nested = BuiltinFiber.spawn_raw (fn () => BuiltinFiber.await (BuiltinFiber.spawn_raw (fn () => BuiltinFilesystem.read_text \"/definitely/not/here.txt\")))\n\
        val _ = BuiltinStdio.println (show (BuiltinFiber.await_result nested))\n\
        val _ = BuiltinFiber.await (BuiltinFiber.spawn_raw (fn () => BuiltinFilesystem.read_text \"/definitely/not/here.txt\"))\n";
    let program = compile(source);
    let single = run_single(&program);
    assert_eq!(single.output, vec!["io error\n", "io error\n"]);
    assert!(
        single.status.starts_with("Failed(ChildProcessFailed("),
        "{single:?}"
    );
    assert!(single.leftovers.is_empty(), "{single:?}");
    for run in 0..THREADED_RUNS {
        let threaded = run_threaded(&program, 1 + run % 4, None);
        assert_eq!(threaded.output, single.output, "run {run}");
        assert!(
            threaded.status.starts_with("Failed(ChildProcessFailed("),
            "run {run}: {threaded:?}"
        );
        assert!(threaded.leftovers.is_empty(), "run {run}: {threaded:?}");
    }
}

/// Spawn and await 100,000 children in sequence, alternating raw and
/// Result awaits.
const SPAWN_AWAIT_LOOP: &str = "\
    import __Builtin.Fiber\n\
    import __Builtin.Stdio\n\
    fun loop n =\n\
      if n = 0 then ()\n\
      else\n\
        let\n\
          val c = BuiltinFiber.spawn_raw (fn () => n)\n\
          val _ = if n mod 2 = 0 then BuiltinFiber.await c else let val _ = BuiltinFiber.await_result c in 0 end\n\
        in\n\
          loop (n - 1)\n\
        end\n\
    val _ = loop 100000\n\
    val _ = BuiltinStdio.println \"done\"\n";

/// A VM with only the builtins behind the `__Builtin` modules that
/// `SPAWN_AWAIT_LOOP` imports. Spawning copies the
/// parent's builtin table, so this keeps 100,000 spawns cheap in debug builds.
fn spawn_await_loop_vm() -> crate::vm::VM {
    crate::builder::VMBuilder::new(compile(SPAWN_AWAIT_LOOP))
        .register_builtin_name("print")
        .register_builtin_name("println")
        .register_builtin_name("eprint")
        .register_builtin_name("eprintln")
        .register_builtin_name("read_line")
        .register_builtin_name("read_stdin")
        .register_builtin_name("spawn")
        .register_builtin_name("await_process")
        .register_builtin_name("await_process_result")
        .register_builtin_name("fiber_await_all")
        .register_builtin_name("cancel")
        .register_builtin_name("wait_any")
        .build()
}

fn assert_spawn_await_loop_report(report: &Report, max_tombstones: usize, label: &str) {
    assert_eq!(report.output, vec!["done\n"], "{label}: {report:?}");
    assert_eq!(report.status, "Done", "{label}");
    assert!(report.leftovers.is_empty(), "{label}: {report:?}");
    assert!(
        max_tombstones < 10,
        "{label}: max tombstones {max_tombstones}"
    );
}

#[test]
fn spawn_await_loop_keeps_tombstones_bounded_single_threaded() {
    let mut runtime = Runtime::new();
    let pid = runtime.spawn_root_vm(spawn_await_loop_vm());
    let max_tombstones = runtime.run_tracking_max_tombstones().unwrap();
    let report = runtime.lifecycle_report(pid);
    assert_spawn_await_loop_report(&report, max_tombstones, "single-threaded");
}

#[test]
fn spawn_await_loop_keeps_tombstones_bounded_threaded() {
    for workers in [1, 4] {
        let runtime = ThreadedRuntime::new(workers);
        let pid = runtime.spawn_root_vm(spawn_await_loop_vm());
        runtime.run_to_completion().unwrap();
        let report = runtime.lifecycle_report(pid);
        assert_spawn_await_loop_report(
            &report,
            runtime.max_tombstones(),
            &format!("{workers} worker(s)"),
        );
    }
}

/// Programs in which the root cancels, or abandons, a child sleeping 60s.
const SLEEP_CANCEL_PROGRAMS: &[(&str, &str)] = &[
    (
        "explicit cancel, then join",
        "import __Builtin.Fiber\n\
         import __Builtin.Time\n\
         import __Builtin.Stdio\n\
         val c = BuiltinFiber.spawn_raw (fn () => BuiltinTime.sleep 60000)\n\
         val d = BuiltinFiber.spawn_raw (fn () => 0)\n\
         val _ = BuiltinFiber.await d\n\
         val _ = BuiltinFiber.cancel_raw c\n\
         val _ =\n\
           case BuiltinFiber.await_result c of\n\
               BuiltinFiber.Err BuiltinFiber.Cancelled => BuiltinStdio.println \"cancelled\"\n\
             | _ => BuiltinStdio.println \"other\"\n",
    ),
    (
        "abandoned at scope exit",
        "import __Builtin.Fiber\n\
         import __Builtin.Time\n\
         import __Builtin.Stdio\n\
         val _ = BuiltinFiber.spawn_raw (fn () => BuiltinTime.sleep 60000)\n\
         val d = BuiltinFiber.spawn_raw (fn () => 0)\n\
         val _ = BuiltinFiber.await d\n\
         val _ = BuiltinStdio.println \"cancelled\"\n",
    ),
    (
        "grandchild sleeping under a cancelled child",
        "import __Builtin.Fiber\n\
         import __Builtin.Time\n\
         import __Builtin.Stdio\n\
         val c = BuiltinFiber.spawn_raw (fn () => BuiltinFiber.await (BuiltinFiber.spawn_raw (fn () => BuiltinTime.sleep 60000)))\n\
         val d = BuiltinFiber.spawn_raw (fn () => 0)\n\
         val _ = BuiltinFiber.await d\n\
         val _ = BuiltinFiber.cancel_raw c\n\
         val _ = BuiltinStdio.println \"cancelled\"\n",
    ),
];

#[test]
fn cancelling_a_sleeping_fiber_and_dropping_the_runtime_is_prompt() {
    // Threaded only: the single-threaded runtime sleeps inline, so a sleeping
    // fiber cannot be cancelled there.
    for (name, source) in SLEEP_CANCEL_PROGRAMS {
        let program = compile(source);
        for workers in [1, 4] {
            let start = Instant::now();
            let runtime = ThreadedRuntime::new(workers);
            let pid = runtime.spawn_root(program.clone());
            runtime.run_to_completion().unwrap();
            let report = runtime.lifecycle_report(pid);
            drop(runtime);
            let elapsed = start.elapsed();

            assert_eq!(report.output, vec!["cancelled\n"], "{name}");
            assert_eq!(report.status, "Done", "{name}");
            assert!(report.leftovers.is_empty(), "{name}: {report:?}");
            assert!(
                elapsed < Duration::from_millis(100),
                "{name}, {workers} worker(s): run + drop took {elapsed:?}"
            );
        }
    }
}

/// An I/O backend that never completes anything and records cancellations.
/// Shared with other runtime test modules that need a child to stay blocked
/// until cancellation: a mock sleep is not a scheduling barrier.
#[derive(Default)]
pub(crate) struct StuckIoBackend {
    pub(crate) registered: Mutex<Vec<IoToken>>,
    pub(crate) cancelled: Mutex<Vec<IoToken>>,
}

impl StuckIoBackend {
    /// Every request the backend accepted was cancelled. (A request can be
    /// cancelled without ever being registered, if the cancel arrives while
    /// the child is parking.)
    pub(crate) fn assert_all_registered_cancelled(&self) {
        let registered = self.registered.lock().unwrap().clone();
        let cancelled = self.cancelled.lock().unwrap().clone();
        assert!(
            registered.iter().all(|token| cancelled.contains(token)),
            "registered {registered:?}, cancelled {cancelled:?}"
        );
    }
}

impl IoBackend for StuckIoBackend {
    fn register(&self, token: IoToken, _request: IoRequest) -> Result<(), String> {
        self.registered.lock().unwrap().push(token);
        Ok(())
    }

    fn poll(&self) -> Vec<(IoToken, IoResult)> {
        Vec::new()
    }

    fn cancel(&self, token: IoToken) {
        self.cancelled.lock().unwrap().push(token);
    }
}

#[test]
fn cancelling_a_process_blocked_on_io_wakes_its_parent_and_cancels_the_io() {
    // The backend never completes, so the run can only end if cancellation
    // terminates the blocked child and wakes its awaiting parent.
    let source = "\
        import __Builtin.Fiber\n\
        import __Builtin.Filesystem\n\
        import __Builtin.Stdio\n\
        val c = BuiltinFiber.spawn_raw (fn () => BuiltinFilesystem.read_text \"/never/completes.txt\")\n\
        val d = BuiltinFiber.spawn_raw (fn () => 0)\n\
        val _ = BuiltinFiber.await d\n\
        val _ = BuiltinFiber.cancel_raw c\n\
        val _ =\n\
          case BuiltinFiber.await_result c of\n\
              BuiltinFiber.Err BuiltinFiber.Cancelled => BuiltinStdio.println \"cancelled\"\n\
            | _ => BuiltinStdio.println \"other\"\n";
    let program = compile(source);
    for run in 0..THREADED_RUNS {
        let backend = Arc::new(StuckIoBackend::default());
        let report = run_threaded(&program, 1 + run % 4, Some(backend.clone()));
        assert_eq!(report.output, vec!["cancelled\n"], "run {run}");
        assert_eq!(report.status, "Done", "run {run}");
        assert!(report.leftovers.is_empty(), "run {run}: {report:?}");
        backend.assert_all_registered_cancelled();
    }
}

// ── What a child process can see (#113) ─────────────────────────────

/// Run `source` on both runtimes and check the root printed `expected`
/// (one `println` per line) and finished cleanly.
fn assert_child_output(source: &str, expected: &[&str]) {
    let expected: Vec<String> = expected.iter().map(|line| format!("{line}\n")).collect();
    let program = compile(source);
    let single = run_single(&program);
    assert_eq!(single.output, expected, "single-threaded: {single:?}");
    assert_eq!(single.status, "Done", "single-threaded: {single:?}");
    for run in 0..20 {
        let workers = 1 + run % 4;
        let report = run_threaded(&program, workers, None);
        assert_eq!(report.output, expected, "threaded run {run}: {report:?}");
        assert_eq!(report.status, "Done", "threaded run {run}: {report:?}");
        assert!(
            report.leftovers.is_empty(),
            "threaded run {run}: {report:?}"
        );
    }
}

#[test]
fn child_calls_top_level_functions_and_constructors() {
    let source = "\
        import __Builtin.Fiber\n\
        import __Builtin.Stdio\n\
        import __Builtin.Int\n\
        datatype shape = Sq of int | Pt\n\
        fun fact n = if n = 0 then 1 else n * fact (n - 1)\n\
        fun even n = if n = 0 then true else odd (n - 1)\n\
        and odd n = if n = 0 then false else even (n - 1)\n\
        fun twice n = fact n + fact n\n\
        fun area s = case s of Sq n => n * n | Pt => 0\n\
        val base = 100\n\
        val names = [\"a\", \"b\", \"c\"]\n\
        fun len xs = case xs of [] => 0 | _ :: rest => 1 + len rest\n\
        val add5 = let val k = 5 in fn x => x + k end\n\
        val show = BuiltinInt.to_string\n\
        structure M = struct fun sq (x : int) = x * x end\n\
        fun show_pid p = show (BuiltinFiber.await p)\n\
        val _ = BuiltinStdio.println (show_pid (BuiltinFiber.spawn_raw (fn () => fact 5)))\n\
        val _ = BuiltinStdio.println (show_pid (BuiltinFiber.spawn_raw (fn () => if even 10 then 1 else 0)))\n\
        val _ = BuiltinStdio.println (show_pid (BuiltinFiber.spawn_raw (fn () => twice 3)))\n\
        val _ = BuiltinStdio.println (show_pid (BuiltinFiber.spawn_raw (fn () => area (Sq 4) + area Pt)))\n\
        val _ = BuiltinStdio.println (show_pid (BuiltinFiber.spawn_raw (fn () => base + len names)))\n\
        val _ = BuiltinStdio.println (show_pid (BuiltinFiber.spawn_raw (fn () => add5 1)))\n\
        val _ = BuiltinStdio.println (show_pid (BuiltinFiber.spawn_raw (fn () => M.sq 9)))\n\
        val _ = BuiltinStdio.println (BuiltinFiber.await (BuiltinFiber.spawn_raw (fn () => show 42)))\n";
    assert_child_output(source, &["120", "1", "12", "16", "103", "6", "81", "42"]);
}

#[test]
fn child_receives_closures_in_captures_and_spawns_grandchildren() {
    let source = "\
        import __Builtin.Fiber\n\
        import __Builtin.Stdio\n\
        import __Builtin.Int\n\
        fun fact n = if n = 0 then 1 else n * fact (n - 1)\n\
        fun run () =\n\
          let val k = 10\n\
              val f = fn x => fact x + k\n\
              val p = BuiltinFiber.spawn_raw (fn () => BuiltinFiber.await (BuiltinFiber.spawn_raw (fn () => f 3)))\n\
          in BuiltinFiber.await p end\n\
        val _ = BuiltinStdio.println (BuiltinInt.to_string (run ()))\n";
    assert_child_output(source, &["16"]);
}

#[test]
fn child_sees_bindings_defined_before_the_spawn() {
    // The first spawn happens before `late` is defined; each spawn sees the
    // globals defined up to that point.
    let source = "\
        import __Builtin.Fiber\n\
        import __Builtin.Stdio\n\
        import __Builtin.Int\n\
        fun early () = 1\n\
        val p = BuiltinFiber.spawn_raw (fn () => early ())\n\
        fun late () = early () + 1\n\
        val q = BuiltinFiber.spawn_raw (fn () => late ())\n\
        val _ = BuiltinStdio.println (BuiltinInt.to_string (BuiltinFiber.await p + BuiltinFiber.await q))\n";
    assert_child_output(source, &["3"]);
}

#[test]
fn child_sees_the_binding_in_scope_where_it_was_spawned() {
    let source = "\
        import __Builtin.Fiber\n\
        import __Builtin.Stdio\n\
        import __Builtin.Int\n\
        val x = 1\n\
        fun f () = x\n\
        val x = \"two\"\n\
        val p = BuiltinFiber.spawn_raw (fn () => f () + 1)\n\
        val _ = BuiltinStdio.println (x ^ \" \" ^ BuiltinInt.to_string (BuiltinFiber.await p))\n";
    assert_child_output(source, &["two 2"]);
}

#[test]
fn child_uses_program_binding_that_rebinds_a_builtin_name() {
    let source = "\
        import __Builtin.Fiber\n\
        import __Builtin.Stdio\n\
        fun int_to_string (n : int) = \"n\"\n\
        val _ = BuiltinStdio.println (BuiltinFiber.await (BuiltinFiber.spawn_raw (fn () => int_to_string 1)))\n";
    assert_child_output(source, &["n"]);
}

#[test]
fn child_reading_an_unsendable_global_fails_with_its_name() {
    let source = "\
        import __Builtin.Fiber\n\
        import __Builtin.Random\n\
        import __Builtin.Bytes\n\
        import __Builtin.Stdio\n\
        val r = BuiltinRandom.seed (BuiltinBytes.of_string \"seed\")\n\
        fun use_rng () = let val _ = r in 1 end\n\
        val _ =\n\
          case BuiltinFiber.await_result (BuiltinFiber.spawn_raw (fn () => use_rng ())) of\n\
              BuiltinFiber.Err (BuiltinFiber.RuntimeError msg) => BuiltinStdio.println msg\n\
            | _ => BuiltinStdio.println \"other\"\n";
    assert_child_output(
        source,
        &[
            "top-level `r` cannot be used in a child process: cannot send Rng state across processes",
        ],
    );
}
