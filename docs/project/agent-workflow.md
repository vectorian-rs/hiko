# Coding Agent Workflow

Hiko uses one set of repository instructions, PR skills and role prompts for
Codex, Pi, Claude Code, and other coding agents.

## Entry points

| File | Purpose |
| --- | --- |
| [AGENTS.md](../../AGENTS.md) | Shared repository rules, role selection, and conditional tool guidance. |
| [CLAUDE.md](../../CLAUDE.md) | Claude entry point importing `AGENTS.md`. |
| [prompts/index.md](../prompts/index.md) | Shared role catalog and specialist prompts. |
| [.agents/skills/](../../.agents/skills/) | Shared simplify and code-review skills for Pi and Codex. |
| [delivery.md](delivery.md) | Lifecycle, role authority, claims, and human gates for covered work. |
| [dev-standard.md](dev-standard.md) | Development and validation requirements. |

Codex loads the root `AGENTS.md` through its
[project instruction discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md).
Claude Code loads the same content through its
[`@AGENTS.md` import](https://code.claude.com/docs/en/memory#share-one-file-with-other-coding-tools).
The import also supports Claude sessions that cannot load `AGENTS.md` directly.
Other tools can be instructed to read the root `AGENTS.md` explicitly.

## PR skills

Every PR follows Implement → simplify/fix → verify → code-review → fix/reverify
before Present. Both skills live in `.agents/skills/`, which Codex and Pi discover
directly. Claude uses its native commands when available.

| Host | Cleanup | Correctness review |
| --- | --- | --- |
| Codex | `$simplify` | `$code-review 223` |
| Pi | `/skill:simplify` | `/skill:code-review 223` |
| Claude Code | `/simplify` | `/code-review 223` |

Replace `223` with your PR. Without a target, the ports review the branch change
plus uncommitted work; a path or revision range can narrow that scope. Code-review
reports without editing or posting by default. Pass `--fix` to request fixes or
`--comment` to publish, for example `/skill:code-review --comment 223`. Simplify
applies behavior-preserving cleanup. Use Pi's `/reload` after skill edits; restart
Codex if a change does not appear. No provider-specific copy is needed.

The [simplify port](../../.agents/skills/simplify/SKILL.md) follows Claude's
current four cleanup passes: reuse, simplification, efficiency and abstraction
level, then consolidation and fixes. The
[code-review port](../../.agents/skills/code-review/SKILL.md) follows Anthropic's
plugin: two instruction auditors, two bug auditors, separate finding validation
and optional PR comments. It also accepts the local command's fix, depth and
finding-limit options; depth is an instruction, not a host model setting.

These adapt Claude's workflows; they are not extracted bundled prompts or its
cloud service. Sources checked 2026-11-07:
[Claude commands](https://code.claude.com/docs/en/commands),
[review plugin](https://github.com/anthropics/claude-plugins-official/blob/main/plugins/code-review/commands/code-review.md),
[local review](https://code.claude.com/docs/en/code-review-local),
[Codex skills](https://learn.chatgpt.com/docs/build-skills), and
[Pi skills](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/skills.md).

Tool APIs and model selection belong to the host. Use delegated reviewers when
available and permitted; otherwise run the same passes sequentially and label
them self-review. Pi needs a delegation-capable extension for separate reviewers.
Sequential passes do not satisfy a required independent review. Hiko's all-PR
rule replaces the plugin's draft/trivial/already-commented skip conditions.

Record scope, actual passes, reviewer mode, findings/fixes and the resulting
head in PR evidence. Generic `simplicity: none` or `correctness: none` lines
without running the passes do not supply this evidence. This is a workflow
requirement; it does not install a CI service or grant merge approval.

## Run or resume an issue

Use [the shared issue-cycle prompt](../prompts/issue-cycle.md) with an issue
number or roadmap. The [delivery loop](delivery.md#default-issue-cycle) is the
source of truth for Plan → Implement → Verify → Present → Advance.

```sh
codex "Use docs/prompts/issue-cycle.md for issue #191; resume its checkpoint."
claude "Use docs/prompts/issue-cycle.md for issue #191; resume its checkpoint."
```

A roadmap invocation selects one eligible implementation issue at a time.
It keeps the issue's existing PR and approved scope through routine fixes and
verification retries. Material plan changes need a new revision; the maintainer
approves reviewed revisions and merges ready PRs. Reviewed issue/revision pairs
can be approved in a batch and recorded separately.

The issue holds the phase/role/claim, approval, branch/PR/head, acceptance results,
review findings, CI, and next action. A new session reads that checkpoint before
continuing. One assigned session may coordinate and implement sequentially;
required independent reviews still need an independent reviewer.

## Choosing a role

Request a role in the task text. Both tools use the same role names and source
files; these names are repository prompt conventions, not native subagent types
or slash commands.

For lifecycle work, use the assigned `lead`, `worker`, `lens-reviewer`, or
`external-reviewer` [delivery role](../prompts/delivery-roles.md). Task prompts
such as `developer` or `reviewer` do not replace its claims or human gates.

```sh
codex "Use the reviewer role to review the current diff."
claude "Use the reviewer role to review the current diff."

codex "Use the developer role to fix the parser regression."
claude "Use the developer role to fix the parser regression."

codex "Use the docs-writer role to document the CLI change."
claude "Use the docs-writer role to document the CLI change."
```

Specialist roles such as `code-simplifier` and `docs-simplifier` use the existing
prompts under `docs/prompts/`. Read the selected prompt; the catalog does not
load every prompt into every task. Prompts containing a fenced `text` block use
that block as the task instructions.

## Tool conditions

The active session determines which conditional section in `AGENTS.md` applies.
Shared roles describe responsibilities and results without assuming model names,
tool APIs, provider flags, or permission settings.

- In Codex, use Codex's available tools and configuration.
- In Claude Code, use Claude's available tools and configuration.
- In another tool, use the shared rules and roles with that tool's capabilities.

A role can run in the current session. When using delegation supported by the
active tool, pass the shared repository instructions and selected role to the
worker; do not assume every tool inherits the same context.

## Maintenance and verification

- Edit shared rules in `AGENTS.md`, skills in `.agents/skills/`, and role prompts
  in `docs/prompts/`.
- Keep `CLAUDE.md` as the import, avoiding a second copy of repository rules.
- Keep provider-specific behavior in explicitly conditional sections. Local
  credentials, model choices, permissions, and machine settings belong in local
  tool configuration rather than role prompts.
- Check Markdown links and the `CLAUDE.md` import after changing these files.
- In a fresh Codex or Claude session, ask it to summarize the active repository
  instructions and identify the selected role file. Confirm the shared file and
  only the active tool's conditional guidance are used.
