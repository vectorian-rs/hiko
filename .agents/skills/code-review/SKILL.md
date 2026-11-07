---
name: code-review
description: Review a PR or branch for concrete bugs and repository instruction violations, validate findings, and report actionable results. Read-only unless fixes or PR comments are explicitly requested.
---

# Code review

Port of Claude's code-review workflow: establish the target, gather applicable
instructions, use separate auditors, validate candidates, and report findings
with high confidence. Default to a report without edits or GitHub comments.

## Target and options

Accept a PR number/URL, branch, Git revision range, or path. With no target,
review branch commits against the PR base or default branch, plus staged,
unstaged, and relevant untracked changes. State the resolved base/head and
dirty scope. An existing review covers only the same head and scope, not later
commits. Never skip a change solely because an agent wrote or reviewed it.

Accept `--fix` to apply validated fixes after the review, and `--comment` to
publish findings on the specified PR. These options do not authorize a merge
or unrelated edits. Optional `low`, `medium`, `high`, `xhigh`, or `max` arguments
request review depth; do not pretend they change the host's model or settings.
`--max-findings n` limits the reported findings by severity; `all` reports every
validated finding and `default` removes the explicit cap. Disclose any omitted
findings; never equate a capped report with resolution of the omitted issues.
Claude's cloud `ultra`/`--post` service is not implemented by this portable skill.
Report unsupported options instead of silently ignoring them.

Read PR state, title, description, diff and existing comments using available
repository tools (`gh` for GitHub). Closed PRs normally need no readiness review;
review them when explicitly requested. Draft and small PRs can still be reviewed
when requested or required by the repository. Do not treat a draft review as
permission to present or merge it.

Collect the root and ancestor instruction files governing each changed file.
Follow `AGENTS.md`; apply `CLAUDE.md` imports/rules when the active host loads
them. Do not impose rules from unrelated directories or invent conventions.
Summarize the change and intended behavior before looking for defects.

## Separate audits

Launch four read-only reviewers in separate contexts when delegation is
available and permitted, concurrently within the host's limit (use waves if
necessary). Each gets the same target, complete diff, PR title/description,
applicable instructions and review depth. Do not give a reviewer another's
findings before its initial audit.

1. **Instruction audit A.** Check changed code against explicit, applicable
   repository rules. Quote the exact rule for each proposed violation.
2. **Instruction audit B.** Independently audit the same scoped instructions.
   This intentionally repeats the first audit to catch different omissions.
3. **Diff bug audit.** Look for significant, demonstrable bugs introduced by
   the diff. Report evidence visible in the changed code; identify any missing
   context needed to validate a suspicion rather than guessing.
4. **Context bug audit.** Trace changed behavior through surrounding code,
   callers and tests. Use history, blame or prior PR discussion when needed to
   establish intent. Focus on defects introduced or exposed by this change.

If delegation is unavailable or prohibited, run these passes sequentially and
label the result a self-review. This preserves the passes, but does not supply
an independent reviewer where one is required. Report an unavailable required
reviewer as a remaining limitation.

## Validate and filter

Deduplicate candidate findings. Have a fresh read-only reviewer validate each
candidate when delegation is available and permitted. Provide the target,
instructions, candidate and claimed failure; ask it to confirm or refute the
finding from the code. Otherwise validate each yourself and disclose that mode.

Check a concrete trigger and consequence, whether the defect is new, and any
rule's exact wording and scope. Use a focused reproducer or existing test when
it resolves uncertainty. Filter out pre-existing unrelated defects, deliberate
behavior, speculative warnings, style preferences, findings already covered by
linters, general quality complaints, and explicitly permitted exceptions.
Keep a defect requiring particular inputs when those inputs and the failure
are demonstrated; uncertainty alone is not a confirmed finding.

Only report candidates supported by validation. Do not invent confidence scores
or claim that a review proves the absence of bugs. Keep unresolved verification
gaps separate from confirmed findings.

## Report, optionally fix or publish

Report validated findings by severity. Each includes the file and narrow line
range, trigger, consequence, evidence and smallest correction. Cite an explicit
rule when relevant. Use full-commit GitHub permalinks when publishing; identify
uncommitted locations as working-tree evidence instead of linking them to a
commit that does not contain the code. If none remain, say no actionable
findings were found and state the scope, reviewer mode and material limits.

With `--fix`, apply in-scope validated fixes in the matching checkout or an
isolated worktree, preserving unrelated work. Run affected checks and re-review
the changed paths; record what was fixed and what remains. Without `--fix` or
another explicit fix request, do not edit files.

With `--comment`, recheck the PR head before posting and post each unique finding
once. Use inline comments when supported, otherwise a single summary with code
links. A committable suggestion must completely fix its issue; describe larger
fixes instead. For a clean review, publish one concise summary. Without this
option or explicit publishing authorization, return the review in conversation.
In Hiko, sign published agent comments and use the delivery finding format when
recording lens evidence. This skill supplies the `correctness` review; selected
specialist lenses and required independent reviews remain separate obligations.

## Source and host adaptation

Based on Anthropic's [code-review plugin](https://github.com/anthropics/claude-plugins-official/blob/main/plugins/code-review/commands/code-review.md)
and [local review documentation](https://code.claude.com/docs/en/code-review-local),
checked 2026-11-07. Preserved the plugin's two instruction auditors, two bug
auditors, finding validation, filtering and optional publication. Adapted tool
and model selection to the active Pi or Codex host, with an explicit sequential
mode when delegated review is unavailable. Hiko's all-PR requirement overrides
the plugin's automatic trivial/draft/already-commented skipping.
