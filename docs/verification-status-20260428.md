# Verification Status Snapshot — 2026-04-28

This is a point-in-time assessment of Hiko's verification posture. It is a
planning aid, not a formal assurance claim. See
[`verification.md`](verification.md) for the current verifier/spec inventory and
trust-boundary details.

Overall grade at this snapshot:

```text
B-
```

More precisely:

```text
Runtime/process semantics: B / B+
Bytecode structural verification: B-
Security-sensitive host boundaries: B-
Formal verification maturity: C+
Fuzz/property testing: D / not yet mature
Overall: B-
```

## Area grades

| Area | Grade | Notes |
| --- | --- | --- |
| Bytecode structural verifier | B- | Solid baseline; lacks local-slot/capture metadata validation and semantic checks. |
| VM runtime panic resistance | B | Recent hardening converted many panics/silent corruptions into controlled failures. |
| Threaded runtime correctness | C+ / B- | Better after stale waiter cleanup, but still complex and only partially modeled. |
| Formal specs | C+ | Good TLA+ investment, but specs are not yet full implementation refinements. |
| Filesystem/host capability boundary | B | cap-std is a major improvement; still not a full sandbox and needs fuzzing. |
| Remote module loading | B | Verification TOCTOU and HTTPS/body caps are good; more fuzzing would help. |
| Allocation/resource accounting | B | Many high-risk builtins now preflight intermediates; full resource policy docs remain. |
| Fuzz/property testing | D | Important next step. |
| Documentation of guarantees | B | Much better now with `docs/verification.md`, `docs/runtime.md`, `docs/vm.md`, etc. |

## Why the grade is not lower

Hiko has multiple verification layers already:

- a bytecode verifier in
  [`crates/hiko-vm/src/verify.rs`](../crates/hiko-vm/src/verify.rs),
- runtime hardening tests for missing processes, stale waiters, placeholder
  validation, allocation accounting, exec identity revalidation, and filesystem
  capability boundaries,
- formal TLA+ models for process lifecycle, threaded scheduling structure,
  `wait_any`, cancellation/I/O races, and numeric width semantics,
- documentation describing verifier guarantees and non-guarantees, and
- a pattern of converting runtime panics or silent skips into controlled errors.

For a pre-1.0 VM/runtime, that is a strong base.

## Why the grade is not higher

The remaining gaps are the ones that tend to matter most for language runtimes:

1. Formal models are still partial.
   - `ProcessLifecycle.tla` is the strongest semantic model.
   - `ThreadedSchedulerImpl.tla` still lags the current Rust threaded runtime.
   - Tombstones, `child_parents`, `pending_cancels`, `AwaitKind::Result`, stale
     waiter cleanup, and scope-cancellation details need better model coverage.
2. Fuzzing/property testing is not yet mature.
   - There is not yet a committed fuzz suite for verifier, interpreter,
     lockfile, filesystem capability paths, or module-loader inputs.
3. The bytecode verifier is structural, not semantic.
   - It checks opcodes, operands, jump targets, stack depth, constants, and some
     function/upvalue bounds.
   - It does not prove value types, local slot bounds, local closure capture
     bounds, indirect call arity, data tag validity, effect protocol correctness,
     or resource safety.
4. The threaded runtime remains difficult to verify.
   - Recent stale waiter cleanup improved it, but concurrency protocols still
     deserve stronger model alignment, Loom/Shuttle-style tests, or more
     transition invariants.
5. Security-sensitive host boundaries are improving but not fully systematic.
   - Recent work improved filesystem capabilities, remote module loading, exec
     identity revalidation, and AWS auth-source policy.
   - Diagnostics/span sanitization, provider secrecy tests, path fuzzing, and
     child resource/fuel policy documentation remain useful follow-ups.

## Short path to B+

The shortest path to a B+ assessment is:

1. Add fuzz target scaffolding for:
   - bytecode verifier,
   - interpreter dispatch,
   - lockfile parsing, and
   - filesystem path/capability policy.
2. Add local-slot metadata to bytecode chunks, or explicitly document why the
   verifier cannot validate local slots yet.
3. Refresh `ThreadedSchedulerImpl.tla` to include:
   - tombstones,
   - `child_parents`,
   - `pending_cancels`,
   - stale waiter cleanup, and
   - `AwaitKind::Result`.
4. Add threaded runtime transition invariant tests.
5. Document child fuel/resource inheritance policy.

## Path to A-

An A- posture would likely require:

- a fuzz suite running in CI,
- reproducible model-checking commands documented for maintained specs,
- a threaded runtime model closely aligned with Rust,
- Loom/Shuttle-style concurrency tests for cancellation/completion/waiter races,
- a stronger verifier with local slot/capture bounds and more ABI checks,
- systematic security policy docs for diagnostics, providers, filesystem, exec,
  and remote loading, and
- more negative tests for host capability policies.

## Suggested re-assessment prompt

Use this prompt to re-run the assessment after significant verification or
runtime/security work:

```text
Review the current Hiko repository verification posture. Read docs/index.md,
docs/verification.md, the latest docs/verification-status-*.md snapshot,
crates/hiko-vm/src/verify.rs, crates/hiko-vm/src/runtime.rs,
crates/hiko-vm/src/threaded.rs, and the specs/ tree. Compare the current state
against the previous snapshot. Grade the project by area using the same table
categories: bytecode structural verifier, VM runtime panic resistance, threaded
runtime correctness, formal specs, filesystem/host capability boundary, remote
module loading, allocation/resource accounting, fuzz/property testing, and
documentation of guarantees. Explain what improved, what regressed, what remains
unverified, and the shortest path to the next grade.
```

Keep this prompt in the status snapshot so the question can be repeated with the
same frame of reference. If the categories change materially, update
[`verification.md`](verification.md) first and then create a new dated snapshot.
