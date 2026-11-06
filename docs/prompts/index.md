# Agent Prompts

Shared roles and reusable prompts for coding agents working on Hiko. Start with
the repository instructions in [AGENTS.md](../../AGENTS.md). The same role names
and prompts apply in Codex, Claude Code, and other tools.

## Delivery roles

For work covered by the [delivery lifecycle](../project/delivery.md), use the
assigned role below. These roles define authority and handoffs; the general and
specialist task prompts refine focus without changing the lifecycle's gates.

| Role | Instructions |
| --- | --- |
| `lead` | [delivery-roles.md: Lead](delivery-roles.md#lead) |
| `worker` | [delivery-roles.md: Worker](delivery-roles.md#worker) |
| `lens-reviewer` | [delivery-roles.md: Lens reviewer](delivery-roles.md#lens-reviewer) |
| `external-reviewer` | [delivery-roles.md: External reviewer](delivery-roles.md#external-reviewer) |

## General roles

| Role | Instructions | Use |
| --- | --- | --- |
| `developer` | [developer.md](developer.md) | Implement changes and validate the result. |
| `reviewer` | [reviewer.md](reviewer.md) | Review code for defects and regressions; report findings without editing. |
| `docs-writer` | [docs-writer.md](docs-writer.md) | Write and maintain documentation against current implementation facts. |

## Specialist roles

Use the following names when requesting a specialist role. Each name maps to an
existing prompt; the prompt remains the single source for that role's behavior.

| Role | Prompt |
| --- | --- |
| `code-simplifier` | [code-simplifier.md](code-simplifier.md) |
| `docs-simplifier` | [docs-simplifier.md](docs-simplifier.md) |
| `docs-reviewer` | [docs-quality-score.md](docs-quality-score.md) |
| `language-reviewer` | [language-evaluation-prompt.md](language-evaluation-prompt.md) |
| `design-reviewer` | [review-whitepaper.md](review-whitepaper.md) |

## Prompt reference

| File                                           | Status                     | Summary                                                                                                                   |
| ---------------------------------------------- | -------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| [`code-simplifier.md`](code-simplifier.md)                       | Current operational prompt | Project-specific code simplification skill for Hiko/Rust changes, focused on preserving behavior while improving clarity. |
| [`delivery-roles.md`](delivery-roles.md) | Current operational prompt | Lead, worker, lens reviewer and external reviewer prompts for the delivery lifecycle. |
| [`docs-quality-score.md`](docs-quality-score.md)                 | Review prompt              | Prompt for scoring the `docs/` folder on concision, correctness, completeness, understandability, terseness, and agent usefulness. |
| [`docs-simplifier.md`](docs-simplifier.md)                       | Operational prompt         | Prompt for compressing and clarifying docs while preserving technical meaning and source-of-truth boundaries. |
| [`language-evaluation-prompt.md`](language-evaluation-prompt.md) | Review prompt              | Prompt for skeptical grading of Hiko correctness, performance, and safety. |
| [`review-whitepaper.md`](review-whitepaper.md)                   | Review prompt              | Prompt material for whitepaper/design review. |

## Recommended use

Ask the active agent to use a role, for example: "Use the reviewer role to review
the current diff." Read only the selected role and the topic docs needed for the
task. Review roles report findings; editing roles make the requested changes.

These are task instructions, not source-of-truth language or runtime
documentation. Model selection, tool names, permissions, and provider settings
belong to the active tool's configuration. See
[agent-workflow.md](../project/agent-workflow.md) for Codex and Claude usage.
