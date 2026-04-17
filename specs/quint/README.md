# Quint Specs

This directory contains Quint ports of the TLA+ models in [`specs/tla`](../tla).

Files:

- `ProcessLifecycle.qnt`
  - High-level process lifecycle semantics: spawn, await, completion/failure,
    I/O blocking and resolution, mailbox send/receive, and deadlock detection.
- `ThreadedSchedulerImpl.qnt`
  - Lower-level threaded scheduler structure: worker ownership, runnable queue,
    waiter bookkeeping, I/O waiter bookkeeping, and shutdown-on-deadlock logic.

Notes:

- These are intentionally close to the existing TLA+ specs rather than a fresh
  redesign.
- `ProcessLifecycle.qnt` still includes mailbox send/receive because the source
  TLA+ model includes it. That is useful for fidelity to the current TLA+ model,
  even though Hiko's current runtime surface has already moved away from public
  mailbox messaging.
- The Quint versions use bounded universes and TLC-backed verification via the
  Quint CLI.
- This first pass ports the transition relations and safety invariants. The TLA+
  fairness/liveness formulas have not been re-expressed in Quint yet.

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
