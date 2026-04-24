# Quint Specs

## Explicit status

These files are useful, but they are **not** all equally current.

- `ThreadedSchedulerImpl.qnt` is the more trustworthy Quint port right now.
- `ProcessLifecycle.qnt` is a lagging port of an older lifecycle model.
- If you need the current formal semantic model, use `specs/tla`, especially
  `ProcessLifecycle.tla`.
- Do not assume "there is a Quint file" means "the feature is fully ported and
  current".

This directory contains Quint ports of the TLA+ models in [`specs/tla`](../tla).

Files:

- `ProcessLifecycle.qnt`
  - High-level lifecycle port.
  - This file currently lags the refactored TLA+ lifecycle model and should not
    be treated as the primary source of truth for cancellation, `wait_any`, or
    `await_result` semantics until it is refreshed.
- `ThreadedSchedulerImpl.qnt`
  - Lower-level threaded scheduler structure: worker ownership, runnable queue,
    waiter bookkeeping, I/O waiter bookkeeping, and shutdown-on-deadlock logic.

Notes:

- These are intentionally close to the existing TLA+ specs rather than a fresh
  redesign.
- The Quint versions use bounded universes and TLC-backed verification via the
  Quint CLI.
- Both Quint files include temporal fairness/liveness formulas.
- When the Quint ports and TLA+ specs diverge, prefer the TLA+ files in
  `specs/tla` and the Rust runtime sources.

Checked commands:

```sh
quint typecheck specs/quint/ProcessLifecycle.qnt
quint typecheck specs/quint/ThreadedSchedulerImpl.qnt

quint verify specs/quint/ProcessLifecycle.qnt \
  --backend=tlc \
  --invariant=safetyInvariant \
  --max-steps=8

quint verify specs/quint/ThreadedSchedulerImpl.qnt \
  --backend=tlc \
  --invariant=safetyInvariant \
  --max-steps=10
```
