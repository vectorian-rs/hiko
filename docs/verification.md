# Verification Status

This document summarizes Hiko's current verification coverage. It is a status
and routing document, not a claim that the VM or runtime are fully verified.

Hiko currently has three complementary verification layers:

1. the Rust bytecode verifier in `hiko-vm`,
2. Rust unit/regression tests around runtime and VM invariants, and
3. formal models under [`specs/`](../specs).

The executable Rust implementation remains the source of truth. Formal specs
and docs should make intended behavior explicit and catch classes of bugs early,
but they do not replace tests against the real implementation.

## Bytecode verifier

The bytecode verifier lives in
[`crates/hiko-vm/src/verify.rs`](../crates/hiko-vm/src/verify.rs). VM
construction calls it before executing compiled programs.

### What it currently checks

The verifier is primarily a structural bytecode verifier. It currently checks:

- opcode bytes decode to valid `Op` variants,
- operands are not truncated,
- constant-pool index bounds for `Const`, `GetGlobal`, `SetGlobal`, and
  `Panic`,
- string-constant requirements for global/string-constant operations,
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
  plus the handler payload values, and
- function chunks start with stack depth equal to function arity while the main
  chunk starts at depth `0`.

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
- Local slot bounds.
  - `GetLocal` and `SetLocal` currently read the slot operand but do not validate
    the slot index against a known local count.
  - This may be because the chunk metadata available to the verifier does not
    currently expose enough local-slot information.
- Local capture bounds for `MakeClosure`.
  - `MakeClosure` checks non-local/upvalue captures, but local captures are read
    without validating that the referenced local slot exists.
- Reachability as a security guarantee.
  - Stack effects are propagated from the first instruction over control-flow
    successors. Structurally malformed but unreachable bytecode is still decoded,
    but stack rules are only meaningful for reachable control-flow paths.
- Termination.
  - The verifier does not prove that bytecode halts.
- Resource safety.
  - The verifier does not prove that memory or fuel limits will not be exceeded.
- Effect/resume protocol semantic correctness.
  - Stack depth is checked for `Perform` and `Resume`, but the verifier does not
    prove full effect-handler protocol validity.

Indirect `Call` and `TailCall` callable-ness and arity failures remain runtime
errors.

## Formal specifications

Formal specs live under [`specs/`](../specs). They are small bounded models for
specific runtime or semantic questions. They should be read as executable design
contracts and regression tools, not as full proofs of the Rust implementation.

### TLA+

Current TLA+ files:

- [`specs/tla/ProcessLifecycle.tla`](../specs/tla/ProcessLifecycle.tla)
  with [`ProcessLifecycle.cfg`](../specs/tla/ProcessLifecycle.cfg) and
  [`ProcessLifecycleLive.cfg`](../specs/tla/ProcessLifecycleLive.cfg)
  - Semantic model of process lifecycle behavior.
  - Covers spawn, `await`, `await_result`, `wait_any`, cooperative
    cancellation, parent-exit scope cleanup, I/O blocking/completion, and
    deadlock detection.
  - This is the current formal source of truth for user-visible process
    lifecycle semantics.

- [`specs/tla/ThreadedSchedulerImpl.tla`](../specs/tla/ThreadedSchedulerImpl.tla)
  with [`ThreadedSchedulerImpl.cfg`](../specs/tla/ThreadedSchedulerImpl.cfg)
  and [`ThreadedSchedulerImplLive.cfg`](../specs/tla/ThreadedSchedulerImplLive.cfg)
  - Lower-level worker/scheduler implementation model.
  - Covers queue behavior, worker ownership, stale queue entries, join waiters,
    I/O waiters, and shutdown/deadlock structure.
  - This is still a partial model of the current threaded runtime. Recent Rust
    hardening around stale `wait_any`, join, and I/O waiter registrations should
    eventually be reflected more directly here.

- [`specs/tla/CancelIoRace.tla`](../specs/tla/CancelIoRace.tla) with
  [`CancelIoRace.cfg`](../specs/tla/CancelIoRace.cfg) and
  [`CancelIoRaceLive.cfg`](../specs/tla/CancelIoRaceLive.cfg)
  - Focused model for cancellation racing asynchronous I/O completion.
  - Covers cancel-before-completion, completion-before-cancel, failure-before-
    cancel, and stale completion after cancellation.

- [`specs/tla/WaitAnyLeftmost.tla`](../specs/tla/WaitAnyLeftmost.tla) with
  [`WaitAnyLeftmost.cfg`](../specs/tla/WaitAnyLeftmost.cfg),
  [`WaitAnyLeftmostLive.cfg`](../specs/tla/WaitAnyLeftmostLive.cfg), and
  [`WaitAnyLeftmostRightNotifier.scenario`](../specs/tla/WaitAnyLeftmostRightNotifier.scenario)
  - Focused model for deterministic `wait_any` selection.
  - Covers the rule that delivery chooses the leftmost ready child from the
    caller's requested child list, not whichever notifier happened to run first.

- [`specs/tla/NumericWidthSemantics.tla`](../specs/tla/NumericWidthSemantics.tla)
  with [`NumericWidthSemantics.cfg`](../specs/tla/NumericWidthSemantics.cfg)
  - Focused semantic model for width-specific numeric modules.
  - Covers bounded Int32/Word32 conversion decisions, add variant invariants,
    wrapping/saturating boundary values, and symbolic Float32 rounding
    invariants.

- [`specs/tla/broken/WaitAnyNotifierBroken.tla`](../specs/tla/broken/WaitAnyNotifierBroken.tla)
  with [`WaitAnyNotifierBroken.cfg`](../specs/tla/broken/WaitAnyNotifierBroken.cfg)
  - Intentionally broken negative-check model for the `wait_any` notifier
    policy.
  - Useful as a sanity check that the focused `wait_any` property can catch the
    wrong implementation shape.

Scenario files:

- [`specs/tla/CancelIoRaceStaleCompletion.scenario`](../specs/tla/CancelIoRaceStaleCompletion.scenario)
- [`specs/tla/WaitAnyLeftmostRightNotifier.scenario`](../specs/tla/WaitAnyLeftmostRightNotifier.scenario)

These are focused traces/examples for specific interleavings.

### Quint

Quint files live under [`specs/quint`](../specs/quint):

- [`specs/quint/README.md`](../specs/quint/README.md)
- [`specs/quint/ProcessLifecycle.qnt`](../specs/quint/ProcessLifecycle.qnt)
- [`specs/quint/ThreadedSchedulerImpl.qnt`](../specs/quint/ThreadedSchedulerImpl.qnt)

Current status:

- `ThreadedSchedulerImpl.qnt` tracks the lower-level scheduler model and is
  useful for typechecking and model-checking that structure.
- `ProcessLifecycle.qnt` is a lagging port of an older lifecycle model. Prefer
  [`specs/tla/ProcessLifecycle.tla`](../specs/tla/ProcessLifecycle.tla) for
  current cancellation, `wait_any`, and `await_result` semantics.

If Rust source, maintained docs, TLA+, and Quint disagree, prefer them in this
order:

1. Rust source and tests,
2. maintained docs in `docs/`,
3. TLA+ specs in `specs/tla`,
4. Quint ports in `specs/quint`.

## What is covered today

Current coverage is strongest around:

- bytecode structural validity before VM execution,
- stack-depth underflow/overflow for verified bytecode paths,
- process lifecycle semantics,
- cancellation and async I/O interaction,
- deterministic `wait_any` selection,
- threaded scheduler ownership/waiter consistency at a partial implementation
  level, and
- numeric width semantic boundaries.

Recent runtime hardening also added Rust tests and test-only invariants for
missing process entries and stale waiter registrations in the threaded runtime.
Those tests check the real implementation paths and complement the formal specs.

## What remains possible

Useful next verification work includes:

- add verifier docs/tests for any future local-slot metadata and validate
  `GetLocal`, `SetLocal`, and local closure captures against it,
- add fuzz targets for bytecode verification and interpreter dispatch,
- add fuzz targets for lockfile parsing and filesystem capability path handling,
- refresh `ProcessLifecycle.qnt` to match the current TLA+ lifecycle model,
- extend `ThreadedSchedulerImpl.tla` with current tombstone, `child_parents`,
  `pending_cancels`, `AwaitKind::Result`, and stale waiter cleanup behavior,
- model more of the threaded runtime as a refinement of the lifecycle spec,
- add model scenarios for cancellation racing child completion and parent-exit
  scope cleanup,
- document and model child resource/fuel budget inheritance policy, and
- add debug-only runtime invariants for GC references or process table
  transitions where they would catch bugs without affecting release behavior.

The goal is incremental coverage: each spec or verifier check should state a
small invariant clearly enough that both the model and the Rust tests can defend
it over time.
