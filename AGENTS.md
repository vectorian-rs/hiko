# Hiko agent instructions

These instructions and the roles in [docs/prompts/index.md](docs/prompts/index.md)
are shared by coding agents working on this repository.

## Project context

Hiko is an SML-derived scripting language implemented as a Rust workspace.
The main pipeline is syntax, type inference, bytecode compilation, and VM
execution. Builtins, capability policies, extensions, and the CLI expose that
pipeline to scripts and tooling.

- Read [docs/index.md](docs/index.md) to find the relevant topic documentation.
- Follow [docs/project/dev-standard.md](docs/project/dev-standard.md) for changes.
- Read [docs/project/how-to-document-hiko.md](docs/project/how-to-document-hiko.md)
  before adding or reorganizing documentation.
- Prefer source and tests for implementation facts, maintained topic docs for
  contracts, and the whitepaper for design intent. Treat research, bootstrap,
  and dated snapshots as context rather than proof of current behavior.

## Shared roles

For work covered by the [delivery lifecycle](docs/project/delivery.md), use the
assigned [delivery role](docs/prompts/delivery-roles.md): `lead`, `worker`,
`lens-reviewer`, or `external-reviewer`. Task prompts below refine the work;
they do not change that role's authority, claims, or human gates.

Outside that lifecycle, use the role requested by the user. Otherwise, use
`developer` for implementation, `reviewer` for code review, and `docs-writer`
for documentation work. Read the selected role's instructions before starting.

| Role | Instructions |
| --- | --- |
| `developer` | [docs/prompts/developer.md](docs/prompts/developer.md) |
| `reviewer` | [docs/prompts/reviewer.md](docs/prompts/reviewer.md) |
| `docs-writer` | [docs/prompts/docs-writer.md](docs/prompts/docs-writer.md) |

For simplification and specialist reviews, use the additional roles listed in
[docs/prompts/index.md](docs/prompts/index.md). A role describes the task and its
expected result; it does not select a provider, model, permission mode, or tool
API. Roles can run in the current session without native subagent support.

## Issue cycle

For tracked implementation work, use
[the issue-cycle prompt](docs/prompts/issue-cycle.md) and the
[delivery loop](docs/project/delivery.md#default-issue-cycle):
Plan → Implement → Verify → Present → Advance. Keep one issue/PR active per
implementation session. Fix failed checks within the approved scope and repeat
verification; return to planning for material changes. Record progress and
evidence in the issue so another session can resume. The maintainer approves
plan revisions and merges ready PRs.

## Working rules

- Keep changes within the requested scope and preserve unrelated local work.
- Use the tools available in the active session. Do not assume a particular
  editing, planning, shell, or delegation tool exists.
- Write repository tooling and ad hoc automation in TypeScript and run it with
  Bun. Pass subprocess arguments as arrays to `Bun.spawn`.
- Preserve diagnostics, source spans, public behavior, and ABI-sensitive runtime
  encodings unless the task explicitly changes them.
- Keep generated artifacts and relevant documentation in sync with their sources.
- Validate according to the development standard. For Rust changes, start with
  the affected crate; cross-crate changes need workspace checks. The CI lint
  entry point is `bun scripts/lint.ts`; CI tests default and all features.
- For documentation-only changes, check local links and instruction references.
- Report the result, validation actually run, and any remaining limitations.

## Required PR skills

Before presenting any PR, run [simplify](.agents/skills/simplify/SKILL.md), verify
the result, then run [code-review](.agents/skills/code-review/SKILL.md). Apply
in-scope fixes and repeat affected checks/reviews at the resulting head. This
includes docs-only PRs. These supply the `simplicity` and `correctness` reviews;
other selected lenses and required independent reviews still apply.

Follow the skills' delegated reviewer passes when the host supports and permits
delegation, in waves within its concurrency limit. Reviewers stay read-only;
the coordinating worker applies fixes. Otherwise run the same passes
sequentially and label them self-review. Record scope, passes, reviewer mode,
findings/fixes and checks at the final PR head.

## Conditional tool guidance

Apply only the section matching the tool hosting this session. Having a CLI
installed, or mentioning another tool in a task, does not activate its guidance.

### When running in Codex

- Use this file as the repository entry point and respect the instruction chain
  supplied by Codex, including applicable nested `AGENTS.md` and overrides.
- Markdown links here are reading directions; open the selected role and topic
  docs explicitly. Do not rely on Claude's `@path` import syntax.
- Use the session's Codex configuration and available tools. Claude settings,
  hooks, permission names, and native agent definitions apply only to Claude.
- Load the shared skills as `$simplify` and `$code-review`, or read their
  `SKILL.md` files directly. Native review may assist their passes; record
  what actually ran.

### When running in Pi

- Use shared `AGENTS.md` and `.agents/skills/` discovery.
- Invoke `/skill:simplify` and `/skill:code-review`; use `/reload` after skill
  changes in an active session.
- Use delegated reviewers when the session has that capability; otherwise
  use the skills' sequential self-review mode.

### When running in Claude Code

- The root `CLAUDE.md` imports this file with `@AGENTS.md`, keeping shared rules
  and role definitions in one place.
- Respect applicable Claude instruction files and rules loaded by the session.
  Use `/memory` to inspect loaded instruction files when troubleshooting.
- Use the session's Claude configuration and available tools. Codex settings,
  approval options, and native agent definitions apply only to Codex.
- Use native `/simplify` and `/code-review` when available, with the same
  required sequence and evidence rules. The shared ports describe the passes
  for sessions without those commands. Reviewers stay read-only; the
  coordinating worker applies cleanup.

See [docs/project/agent-workflow.md](docs/project/agent-workflow.md) for usage and
maintenance of this setup.
