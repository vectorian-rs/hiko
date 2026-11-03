---
name: Plan
about: One unit of work, from plan to release (docs/project/delivery.md)
labels: stage:draft
---

<!-- Only the lead edits this issue. Everything else is a comment.
     Every change raises rev, except Next action, claims, Context, Gate 1 and Ready. -->

**rev 1** · stage: draft · unit: language | runtime | builtin | stdlib | tooling | performance

## Goal

<one or two sentences: what changes, and why>

- Source commit: `<sha on main>`
- Depends on: #<n> | none
- Next action: <who does what>

## Acceptance

- **A1** <observable through a program: `hiko check p.hml` rejects it with "…"; `hiko run p.hml` prints "…">
- **A2** …
- Not included: …

## Decisions

<!-- Only choices the maintainer owns. Each with a recommendation. -->

- **Q1** <question> Recommendation: <option>, because …

## Design

- **D1** <one decision or invariant>. Reuses `<path>`; deletes `<path>`.
- **D2** …

## Evidence map

| A | Code path | Evidence |
|---|---|---|
| A1 | `<path>` | `<test or method>` |

## Reviews

- Plan lenses: [ ] soundness · [ ] grammar · [ ] semantics · [ ] explicit · [ ] diagnostics · [ ] security · [ ] concurrency · [ ] performance · [ ] compat
- Implementation lenses: [x] simplicity · [x] correctness · [ ] soundness · [ ] grammar · [ ] semantics · [ ] explicit · [ ] diagnostics · [ ] security · [ ] concurrency · [ ] performance · [ ] compat
- Not run, and why: …

## Sections

1. **<name>**: `<paths>`; covers A1; checks: `<exact commands>`
2. …

<!-- At most four. Docs change in the first section when a decision is new. -->

## Methods

| A/D | Method | Command | Limit |
|---|---|---|---|
| A1 | conformance | `cargo test -p hiko-vm --test stdlib` | |

## Resolution

| Finding | Outcome |
|---|---|
| `<lens>-1` | fixed in D2 / rejected: <evidence> |

## Gate 1

Approved: rev `<n>` by the maintainer, `<date>`, in `<session or comment link>`.

## Ready

PR #`<n>` at head `<sha>`: final verification `<link>`.

## Rollout

`main` on merge | behind cargo feature `<name>` | behind capability `<key>`, off by default. Disable by: …

## Context

- **Section 1** — files: `…`; pattern to copy: `<path>`; checks: `…`; next step: …
