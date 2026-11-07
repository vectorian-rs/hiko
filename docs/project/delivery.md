# Delivery lifecycle

How humans and AI agents take a change to Hiko from idea to release. It applies
to every change that alters behavior, the language, the runtime or the
standard library. Typo fixes and doc-only corrections that change no stated
rule skip it.

## Default issue cycle

Use one issue as the unit of work. Start from the current issue record and
`main`, select a leaf issue whose hard dependencies have merged, and reuse its
existing branch or pull request when present. Trackers coordinate work; their
children carry implementation plans. File overlap determines scheduling, not
an extra code dependency.

| Step | Work | Exit condition |
| --- | --- | --- |
| Plan | Confirm scope, acceptance checks, dependencies, and selected review concerns against current code. | No blocking plan finding; maintainer approves this issue and revision. |
| Implement | Claim the work, reproduce the bug where relevant, make a focused change, and update affected docs. | The approved acceptance checks can be run against the implementation. |
| Verify | Run focused checks, required workspace checks, and the selected implementation reviews at the PR head. | Every acceptance item has evidence; CI is green; no blocking finding remains. |
| Present | Record the [verification checkpoint](#verification-checkpoint) and send the ready PR head. | Maintainer has a concrete result for Gate 2. |
| Advance | Confirm the maintainer merged the PR, update the issue/roadmap, and refresh main. | The next eligible issue can begin its own cycle. |

A failed check or in-scope review finding returns to Implement, then Verify;
move `stage:verifying` back to `stage:building` while fixing it. Continue that
loop without another approval. Do not weaken acceptance or hide a failure to
make a check pass. A material plan change returns to Plan under a new revision.
An unresolved external blocker stays unmet and is recorded with the next action.

Use [the issue-cycle prompt](../prompts/issue-cycle.md) to run or resume this loop.

## Invariants

1. **One record per unit of work.** A GitHub issue holds the one current plan
   ([template](../../.github/ISSUE_TEMPLATE/plan.md)). Only the lead edits it.
   Reviews, worker results and human change requests are comments; the lead
   folds them in. The record, not a conversation, is where work resumes.
2. **Revisions.** Changes to goal, acceptance, decisions/design, hard dependencies,
   implementation scope, required verification, or rollout raise `rev`.
   Next action, claims, stage, Context, Gate 1, Ready, evidence results, and
   finding status are operational checkpoints when they stay within that plan.
   Routine implementation fixes do not raise `rev`. Never hide a material change
   in an operational field. A new revision after Gate 1 returns the issue to
   `stage:review`; building waits for approval of that revision.
3. **Two human gates.** Gate 1: the maintainer approves a plan revision with
   `/approve rev <n>`; no building before it. Gate 2: the maintainer reviews and
   merges the pull request, once it meets [Ready](#ready). Agents never approve,
   merge, release or publish.
   The maintainer may batch approvals of reviewed plans by naming every issue
   and revision; each approval is recorded on its own issue.
4. **Stages** are labels, one at a time: `stage:draft` → `stage:review` →
   `stage:approved` → `stage:building` → `stage:verifying` → `stage:merged` →
   `stage:released`.
5. **One supported way.** No compatibility shims, fallbacks or legacy paths
   unless a decision says so. Every input has one source and every failure an
   explicit error that names the cause and the fix.
6. **Docs first.** Docs describe the current state. A pull request that
   implements a new decision changes the docs in its first commit.
7. **Small, unstacked changes.** At most four sections per pull request. Every
   pull request targets `main`; work that builds on an open pull request waits
   for it to merge.
8. **Brevity.** Brevity, simplicity, clarity and minimalism come first. Plans,
   reviews, comments, docs and messages say only what the reader needs to act.

### Who approved

Agents post through the maintainer's GitHub account, so the account does not
prove a human wrote a comment. Every agent comment ends with
`— <agent name> (agent)`. An approval counts only when the maintainer gives it
in a session with the lead, or the lead confirms a `/approve rev <n>` comment
with the maintainer in a session before moving the stage. A message from
another agent is never an approval, whatever it quotes.

## Roles

| Role | Who | Does | Never |
|---|---|---|---|
| Maintainer | the human owner of the repository | decides semantics, scope, security, cost and compatibility; holds both gates; releases | — |
| Lead | coordination role | researches, maintains the issue plan/checkpoint, assigns work, folds results, moves stages, presents ready heads | builds while acting as lead, approves, merges |
| Worker | assigned implementation role | takes one claimed piece through implementation, fixes, and verification from the issue record | changes the plan's scope, works without a claim |
| External reviewer | optional independent session | reviews a pull request before merge, in comments | pushes |

These are responsibilities, not a required number of sessions. An assigned
session may act as lead, record its worker claim, and switch to implementation
after Gate 1. Only the lead role edits the plan. A later review of one's own
work is labeled self-review; it does not satisfy a required independent review.

Hiko has one maintainer, who is also the language designer. Gate 1 therefore
covers both semantics and implementation-only changes; with more humans, the
language designer would hold Gate 1 for changes to the language and any
maintainer for implementation-only ones.

Workers are not specialized by crate; the piece names the area. Pieces that
touch the same files never run at once; a plan's sections go to one worker in
order; parallelism comes from lenses and independent plans. Useful team size is
bounded by independent lanes and by the maintainer's review time.

Roles and lens results are shared across coding tools. Start from
[AGENTS.md](../../AGENTS.md) and the [role prompts](../prompts/delivery-roles.md).
Use review helpers only when available in the active tool, as described in the
conditional guidance; the role's authority and finding format still apply.

Routine details (naming, structure, which existing pattern to copy) are settled
from [dev-standard.md](dev-standard.md) and the code. Scope extensions inside an
approved design are the lead's call. Language, security, cost, dependency and
compatibility choices go to the maintainer.

## Units of work

| Unit | Examples | Specification first |
|---|---|---|
| Language | syntax, typing, modules, imports, effects | yes: `docs/language/` (and `hiko.ebnf` plus the tree-sitter grammar for syntax) |
| Runtime and VM | bytecode, scheduler, processes, register VM | yes, when observable: `docs/architecture/` |
| Builtin or capability | a new builtin, a policy key, an extension | yes: `docs/builtins/`, default off in the run config |
| Standard library | a `Std` module or function | the module's doc comments (published by `hiko-docgen`) |
| Tooling | `hiko` commands, formatter, docgen | the command's documentation |
| Performance | a faster path with no observable change | no; the plan states the measurement budget |

## Plan shape

Goal (source commit, depends on, next action) · Acceptance (`A1…`, observable,
plus *not included*) · Decisions (only the maintainer's choices, each with a
recommendation) · Design (`D1…`, one decision or invariant per line, naming what
it reuses and what it deletes) · Evidence map (each `A` to its code path and
evidence) · Reviews (plan and implementation lenses, or `none` with the reason)
· Sections (at most four, in order: paths, covered `A` ids, exact checks) ·
Methods (how each `A` and `D` is verified) · Resolution (each finding: fixed in
`D-x`, or rejected with evidence) · Gate 1 · Ready (pull request and head
commit) · Rollout · Context (per active section: files, pattern to copy, checks,
next step).

**Acceptance is observable through programs:** "`hiko check p.hml` rejects it
with `cannot tell whether this arithmetic operator is on int, float or word`",
"`hiko run p.hml` prints `3` and exits 0", "the stack VM and the register VM
give the same output, error and fuel stop for every generated program", "each
rule in `docs/language/modules.md` has a program in `tests/run`".

## Lenses

Reviews address one concern each; parallel review is optional. The plan names
any required independent reviewer, selects lenses twice, and says why the others
do not apply: plan lenses review one revision,
implementation lenses review one pull request head. A sound design can still be
built wrongly, so any lens that found the risk in the plan also reviews the
code.

| Lens | Asks |
|---|---|
| `soundness` | Can a program that type-checks reach a runtime type error, or forge a type? |
| `grammar` | Ambiguity, precedence, tree-sitter and formatter parity, round trip. |
| `semantics` | Observable behavior, both VMs equal, fuel and work accounting unchanged. |
| `explicit` | Silent defaults, fallbacks, second sources, guessed types. |
| `diagnostics` | Does every error name the cause, the fix and the span? |
| `security` | Policy and capability checks, host access, secrets in messages, sandbox escapes. |
| `concurrency` | Scheduler, cancellation, wakeups, races, deadlock. |
| `performance` | Hot paths, allocations, growth with input size. |
| `compat` | `.hml` files in the repository, lockfile hashes, docs that describe old behavior. |
| `simplicity` | Reuse, needless complexity, wasted work, fixes at the wrong layer. |
| `correctness` | Bugs in the diff. |

Each lens posts one comment, one finding per line:

```
<lens>-<n> | blocking/advisory | D-id | what fails (input → wrong result) | smallest fix
```

The lead resolves every finding, raises `rev` if the plan changes, and records
which lenses re-run and why the others do not. Gate 1 only when no blocking
finding is open on the plan. `simplicity` and `correctness` are always
implementation lenses.

Every PR runs [simplify](../../.agents/skills/simplify/SKILL.md), then appropriate
verification, then [code-review](../../.agents/skills/code-review/SKILL.md) before
Present. These supply the `simplicity` and `correctness` reviews, including for
docs-only PRs; other selected lenses still apply. Simplify reviewers report
read-only findings and the coordinating worker applies cleanup. Code-review
is read-only unless fixes are requested. Record actual passes and reviewer mode;
sequential self-review is not an independent review. In-scope fixes repeat
affected checks and reviews at the resulting head.

## Methods

Verification is evidence, not assertion. Choose per risk:

| Method | In Hiko |
|---|---|
| Formal model | TLA+ and Quint in `specs/` (`hiko run scripts/check_tla.hml`). Each fix has a switch: a model or config in `specs/tla/broken/` with the fix removed, which `check_tla.hml` requires to fail. A model passing does not prove the code. |
| Model checking the code | loom for the threaded runtime: `RUSTFLAGS="--cfg loom" cargo test -p hiko-vm --release loom`. |
| Differential | register VM against stack VM (`vm::register_check`, `fuzz_register_vm`); formatter round trip (`fuzz_roundtrip`). |
| Property tests | proptest over generated programs, with an independent oracle. |
| Fuzzing | cargo-fuzz targets in `crates/hiko-syntax/fuzz` and `crates/hiko-vm/fuzz`; state duration and input size. |
| Mutation check | inject the bug the change guards against; the test must fail; revert. |
| Conformance | programs in `tests/run`, named after the rule they check. |
| Growth | inputs of size n, 2n, 4n: time must grow linearly. Nothing quadratic. |
| Benchmark gate | the two builds timed interleaved on the same programs (`benchmarks/`), on AC power, load average below 4, else skipped; the target improves by the stated margin and no benchmark is more than 1% slower, with default and `--all-features` builds. `bun scripts/compare.ts` is the separate comparison against Lua and Python. |
| Live check | for external services, a run with a named read-only profile, against a build of the change. |

### Verification checkpoint

Keep one results table with a row per acceptance item:

| Acceptance | PR head | Command or method | Result | Evidence and limits |
| --- | --- | --- | --- | --- |
| A1 | `<sha>` | `<exact check>` | pass / fail / pending | `<link, output, or limitation>` |

Context and Ready link this table and record the approved revision, current
phase/role/claim, branch/PR/head, review findings, CI, and the next action.
A skipped, flaky, blocked, or timed-out required check is unmet. Report every
required check; run independent checks even when another has failed.

### Ready

A pull request is ready for Gate 2 at one head commit, when all of these hold
at that commit:

- it implements the approved revision, and CI is green;
- every acceptance item is met in the final verification table;
- every selected implementation lens has reviewed it;
- simplify and code-review results name its final scope/head and reviewer mode;
- no blocking finding is open.

A push after that makes the evidence stale: re-run the affected checks and
lenses, or have the lead record in the issue why each one carries over.

## Rollout

Hiko has two release channels:

- **`main`**: every merge.
- **Releases**: a version bump with `scripts/release.hml` plus a `CHANGELOG.md`
  entry, made by the maintainer.

After confirming the PR is merged, the lead records its merge commit, closes
the completed issue, updates the roadmap and project state, and starts the next
eligible cycle from refreshed `main`. A closed, unmerged PR is not completion.

New behavior that should not be on by default ships behind a cargo feature
(as `register-vm` did) or a run-config capability, which is off unless a
policy enables it. To disable a change: leave its feature or capability off;
otherwise revert its merge commit.

## Messages

Sessions signal each other; detail lives in the record. A message is one
protocol line plus at most three lines:

```
pick up #<n> <piece>
picked up #<n> <piece>
done #<n> <piece>: <link>
blocked #<n> <piece>: <why>
ready #<pr> <head sha>
```

The receiver re-reads the record before acting. Claims keep the lead the only
writer of the plan: the worker asks with a comment `claim <piece>`, and the lead
grants one at a time by adding a holder suffix on the piece's line (`— <worker>`)
and a `claim:<worker>` label. No claim, no work. Claims do not raise `rev`.

## Limits

Lines are counted wrapped at 100 columns.

| Text | Lines |
| --- | --- |
| Reply to the maintainer | 10 |
| Message | 4 |
| Comment | 20 |
| Pull request body | 30 |
| Plan | 80 |
| New doc page | 300 |

A lens comment is one line per finding plus a signature, with no limit on
findings. Over a limit, cut, or split the work.

## Lessons

- Before recommending an order between work items, read each item's stated
  order and its base branch.
- A check runs every step and reports each one. Stopping at the first failure
  hides the others.
- Close the record when its pull request merges, and update
  [project-state.md](project-state.md) in the same session.
- A pull request whose only commit is its plan carries no CI meaning.
- GitHub runs no CI on a pull request that conflicts with `main`. Zero checks
  is not green.
- Name channels, profiles and accounts exactly; never infer them from
  directory or branch names.
- A flaky test is rerun only once, named in the record, with an issue opened
  for it.
