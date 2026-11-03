# Delivery lifecycle

How humans and AI agents take a change to Hiko from idea to release. It applies
to every change that alters behavior, the language, the runtime or the
standard library. Typo fixes and doc-only corrections that change no stated
rule skip it.

## Invariants

1. **One record per unit of work.** A GitHub issue holds the one current plan
   ([template](../../.github/ISSUE_TEMPLATE/plan.md)). Only the lead edits it.
   Reviews, worker results and human change requests are comments; the lead
   folds them in. The record, not a conversation, is where work resumes.
2. **Revisions.** Any change to Acceptance, Design, Sections or Methods raises
   `rev`. An approval binds to one revision.
3. **Two human gates.** Gate 1: the maintainer approves a plan revision with
   `/approve rev <n>`. Gate 2: the maintainer reviews and merges the pull
   request. Agents never approve, merge, release or publish.
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
| Lead | one agent session | researches, writes and revises plans, ticks lenses, hands out pieces, folds results, moves stages, keeps the status table, asks the maintainer (batched, each ask with a recommendation) | builds, approves, merges |
| Worker | agent sessions, started by hand | one piece at a time: a section, a lens, methods, a fix, final verification; starts from the record | edits the plan, works without a claim |
| External reviewer | optional, another model | reviews a pull request before merge, in comments | pushes |

Hiko has one maintainer, who is also the language designer. Gate 1 therefore
covers both semantics and implementation-only changes; with more humans, the
language designer would hold Gate 1 for changes to the language and any
maintainer for implementation-only ones.

Workers are not specialized by crate; the piece names the area. Pieces that
touch the same files never run at once; a plan's sections go to one worker in
order; parallelism comes from lenses and independent plans. Useful team size is
bounded by independent lanes and by the maintainer's review time.

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
evidence) · Reviews (lenses ticked, or `none` with the reason) · Sections (at
most four, in order: paths, covered `A` ids, exact checks) · Methods (how each
`A` and `D` is verified) · Resolution (each finding: fixed in `D-x`, or rejected
with evidence) · Gate 1 · Rollout · Context (per active section: files, pattern
to copy, checks, next step).

**Acceptance is observable through programs:** "`hiko check p.hml` rejects it
with `cannot tell whether this arithmetic operator is on int, float or word`",
"`hiko run p.hml` prints `3` and exits 0", "the stack VM and the register VM
give the same output, error and fuel stop for every generated program", "each
rule in `docs/language/modules.md` has a program in `tests/run`".

## Lenses

Independent reviews of one revision, run in parallel, one concern each. The
plan ticks the lenses that apply and says why the others do not.

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
| `simplicity` | Reuse, needless complexity, wasted work, fixes at the wrong layer (`/simplify`). |
| `correctness` | Bugs in the diff (`/code-review`). |

Each lens posts one comment, one finding per line:

```
<lens>-<n> | blocking/advisory | D-id | what fails (input → wrong result) | smallest fix
```

The lead resolves every finding, raises `rev`, and records which lenses re-run
and why the others do not. Gate 1 only when no blocking finding is open.
`simplicity` and `correctness` also run on the pull request before Gate 2.

## Methods

Verification is evidence, not assertion. Choose per risk:

| Method | In Hiko |
|---|---|
| Formal model | TLA+ and Quint in `specs/` (`hiko run scripts/check_tla.hml`). Each fix has a switch: a model or config in `specs/tla/broken/` with the fix removed, which `check_tla.hml` requires to fail. A model passing does not prove the code. |
| Model checking the code | loom for the threaded runtime: `RUSTFLAGS="--cfg loom" cargo test`. |
| Differential | register VM against stack VM (`vm::register_check`, `fuzz_register_vm`); formatter round trip (`fuzz_roundtrip`). |
| Property tests | proptest over generated programs, with an independent oracle. |
| Fuzzing | cargo-fuzz targets in `crates/hiko-syntax/fuzz` and `crates/hiko-vm/fuzz`; state duration and input size. |
| Mutation check | inject the bug the change guards against; the test must fail; revert. |
| Conformance | programs in `tests/run`, named after the rule they check. |
| Growth | inputs of size n, 2n, 4n: time must grow linearly. Nothing quadratic. |
| Benchmark gate | the two builds timed interleaved on the same programs (`benchmarks/`), on AC power, load average below 4, else skipped; the target improves by the stated margin and no benchmark is more than 1% slower, with default and `--all-features` builds. `scripts/compare.sh` is the separate comparison against Lua and Python. |
| Live check | for external services, a run with a named read-only profile, against a build of the change. |

Final verification posts one table per acceptance item: method, commit,
result, limits. A skipped, flaky or timed-out run counts as unmet.

## Rollout

Hiko has two release channels:

- **`main`**: every merge.
- **Releases**: a version bump with `scripts/release.hml` plus a `CHANGELOG.md`
  entry, made by the maintainer.

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
ready #<pr>
```

The receiver re-reads the record before acting. A piece is claimed by a holder
suffix on its line in the plan (`— <worker>`) and a `claim:<worker>` label; no
claim, no work.

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
