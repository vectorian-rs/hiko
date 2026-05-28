# Agent Prompts

Reusable prompts and skills for coding agents working on Hiko.

| File                                           | Status                     | Summary                                                                                                                   |
| ---------------------------------------------- | -------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| [`code-simplifier.md`](code-simplifier.md)                       | Current operational prompt | Project-specific code simplification skill for Hiko/Rust changes, focused on preserving behavior while improving clarity. |
| [`docs-quality-score.md`](docs-quality-score.md)                 | Review prompt              | Prompt for scoring the `docs/` folder on concision, correctness, completeness, understandability, terseness, and agent usefulness. |
| [`docs-simplifier.md`](docs-simplifier.md)                       | Operational prompt         | Prompt for compressing and clarifying docs while preserving technical meaning and source-of-truth boundaries. |
| [`language-evaluation-prompt.md`](language-evaluation-prompt.md) | Review prompt              | Prompt for skeptical grading of Hiko correctness, performance, and safety. |
| [`review-whitepaper.md`](review-whitepaper.md)                   | Review prompt              | Prompt material for whitepaper/design review. |

## Recommended use

Use these as prompt assets. They are not source-of-truth project documentation unless copied into the active agent or skill configuration.
