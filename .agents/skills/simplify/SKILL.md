---
name: simplify
description: Review changed code for reuse, simplification, efficiency, and abstraction problems, then apply behavior-preserving fixes. Use after implementation and before presenting a PR.
---

# Simplify

Port of Claude Code's `/simplify` cleanup workflow. Review the change, consolidate
findings, and apply useful fixes. Correctness review belongs to `code-review`.

## Establish the scope

Read the applicable repository instructions and the requested target. Accept a
PR, branch, revision range, path, or optional focus such as memory efficiency.
For a PR, resolve its base/head and inspect its complete diff. For the current
branch, include its commits relative to the PR base or default branch, staged
and unstaged changes, and relevant untracked files; a clean working tree does
not mean an empty review. State the actual comparison and any narrowed scope.

Read changed files and surrounding callers. Search for existing implementations
before proposing a new helper. Preserve unrelated work and the approved scope.
Do not edit another PR's checkout: a remote target is report-only until the
matching branch or an isolated worktree is available.

## Four review passes

Launch four read-only reviewers in separate contexts when delegation is
available and permitted. Give each the same complete diff, scope, repository
instructions, and user focus, but only its assigned concern. Run independent
passes concurrently within the host's concurrency limit, using additional waves
when needed. Reviewers report findings; only the coordinating worker edits.
If delegation is unavailable or prohibited, perform all four passes sequentially
and label the result a self-review. Never describe that as independent review.

1. **Reuse.** Find logic that duplicates an existing helper, utility, or data
   source. Look beyond the changed files. Prefer the established implementation
   when its contract fits; do not force unrelated concepts into one helper.
2. **Simplification.** Find redundant state, values derivable from existing
   state, excessive parameters, copied blocks, needless nesting, string-based
   substitutes for existing types, and abstractions that obscure a simple task.
   Preserve useful boundaries and choose clarity over the fewest lines.
3. **Efficiency.** Find repeated computations, avoidable copies or allocations,
   repeated I/O, opportunities for safe concurrency, expensive work on common
   paths, broad reads when a narrow lookup exists, and unbounded retention.
   Validate the relevant execution path before claiming an improvement. Avoid
   speculative tuning and changing ordering, resource limits, or work charges.
4. **Abstraction level.** Check whether the change fixes the cause at its owning
   layer, whether a helper has real reuse, and whether a local fix has grown a
   framework, compatibility path, duplicated policy, or extra source of truth.
   Recommend the smallest coherent design within the requested behavior.

Every finding names its location, concrete duplication or unnecessary work,
and the smallest useful change. A pass may return no findings.

## Consolidate, fix, verify

Deduplicate overlapping findings and inspect each against the actual code.
Discard false positives and recommendations that add complexity or change the
contract. A proposed improvement is not automatically a defect.

Apply the remaining behavior-preserving fixes sequentially. Preserve outputs,
errors, source spans, evaluation order, public interfaces, runtime encodings,
and observable work accounting. Do not expand scope, weaken tests, or introduce
dependencies merely to satisfy a reviewer. Record any useful proposal requiring
a behavior change instead of silently applying it.

Run the checks appropriate to the edits. Reinspect affected passes after fixes;
stop when useful findings are resolved or a concrete unresolved limitation is
reported. Do not manufacture edits when the change is already clear.

Report the scope and commit (including whether uncommitted changes were read),
reviewer mode, important fixes, remaining findings with reasons, and checks
actually run. A clean result explicitly says no simplification was needed.
In Hiko's delivery loop this supplies the `simplicity` review; final evidence
must describe the resulting PR head.

## Source and host adaptation

Adapted from the [Claude Code command reference](https://code.claude.com/docs/en/commands),
checked 2026-11-07. The current version uses four reviewers; older versions used
three. Pi and Codex use this same file with their own tools and permissions.
No Claude model names, tool APIs, permission frontmatter, or cloud service are
required. Repository lifecycle requirements still apply.
