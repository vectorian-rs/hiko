# Verification Documentation

Bytecode verifier status, formal model inventory, dated assessment snapshots, and verification reading order.

| File | Status | Summary |
|---|---|---|
| [`verification.md`](verification.md) | Current source of truth | Current bytecode verifier guarantees/non-guarantees, formal spec inventory, TLA+/Quint coverage, remaining risks, and roadmap. |
| [`verification-tla.md`](verification-tla.md) | Detailed/older focus | Focused TLA+/Quint status and reading order. Useful for model-checking detail; prefer [`verification.md`](verification.md) for the overview. |
| [`verification-status-20260428.md`](verification-status-20260428.md) | Dated snapshot | Verification grade snapshot from 2026-04-28, rationale, short path to B+/A-, and reassessment prompt. |

## Recommended use

- Start with [`verification.md`](verification.md) for any verifier, fuzzing, scheduler-correctness, deadlock, wakeup, cancellation, or model-checking work.
- Use [`verification-tla.md`](verification-tla.md) when you need exact formal model/config paths.
- Treat dated snapshots as historical assessments; create a new snapshot instead of editing old dates.
