# TLA+ Verification

## Explicit status

Read this page as a coverage/status document, not as a blanket proof that the
runtime is fully modeled.

- `ProcessLifecycle.tla` is the current semantic source of truth for formal
  modeling of process lifecycle behavior.
- `ThreadedSchedulerImpl.tla` is a useful lower-level worker/scheduler model,
  but it is still only a **partial** implementation model of the current
  threaded runtime.
- `ThreadedSchedulerImpl.qnt` tracks the lower-level mailbox removal.
- `ProcessLifecycle.qnt` currently **lags** the refactored TLA+ lifecycle model
  and should not be treated as source of truth for cancellation, `wait_any`, or
  `await_result`.
- If the Rust code, maintained prose docs, and the older Quint lifecycle port
  disagree, prefer:
  1. Rust source
  2. maintained docs in `docs/`
  3. `specs/tla`
  4. stale Quint ports

## Current source of truth

- [`specs/tla/ProcessLifecycle.tla`](../specs/tla/ProcessLifecycle.tla)
  - Semantic model of the user-visible process lifecycle.
  - Covers spawn, `await`, `await_result`, `wait_any`, cooperative cancellation,
    parent-exit scope cleanup, I/O blocking/completion, and deadlock detection.
  - Does **not** model mailbox send/receive. That surface no longer exists in
    the Rust runtime.

- [`specs/tla/ThreadedSchedulerImpl.tla`](../specs/tla/ThreadedSchedulerImpl.tla)
  - Lower-level worker/scheduler model.
  - Covers queue behavior, worker ownership, stale queue entries, join waiters,
    I/O waiters, and shutdown-on-deadlock structure.
  - Mailbox/receive modeling has been removed here as well.
  - This spec still lags the current Rust runtime in several areas:
    `wait_any`, `AwaitKind::Result`, scope cancellation details,
    `child_parents`, `pending_cancels`, tombstones, and the full TOCTOU
    mitigation logic in `threaded.rs`.

- [`specs/tla/NumericWidthSemantics.tla`](../specs/tla/NumericWidthSemantics.tla)
  - Focused semantic model for width-specific numeric modules.
  - Covers bounded Int32/Word32 conversion decisions, add variant invariants,
    wrapping/saturating boundary values, and symbolic Float32 rounding
    invariants.
  - Does **not** execute or prove Rust's `TryFrom`, `checked_*`, `wrapping_*`,
    `saturating_*`, or IEEE-754 operations. Those are validated by Rust unit
    tests against the actual implementation.

## Lifecycle model

`ProcessLifecycle.tla` is the semantic source of truth for:

- parent-child ownership
- single-consumption join behavior
- `await` vs `await_result`
- `wait_any`
- cooperative cancellation
- parent-exit cleanup of direct children
- I/O completion/failure at the process level

The model keeps terminal children in `procs` with explicit join state rather
than mirroring the threaded runtime's tombstone allocation/freeing. That is
intentional: the semantic spec is meant to model what the parent can observe,
not how the runtime stores it.

## Threaded model

`ThreadedSchedulerImpl.tla` remains the implementation-structure model. It is
useful for:

- worker ownership invariants
- stale queue entry handling
- join waiter consistency
- I/O waiter consistency
- deadlock shutdown structure

It is **not** yet a full refinement of the current threaded runtime. Treat it
as a partial implementation model, not as complete coverage of all
multi-worker/cancellation races.

More concretely, it does **not** yet model all of:

- `wait_any`
- `AwaitKind::Result`
- scope cancellation details
- `child_parents`
- `pending_cancels`
- tombstones / consumed-child records
- publish/recheck TOCTOU mitigation paths in `threaded.rs`

## Quint

The Quint ports live in [`specs/quint`](../specs/quint).

- `ThreadedSchedulerImpl.qnt` now matches the mailbox removal on the lower-level
  scheduler model.
- `ProcessLifecycle.qnt` still lags the refactored TLA+ lifecycle model and
  should not be treated as the primary source of truth for the new cancellation
  / `wait_any` / `await_result` semantics until it is refreshed.

That means Quint is currently useful for:

- lower-level scheduler structure checks
- typechecking the checked-in ports

It is currently **not** the best place to answer semantic questions about the
new lifecycle model.

## Configs

- [`specs/tla/ProcessLifecycle.cfg`](../specs/tla/ProcessLifecycle.cfg)
  - Safety checking for the lifecycle model.

- [`specs/tla/ProcessLifecycleLive.cfg`](../specs/tla/ProcessLifecycleLive.cfg)
  - Uses `SPECIFICATION Spec`.
  - Checks:
    - `CancelRequestedEventuallySettles`
    - `IoEventuallyCompletes`

- [`specs/tla/ThreadedSchedulerImpl.cfg`](../specs/tla/ThreadedSchedulerImpl.cfg)
  - Safety checking for the lower-level worker/scheduler model.

- [`specs/tla/NumericWidthSemantics.cfg`](../specs/tla/NumericWidthSemantics.cfg)
  - Safety checking for width-specific numeric semantic invariants.

- [`specs/tla/ThreadedSchedulerImplLive.cfg`](../specs/tla/ThreadedSchedulerImplLive.cfg)
  - Uses `SPECIFICATION LiveSpec`.
  - Still checks the existing lower-level liveness properties, now with domain
    guards so TLC does not index unallocated pids.
  - Practical note: liveness execution depends on the checker/toolchain in use;
    not every local `tla` CLI environment supports the `WF`-based run path the
    same way.

## Recommended reading order

1. Read `ProcessLifecycle.tla` for meaning.
2. Read `ThreadedSchedulerImpl.tla` for current worker/scheduler structure.
3. Read [`crates/hiko-vm/src/runtime.rs`](../crates/hiko-vm/src/runtime.rs) and
   [`crates/hiko-vm/src/threaded.rs`](../crates/hiko-vm/src/threaded.rs) for
   the actual implementation details that still exceed the lower-level model.
