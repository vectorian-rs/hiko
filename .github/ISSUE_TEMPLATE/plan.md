---
name: Plan
about: One issue through plan, implement, verify, present, and advance
labels: stage:draft
---

<!-- Follow docs/project/delivery.md. Only the lead edits the plan.
     Material plan changes raise rev; progress/evidence and in-scope fixes do not.
     A changed revision needs approval; a changed head needs current evidence. -->

**rev 1** · stage: draft · unit: language | runtime | builtin | stdlib | tooling | performance

## Goal

<what changes and why>

- Source commit: `<sha on main>`; roadmap: #<n> | none
- Depends on: <hard dependencies that must merge; file overlap is scheduling>
- Next action: <phase, owner, next step>

## Acceptance

- **A1** <input, observable output/error, and exact check>
- **A2** …
- Not included: …

## Decisions

- **Q1** <maintainer choice; recommendation and reason, or none open>

## Design

- **D1** <invariant or decision; paths reused and removed>

## Evidence map

- A1 → `<code path>` → `<test or method>`

## Reviews

- Required plan lenses: <concerns, or none with reason>
- Required implementation lenses: simplicity (simplify skill), correctness (code-review skill), <other concerns>
- Omitted lenses and why: …

## Sections

1. **<piece>**: `<paths>`; covers A1; checks: `<exact commands>`
<!-- At most four sections; new decisions change docs in the first commit. -->

## Methods

- A1/D1: <method, exact command, input size/duration, and limits>
- Required final checks: <workspace/CI commands; docs-only validation if applicable>

## Resolution

- `<finding>` → <fix/evidence at head, rejection with evidence, or still open>

## Gate 1

Approved: not yet.
<!-- Record the exact issue/rev, maintainer, date, and confirmation after approval. -->

## Ready

Not ready. PR #<n> at `<head sha>`; acceptance table: <link>; skill passes/mode, reviews/CI: <links>.
<!-- One row per A item: head, check, result, evidence/limits. Missing checks are unmet. -->

## Rollout

<main or feature/capability; breaking changes and how to disable>

## Context

- Phase/role/claim: <current phase and owner>; sequence with: <shared-file work>
- Branch/PR/head: …; latest results/open findings: …; next action: …
<!-- Resume here. In-scope failures return to Implement then Verify, without new approval.
     After confirmed maintainer merge: record merge SHA, close/update records, refresh main. -->
