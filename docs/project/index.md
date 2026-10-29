# Project Documentation

Project-level orientation, standards, documentation process, and historical planning.

| File                                                                     | Status                         | Summary                                                                                                                                                                        |
| ------------------------------------------------------------------------ | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [`project-state.md`](project-state.md)                                   | Snapshot 2026-10-29            | Current state against the refocus on minimal language, bytecode/VM efficiency, and security, with effects as research. Inventory, gaps, issue triage, cleanup order. |
| [`whitepaper.md`](whitepaper.md)                                         | Current design intent | One-page outline: focus, principles, plan, non-goals, and links to the detailed docs. |
| [`dev-standard.md`](dev-standard.md)                                     | Current workflow standard      | Repository working standards for Rust style, error handling, testing, parser/formatter work, compiler/typechecker work, VM/runtime work, CLI behavior, docs, and git workflow. |
| [`how-to-document-hiko.md`](how-to-document-hiko.md)                     | Current documentation guidance | Explains the desired split between whitepaper narrative and definition-style specification, including formal semantics topics still missing.                                   |
| [`bootstrap.md`](bootstrap.md)                                           | Historical/bootstrap           | Original technical bootstrap and staged roadmap. Useful for early design intent, not for current implementation facts.                                                         |
| [`../prompts/language-evaluation-prompt.md`](../prompts/language-evaluation-prompt.md) | Reusable prompt                | Prompt for skeptical grading of Hiko correctness, performance, and safety.                                                                                                     |

## Recommended use

- Start with [`whitepaper.md`](whitepaper.md) for what Hiko is aiming for, then [`project-state.md`](project-state.md) for where it stands.
- Open [`dev-standard.md`](dev-standard.md) before making code changes.
- Open [`how-to-document-hiko.md`](how-to-document-hiko.md) before adding or reorganizing documentation.
