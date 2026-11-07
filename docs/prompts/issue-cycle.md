# Issue Cycle

Take one issue through Plan → Implement → Verify → Present → Advance under
the [delivery lifecycle](../project/delivery.md#default-issue-cycle). Use the
shared [repository instructions](../../AGENTS.md) and
[role responsibilities](delivery-roles.md). This workflow does not select a
provider, model, tool API, or a required number of sessions.

## Start or resume

- Read the current issue body, relevant comments, main, and any existing PR.
  Resume from the recorded checkpoint rather than restarting completed work.
- If given a roadmap, select an implementation issue whose hard dependencies
  have merged. Respect priority and file ownership; trackers are not code tasks.
- Record the current role and claim. An assigned session can coordinate and
  implement sequentially; only the lead role edits the plan.

## Plan

- Check the plan against current source. Tie every acceptance item to a
  concrete check, identify the smallest implementation scope, and run the
  selected plan reviews. Settle routine details from the development standard.
- Raise a revision for material changes as defined in the lifecycle. Keep
  existing decisions and evidence; do not rewrite the plan on every retry.
- Gate 1 must name this issue and revision. Reviewed revisions may be approved
  in a batch, with each approval recorded on its issue. Do not infer approval
  from an agent's comment or an older revision.

## Implement

- After Gate 1, claim the implementation and use one branch/PR against main.
  Reuse an existing PR; do not stack dependent work on an unmerged branch.
- Reproduce a bug with a regression check before fixing it where appropriate.
  Make the smallest coherent change and keep docs/generated artifacts in sync.
- Stay within the approved acceptance and scope. Routine code/test fixes need
  no further approval; material plan changes return to Plan through the lead.

## Verify and retry

- Run [simplify](../../.agents/skills/simplify/SKILL.md) on the complete PR
  change. The coordinating worker applies cleanup; reviewers stay read-only.
- Run focused checks first, then the plan's required workspace and CI checks.
  Add differential, growth, mutation, or concurrency evidence when selected.
  Documentation-only work checks links and instruction consistency.
- Run [code-review](../../.agents/skills/code-review/SKILL.md) after verification,
  plus other selected concerns, against acceptance and the resulting head.
  Record actual skill passes and reviewer mode. Label self-review honestly;
  it cannot replace an independent review required by the plan.
- For an in-scope failure, return to Implement and then Verify without asking
  again. Do not weaken acceptance or hide a failing required check.
- A required check that is pending, blocked, skipped, flaky, or timed out is
  unmet. Record an unresolved blocker and the next action; report every check.
- After a push, refresh affected evidence/reviews at the new head and explain
  why any carried-over evidence remains valid. Recheck approval before pushing.

## Present and advance

- Record one acceptance-results table: acceptance ID, exact head, check,
  result, evidence, and limits. Link selected reviews and CI, list open
  findings, and keep phase/role/claim, branch/PR/head, and next action current.
- Present a ready PR only when every required check and review passes and no
  blocking finding remains. The maintainer reviews and merges it at Gate 2.
- Confirm the actual merge before closing the completed issue and updating its
  roadmap/project state. Refresh main before selecting the next eligible issue.
- Stop at a human checkpoint or an unresolved blocker with the concrete result
  and required next action recorded. Do not approve, merge, release, or publish.
