# Verification Status

This document summarizes Hiko's current verification coverage. It is a status
and routing document, not a claim that the VM or runtime are fully verified.

Hiko currently has four complementary verification layers:

1. the Rust bytecode verifier in `hiko-vm`,
2. Rust unit/regression tests around runtime and VM invariants,
3. Loom models that explore thread interleavings of the threaded runtime's
   real code, and
4. formal models under [`specs/`](../../specs).

For a point-in-time grade and reassessment prompt, see
[`verification-status-20260428.md`](verification-status-20260428.md).

The executable Rust implementation remains the source of truth. Formal specs
and docs should make intended behavior explicit and catch classes of bugs early,
but they do not replace tests against the real implementation.

## Definition coverage matrix

[`docs/language/definition.md`](../language/definition.md) is the entry point
for the future language/runtime Definition. The table below records which
Definition-level claims currently have verification evidence. "Modeled" means a
focused formal model exists; "tested" means Rust implementation tests cover the
claim; "partial" means important gaps remain.

| Definition area | Claim or invariant | Verification evidence | Status | Remaining gap |
|---|---|---|---|---|
| Core bytecode execution | Malformed bytecode should be rejected before execution when structural errors are knowable; semantic/value errors should fail as controlled runtime errors. | Bytecode verifier in [`crates/hiko-vm/src/verify.rs`](../../crates/hiko-vm/src/verify.rs) plus verifier and VM dispatch regression tests. | Tested / specified | Value types, indirect-call callable checks, field presence, data-tag ownership, and resource safety are runtime-guarded or out of verifier scope by design. |
| Effects | `perform` and handler clauses refer to declared effect metadata; a continuation is never a value, and every misuse of `k` is a compile error. | Type-inference tests, compiler tests that reject misuse of `k`, VM effect tests, and verifier checks for `Perform` / `InstallHandler` effect tags and provider slots. | Partial | No complete static effect-row/type-and-effect discipline yet; arbitrary malformed bytecode can still violate source-level effect protocol invariants. |
| Process lifecycle | Spawn, await, await-result, cancellation, I/O blocking/completion, parent-exit cleanup, and deadlock transitions follow the intended lifecycle model. | [`ProcessLifecycle.tla`](../../specs/tla/ProcessLifecycle.tla), configs, runtime tests, and runtime docs. | Modeled / partial implementation evidence | TLA+ is a design model, not a refinement proof of Rust; Quint lifecycle port lags. |
| Threaded scheduler | No lost wakeup, no resurrection of a terminated process, tombstones bounded by live parents, no registration or cancel request left behind, single ownership of each process. | [`ThreadedSchedulerImpl.tla`](../../specs/tla/ThreadedSchedulerImpl.tla) (one table operation per step) with two negative configurations; [Loom models](#loom) of the real lifecycle code; runtime and scheduler tests. | Modeled / Loom-checked | The model is checked with 2 processes; 3 processes are out of reach for the current checker. Loom runs each race with a preemption bound of 3, and the worker loop, monitor loop and `run_to_completion` are not under Loom. |
| Cancellation and async I/O | A cancelled process should not be resurrected by a stale I/O completion. | [`CancelIoRace.tla`](../../specs/tla/CancelIoRace.tla), scenario file, runtime cancellation/I/O tests. | Modeled | More direct refinement tests against threaded runtime interleavings would improve confidence. |
| `wait_any` | When multiple requested children are ready, selection is deterministic and follows the caller's leftmost child order. | [`WaitAnyLeftmost.tla`](../../specs/tla/WaitAnyLeftmost.tla), scenario file, negative broken model, runtime tests. | Modeled | Continue aligning runtime edge-case tests with the focused model. |
| Numeric width semantics | Width-specific numeric modules preserve documented conversion, wrapping, saturating, checked, and Float32 boundary behavior. | [`NumericWidthSemantics.tla`](../../specs/tla/NumericWidthSemantics.tla), numeric module tests, [`docs/language/numerics.md`](../language/numerics.md). | Modeled / tested | Float32 model is symbolic; implementation tests remain necessary for concrete host behavior. |
| Capability and builtin boundaries | Host access happens only through registered, policy-gated builtins. | Builtin/domain tests, config parsing tests, capability-specific unit/regression tests, [`docs/builtins/builtin-domains.md`](../builtins/builtin-domains.md). | Tested / partial | Fuel/host-work accounting and some provider/security boundaries remain incomplete; see #63 and #75. |
| Resource limits | Fuel, heap, memory, and I/O limits should fail closed and report controlled errors. | VM/builtin resource tests and runtime error tests. | Partial | Builtin and async I/O host-work accounting is not complete. |

## Bytecode verifier

The bytecode verifier lives in
[`crates/hiko-vm/src/verify.rs`](../../crates/hiko-vm/src/verify.rs). VM
construction calls it before executing compiled programs.

### What it currently checks

The verifier is primarily a structural bytecode verifier. It currently checks:

- opcode bytes decode to valid `Op` variants,
- operands are not truncated,
- constant-pool index bounds for `Const` and `Panic`,
- string-constant requirement for `Panic`,
- global slot bounds for `GetGlobal` and `SetGlobal` (the program's global
  table),
- function-prototype index bounds for `MakeClosure`, `CallDirect`, and
  `TailCallDirect`,
- `MakeClosure` capture count matches the target function prototype,
- `GetUpvalue` references an existing capture in the current function,
- non-local `MakeClosure` captures reference an existing upvalue in the current
  function,
- relative jump targets stay within the chunk,
- jump and handler targets point to instruction boundaries,
- stack depth does not underflow along reachable control-flow paths,
- verifier stack-depth arithmetic does not overflow,
- handler-clause entry stack depth is modeled as the post-install-handler depth
  plus the handler payload values,
- `Perform` and `InstallHandler` effect tags refer to declared effect metadata,
  and
- function chunks start with stack depth equal to function arity while the main
  chunk starts at depth `0`,
- `GetLocal` and `SetLocal` slot operands are within the main chunk's or
  function prototype's declared maximum local slot count, and
- local `MakeClosure` captures reference an existing local slot while non-local
  captures reference an existing upvalue in the current function, and
- `MakeRecClosures` creates at least one closure; each member has a valid
  function-prototype index and the prototype's capture count, and each capture
  is a known source (upvalue, local slot or group member) whose index is within
  the current function's upvalues, its local slots or the group size.

### What it does not currently prove

The verifier intentionally does **not** prove full runtime semantic safety. The
implementation does not prove or document the items below as verifier
guarantees, and several are explicitly runtime concerns:

- Runtime value type correctness.
  - For example, the verifier does not prove that `AddInt` receives two integer
    values.
- Tuple/data field validity.
  - For example, `GetField` reads the field operand, but it does not prove that
    the runtime value has that field.
- Data constructor/tag semantic validity.
  - `MakeData` reads a tag and arity, but it does not prove that the tag belongs
    to a known source-language constructor ABI.
- Indirect call target validity or type safety.
  - `Call` and `TailCall` check stack depth only; they do not prove that the
    callee is callable or that its arity matches.
  - Direct calls are more constrained because `CallDirect` and `TailCallDirect`
    reference known function prototypes.
- Host builtin semantic safety.
  - The verifier does not prove builtin argument types, host capability
    permission, filesystem safety, HTTP policy, exec policy, AWS policy, or
    other provider-specific safety properties.
- Reachability as a security guarantee.
  - Stack effects are propagated from the first instruction over control-flow
    successors. Structurally malformed but unreachable bytecode is still decoded,
    but stack rules are only meaningful for reachable control-flow paths.
- Termination.
  - The verifier does not prove that bytecode halts.
- Resource safety.
  - The verifier does not prove that memory or fuel limits will not be exceeded.
- Full effect/resume protocol semantic correctness for arbitrary bytecode.
  - The verifier checks declared effect tags and stack depth for `Perform` and
    `InstallHandler`, but it does not prove the full source-level effect-handler
    protocol for hand-written malformed bytecode.

Indirect `Call` and `TailCall` callable-ness and arity failures remain runtime
errors.

## Formal specifications

Formal specs live under [`specs/`](../../specs). They are small bounded models for
specific runtime or semantic questions. They should be read as executable design
contracts and regression tools, not as full proofs of the Rust implementation.

### TLA+

Current TLA+ files:

- [`specs/tla/ProcessLifecycle.tla`](../../specs/tla/ProcessLifecycle.tla)
  with [`ProcessLifecycle.cfg`](../../specs/tla/ProcessLifecycle.cfg) and
  [`ProcessLifecycleLive.cfg`](../../specs/tla/ProcessLifecycleLive.cfg)
  - Semantic model of process lifecycle behavior.
  - Covers spawn, `await`, `await_result`, `wait_any`, cooperative
    cancellation, parent-exit scope cleanup, I/O blocking/completion, and
    deadlock detection.
  - This is the current formal source of truth for user-visible process
    lifecycle semantics.
  - `ProcessLifecycleLive.cfg` checks `CancelRequestedEventuallySettles` and
    `IoEventuallyCompletes`.
  - Terminal children stay in `procs` with explicit join state instead of
    mirroring the runtime's tombstones: the model describes what a parent can
    observe, not how the runtime stores it. Mailbox send/receive is not
    modeled; that surface no longer exists.

- [`specs/tla/ThreadedSchedulerImpl.tla`](../../specs/tla/ThreadedSchedulerImpl.tla)
  with [`ThreadedSchedulerImpl.cfg`](../../specs/tla/ThreadedSchedulerImpl.cfg)
  - Implementation model of
    [`threaded.rs`](../../crates/hiko-vm/src/threaded.rs). Each step is one
    operation on the shared process table, so the checker explores the same
    races the code has. Two workers and the I/O monitor are actors.
  - Covers the register/park/recheck wake protocol, `take_waiter`'s in-flight
    and stale cases, `finish_process` (tombstones, parent-gone reaping,
    `live`, scope-exit cancellation and reaping, wakeups), `cancel_pid` and
    `pending_cancels`, the VM cancel flag, raw and result awaits, `wait_any`,
    and I/O registration, completion and late completions. A process's
    children are `Process::children` in the code (`kids` in the model); there
    is no separate `child_parents` map.
  - Invariants: `NoLostWakeup` (once nothing can run, every process has
    terminated), `CleanAtQuiescence`, `NoResurrection`, `BoundedTombstones`,
    `PendingCancelsConsistent`, `RegistrationsConsistent`.
  - No liveness configuration. `NoLostWakeup` is the safety form of "a
    waiter is eventually woken": the model is finite, so a waiter that is
    never woken ends in a state where nothing can run. The liveness form
    needs weak fairness per actor (otherwise the I/O monitor may be starved
    while a process yields forever, which the runtime never does), and `tla`
    cannot evaluate `WF`.
  - With `MaxProcesses = 2`: 41,310 reachable states, 87,366 transitions,
    depth 50, no violations (6 minutes with `tla`).
  - Abstracts bytecode and values; the leftmost-winner rule of `wait_any` is
    `WaitAnyLeftmost.tla`'s job.

- [`specs/tla/CancelIoRace.tla`](../../specs/tla/CancelIoRace.tla) with
  [`CancelIoRace.cfg`](../../specs/tla/CancelIoRace.cfg) and
  [`CancelIoRaceLive.cfg`](../../specs/tla/CancelIoRaceLive.cfg)
  - Focused model for cancellation racing asynchronous I/O completion.
  - Covers cancel-before-completion, completion-before-cancel, failure-before-
    cancel, and stale completion after cancellation.

- [`specs/tla/WaitAnyLeftmost.tla`](../../specs/tla/WaitAnyLeftmost.tla) with
  [`WaitAnyLeftmost.cfg`](../../specs/tla/WaitAnyLeftmost.cfg),
  [`WaitAnyLeftmostLive.cfg`](../../specs/tla/WaitAnyLeftmostLive.cfg), and
  [`WaitAnyLeftmostRightNotifier.scenario`](../../specs/tla/WaitAnyLeftmostRightNotifier.scenario)
  - Focused model for deterministic `wait_any` selection.
  - Covers the rule that delivery chooses the leftmost ready child from the
    caller's requested child list, not whichever notifier happened to run first.

- [`specs/tla/NumericWidthSemantics.tla`](../../specs/tla/NumericWidthSemantics.tla)
  with [`NumericWidthSemantics.cfg`](../../specs/tla/NumericWidthSemantics.cfg)
  - Focused semantic model for width-specific numeric modules.
  - Covers bounded Int32/Word32 conversion decisions, add variant invariants,
    wrapping/saturating boundary values, and symbolic Float32 rounding
    invariants.
  - Does not execute Rust's `checked_*`, `wrapping_*`, `saturating_*`, or
    IEEE 754 operations; Rust unit tests cover those against the real code.

- [`specs/tla/broken/WaitAnyNotifierBroken.tla`](../../specs/tla/broken/WaitAnyNotifierBroken.tla)
  with [`WaitAnyNotifierBroken.cfg`](../../specs/tla/broken/WaitAnyNotifierBroken.cfg)
  - Intentionally broken negative-check model for the `wait_any` notifier
    policy.
  - Useful as a sanity check that the focused `wait_any` property can catch the
    wrong implementation shape.

- [`specs/tla/broken/ThreadedSchedulerNoRecheck.cfg`](../../specs/tla/broken/ThreadedSchedulerNoRecheck.cfg)
  and [`ThreadedSchedulerNoReap.cfg`](../../specs/tla/broken/ThreadedSchedulerNoReap.cfg)
  - Run `ThreadedSchedulerImpl.tla` with the recheck after parking turned off,
    or with scope exit not reaping children's tombstones, and without I/O
    (neither bug involves it). Both must fail, and do: without the recheck,
    after 4,159 states a parent stays parked on a child that is gone
    (`NoLostWakeup`); without reaping, after 1,950 states
    a child's tombstone outlives its terminated parent
    (`BoundedTombstones`).

Scenario files:

- [`specs/tla/CancelIoRaceStaleCompletion.scenario`](../../specs/tla/CancelIoRaceStaleCompletion.scenario)
- [`specs/tla/WaitAnyLeftmostRightNotifier.scenario`](../../specs/tla/WaitAnyLeftmostRightNotifier.scenario)

These are focused traces/examples for specific interleavings.

### Running the checks

- TLA+: `hiko run scripts/check_tla.hml` runs every maintained model and the
  negative checks in about two minutes, with `ThreadedSchedulerImpl` limited
  to 5,000 states. Each check must finish within the policy's
  `exec_timeout_ms` (60 seconds). The full threaded scheduler check takes
  about 6 minutes:

  ```sh
  tla specs/tla/ThreadedSchedulerImpl.tla \
    --config specs/tla/ThreadedSchedulerImpl.cfg -c MaxProcesses=2 \
    --allow-deadlock --max-states 20000000 --max-depth 1000
  ```

- Loom: `RUSTFLAGS="--cfg loom" cargo test -p hiko-vm --release loom` (a few
  seconds).

There is no CI in this repository yet; run these before merging changes to
`threaded.rs`, `scheduler.rs` or `sync.rs`.

## Loom

[`crates/hiko-vm/src/threaded_loom.rs`](../../crates/hiko-vm/src/threaded_loom.rs)
runs the threaded runtime's real lifecycle functions on two or three Loom
threads, each owning the process it acts on, and checks the process table
after every interleaving:

- await vs child completion (raw and result await);
- `wait_any` with two children completing at once;
- cancel vs completion;
- cancel vs I/O completion;
- tombstone removal (the parent is cancelled) vs the awaited child finishing.

The runtime takes its maps, sets, locks and atomics from
[`sync.rs`](../../crates/hiko-vm/src/sync.rs). Normal builds re-export the
`std` and `dashmap` types; `--cfg loom` builds use Loom's primitives, with a
map on one Loom `RwLock` in place of `DashMap`.

The tests catch real bugs. Each of these, reintroduced on purpose, makes a
test fail with the waiter left blocked: removing the recheck after parking in
`block_on_child`, removing it in `handle_wait_any`, and marking a child not
live before its tombstone is written.

Limits: each model runs with a preemption bound of 3; the worker loop, the
monitor loop and `run_to_completion` (which use OS threads) are not under
Loom; the `RwLock` map serializes operations that `DashMap` runs in parallel
on different shards.

### Quint

Quint files live under [`specs/quint`](../../specs/quint):

- [`specs/quint/README.md`](../../specs/quint/README.md)
- [`specs/quint/ProcessLifecycle.qnt`](../../specs/quint/ProcessLifecycle.qnt)
- [`specs/quint/ThreadedSchedulerImpl.qnt`](../../specs/quint/ThreadedSchedulerImpl.qnt)

Current status:

- `ThreadedSchedulerImpl.qnt` is a port of the earlier, slice-atomic
  scheduler model. It does not follow the step-by-step
  [`ThreadedSchedulerImpl.tla`](../../specs/tla/ThreadedSchedulerImpl.tla);
  prefer the TLA+ model.
- `ProcessLifecycle.qnt` is a lagging port of an older lifecycle model. Prefer
  [`specs/tla/ProcessLifecycle.tla`](../../specs/tla/ProcessLifecycle.tla) for
  current cancellation, `wait_any`, and `await_result` semantics.

If Rust source, maintained docs, TLA+, and Quint disagree, prefer them in this
order:

1. Rust source and tests,
2. maintained docs in `docs/`,
3. TLA+ specs in `specs/tla`,
4. Quint ports in `specs/quint`.

Reading order for runtime semantics: `ProcessLifecycle.tla` for meaning, then
`ThreadedSchedulerImpl.tla` for worker/scheduler structure, then
[`runtime.rs`](../../crates/hiko-vm/src/runtime.rs) and
[`threaded.rs`](../../crates/hiko-vm/src/threaded.rs) for what the models do
not cover yet.

## What is covered today

Current coverage is strongest around:

- bytecode structural validity before VM execution,
- stack-depth underflow/overflow for verified bytecode paths,
- process lifecycle semantics,
- cancellation and async I/O interaction,
- deterministic `wait_any` selection,
- the threaded runtime's wake, cancel and tombstone protocol, modeled step by
  step and checked with Loom on the real code, and
- numeric width semantic boundaries.

Recent runtime hardening also added Rust tests and test-only invariants for
missing process entries and stale waiter registrations in the threaded runtime.
Those tests check the real implementation paths and complement the formal specs.

## What remains possible

Useful next verification work includes:

- add fuzz targets for bytecode verification and interpreter dispatch,
- add fuzz targets for lockfile parsing and filesystem capability path handling,
- refresh `ProcessLifecycle.qnt` to match the current TLA+ lifecycle model,
- check `ThreadedSchedulerImpl.tla` with 3 processes (needs a faster checker),
- bring the worker and monitor loops under Loom,
- model more of the threaded runtime as a refinement of the lifecycle spec,
- add model scenarios for cancellation racing child completion and parent-exit
  scope cleanup,
- document and model child resource/fuel budget inheritance policy, and
- add debug-only runtime invariants for GC references or process table
  transitions where they would catch bugs without affecting release behavior.

The goal is incremental coverage: each spec or verifier check should state a
small invariant clearly enough that both the model and the Rust tests can defend
it over time.
