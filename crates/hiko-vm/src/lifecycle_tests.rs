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
/// module docs. A child process sees only builtins and its captures (every
/// named function, even a local `fun` or a constructor, is a VM global), so
/// it cannot loop. It does `work` units of straight-line work instead, then
/// I/O.
fn leaf(work: usize) -> String {
    let sum = vec!["k"; 100].join(" + ");
    let work: String = (0..work).map(|_| format!("val _ = {sum}\n")).collect();
    format!("(let val k = 1\n{work}in sleep 0 end)")
}

/// Wrap `body` in `depth` nested children, each awaiting the next with `wrap`.
fn nest(depth: usize, body: &str, wrap: impl Fn(usize, &str) -> String) -> String {
    (1..=depth).fold(body.to_string(), |inner, level| wrap(level, &inner))
}

#[test]
fn nested_raw_await_chain_propagates_deepest_panic() {
    // root -> c1 -> c2 -> c3 -> c4 -> c5; c5 panics. A raw await of a failed
    // child fails the awaiting process, so every level fails in turn.
    let chain = nest(5, "panic \"deep boom\"", |_, inner| {
        format!("await_process (spawn (fn () => {inner}))")
    });
    let source = format!("val _ = {chain}\nval _ = println \"unreachable\"\n");
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
fn nested_result_await_chain_returns_err_at_every_level() {
    // root -> c4 -> c3 -> c2 -> c1 -> c0; c0 panics. Every level sees `Err`
    // and fails with its own message, which its parent sees as `Err`.
    let chain = nest(4, "panic \"deep boom\"", |level, inner| {
        format!(
            "(case await_process_result (spawn (fn () => {inner})) of\n\
               BuiltinProcess.Ok v => v\n\
             | BuiltinProcess.Err (BuiltinProcess.RuntimeError msg) =>\n\
                 panic (\"level {level} <- \" ^ msg)\n\
             | BuiltinProcess.Err _ => panic \"unexpected error\")"
        )
    });
    let source = format!(
        "import __Builtin.Process\n\
         val _ =\n\
           case await_process_result (spawn (fn () => {chain})) of\n\
               BuiltinProcess.Ok _ => println \"unexpected ok\"\n\
             | BuiltinProcess.Err (BuiltinProcess.RuntimeError msg) => println msg\n\
             | BuiltinProcess.Err _ => println \"unexpected error\"\n"
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
            "import __Builtin.Process\n\
         val p = spawn (fn () => await_process (spawn (fn () => {leaf})))\n\
         val d = spawn (fn () => 0)\n\
         val _ = await_process d\n\
         val _ = cancel p\n\
         val _ =\n\
           case await_process_result p of\n\
               BuiltinProcess.Ok _ => println \"ok\"\n\
             | BuiltinProcess.Err BuiltinProcess.Cancelled => println \"cancelled\"\n\
             | BuiltinProcess.Err _ => println \"other\"\n"
        )
    };
    assert_scenario(source, Io::Stuck, &["cancelled\n"], "Done");
}

#[test]
fn wait_any_over_failed_cancelled_and_successful_children() {
    let source = |leaf: &str| {
        format!(
            "import __Builtin.Process\n\
         fun show r =\n\
           case r of\n\
               BuiltinProcess.Ok v => \"ok \" ^ int_to_string v\n\
             | BuiltinProcess.Err (BuiltinProcess.RuntimeError m) => \"err \" ^ m\n\
             | BuiltinProcess.Err BuiltinProcess.Cancelled => \"cancelled\"\n\
             | BuiltinProcess.Err BuiltinProcess.AlreadyJoined => \"already joined\"\n\
             | BuiltinProcess.Err _ => \"other\"\n\
         val f = spawn (fn () => (panic \"wait boom\" : int))\n\
         val c = spawn (fn () => {leaf})\n\
         val s = spawn (fn () => 42)\n\
         val _ = cancel c\n\
         val w = wait_any [f, c, s]\n\
         val _ = println (if w = f orelse w = c orelse w = s then \"winner listed\" else \"winner unknown\")\n\
         val _ = println (show (await_process_result f))\n\
         val _ = println (show (await_process_result c))\n\
         val _ = println (show (await_process_result s))\n\
         val w2 = wait_any [c, s, f]\n\
         val _ = println (if w2 = c then \"joined children count as complete\" else \"bad\")\n\
         val _ = println (show (await_process_result s))\n"
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
            "val slow = spawn (fn () => {leaf})\n\
         val cancelled = spawn (fn () => {leaf})\n\
         val other = spawn (fn () => {leaf})\n\
         val nested = spawn (fn () =>\n\
           let\n\
             val a = spawn (fn () => {leaf})\n\
             val b = spawn (fn () => panic \"inner\")\n\
             val w = wait_any [a, b]\n\
           in\n\
             if w = b then 1 else 0\n\
           end)\n\
         val _ = cancel cancelled\n\
         val w = wait_any [slow, cancelled, other]\n\
         val _ = println (if w = cancelled then \"cancelled child won\" else \"bad\")\n\
         val _ = cancel slow\n\
         val _ = cancel other\n\
         val _ = println (if await_process nested = 1 then \"failed child won\" else \"bad\")\n"
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
            "val p = spawn (fn () =>\n\
           let\n\
             val _ = spawn (fn () => {leaf})\n\
             val _ = spawn (fn () => {leaf})\n\
           in\n\
             7\n\
           end)\n\
         val _ = spawn (fn () => {leaf})\n\
         val _ = println (int_to_string (await_process p))\n"
        )
    };
    assert_scenario(source, Io::Stuck, &["7\n"], "Done");
}

#[test]
fn child_failing_during_io_is_observed_by_awaiters() {
    // On the threaded runtime the failure arrives as an I/O completion; on
    // the single-threaded runtime the builtin fails inline.
    let source = "\
        import __Builtin.Process\n\
        fun show r =\n\
          case r of\n\
              BuiltinProcess.Ok _ => \"ok\"\n\
            | BuiltinProcess.Err (BuiltinProcess.RuntimeError m) =>\n\
                if string_contains (m, \"read_file\") then \"io error\" else \"other error: \" ^ m\n\
            | BuiltinProcess.Err _ => \"other\"\n\
        val direct = spawn (fn () => read_file \"/definitely/not/here.txt\")\n\
        val _ = println (show (BuiltinProcess.await_result_raw direct))\n\
        val nested = spawn (fn () => await_process (spawn (fn () => read_file \"/definitely/not/here.txt\")))\n\
        val _ = println (show (BuiltinProcess.await_result_raw nested))\n\
        val _ = await_process (spawn (fn () => read_file \"/definitely/not/here.txt\"))\n";
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

const SPAWN_AWAIT_LOOP: &str = "\
    import __Builtin.Process\n\
    fun loop n =\n\
      if n = 0 then ()\n\
      else\n\
        let\n\
          val c = spawn (fn () => n)\n\
          val _ =\n\
            if n mod 2 = 0 then await_process c\n\
            else (case BuiltinProcess.await_result_raw c of BuiltinProcess.Ok v => v | _ => 0)\n\
        in\n\
          loop (n - 1)\n\
        end\n\
    val _ = loop 100000\n\
    val _ = println \"done\"\n";

#[test]
fn spawn_await_loop_keeps_tombstones_bounded_single_threaded() {
    let mut runtime = Runtime::new();
    let pid = runtime.spawn_root(compile(SPAWN_AWAIT_LOOP));
    let max_tombstones = runtime.run_tracking_max_tombstones().unwrap();
    let report = runtime.lifecycle_report(pid);
    assert_eq!(report.output, vec!["done\n"]);
    assert_eq!(report.status, "Done");
    assert!(report.leftovers.is_empty(), "{report:?}");
    assert!(max_tombstones < 10, "max tombstones {max_tombstones}");
}

#[test]
fn spawn_await_loop_keeps_tombstones_bounded_threaded() {
    for workers in [1, 4] {
        let runtime = ThreadedRuntime::new(workers);
        let pid = runtime.spawn_root(compile(SPAWN_AWAIT_LOOP));
        // Sample the tombstone count while the loop runs. Sampling can only
        // under-report the true maximum; the final count is exact.
        let probe = runtime.tombstone_probe();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sampler = {
            let done = Arc::clone(&done);
            std::thread::spawn(move || {
                let mut max = 0;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    max = max.max(probe());
                    std::thread::yield_now();
                }
                max
            })
        };
        runtime.run_to_completion().unwrap();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let max_tombstones = sampler.join().unwrap();

        let report = runtime.lifecycle_report(pid);
        assert_eq!(report.output, vec!["done\n"]);
        assert_eq!(report.status, "Done");
        assert!(report.leftovers.is_empty(), "{report:?}");
        assert!(
            max_tombstones < 10,
            "{workers} worker(s): max tombstones {max_tombstones}"
        );
    }
}

/// Programs in which the root cancels, or abandons, a child sleeping 60s.
const SLEEP_CANCEL_PROGRAMS: &[(&str, &str)] = &[
    (
        "explicit cancel, then join",
        "import __Builtin.Process\n\
         val c = spawn (fn () => sleep 60000)\n\
         val d = spawn (fn () => 0)\n\
         val _ = await_process d\n\
         val _ = cancel c\n\
         val _ =\n\
           case BuiltinProcess.await_result_raw c of\n\
               BuiltinProcess.Err BuiltinProcess.Cancelled => println \"cancelled\"\n\
             | _ => println \"other\"\n",
    ),
    (
        "abandoned at scope exit",
        "val _ = spawn (fn () => sleep 60000)\n\
         val d = spawn (fn () => 0)\n\
         val _ = await_process d\n\
         val _ = println \"cancelled\"\n",
    ),
    (
        "grandchild sleeping under a cancelled child",
        "val c = spawn (fn () => await_process (spawn (fn () => sleep 60000)))\n\
         val d = spawn (fn () => 0)\n\
         val _ = await_process d\n\
         val _ = cancel c\n\
         val _ = println \"cancelled\"\n",
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
#[derive(Default)]
struct StuckIoBackend {
    registered: Mutex<Vec<IoToken>>,
    cancelled: Mutex<Vec<IoToken>>,
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
        import __Builtin.Process\n\
        val c = spawn (fn () => read_file \"/never/completes.txt\")\n\
        val d = spawn (fn () => 0)\n\
        val _ = await_process d\n\
        val _ = cancel c\n\
        val _ =\n\
          case BuiltinProcess.await_result_raw c of\n\
              BuiltinProcess.Err BuiltinProcess.Cancelled => println \"cancelled\"\n\
            | _ => println \"other\"\n";
    let program = compile(source);
    for run in 0..THREADED_RUNS {
        let backend = Arc::new(StuckIoBackend::default());
        let report = run_threaded(&program, 1 + run % 4, Some(backend.clone()));
        assert_eq!(report.output, vec!["cancelled\n"], "run {run}");
        assert_eq!(report.status, "Done", "run {run}");
        assert!(report.leftovers.is_empty(), "run {run}: {report:?}");
        // Every request the backend accepted was cancelled.
        let mut registered = backend.registered.lock().unwrap().clone();
        let mut cancelled = backend.cancelled.lock().unwrap().clone();
        registered.sort_by_key(|token| token.0);
        cancelled.sort_by_key(|token| token.0);
        cancelled.dedup();
        assert_eq!(registered, cancelled, "run {run}");
    }
}
