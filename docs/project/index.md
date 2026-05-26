# Project Documentation

Project-level orientation, standards, documentation process, and historical planning.

| File | Status | Summary |
|---|---|---|
| [`whitepaper.md`](whitepaper.md) | Current design intent | Main narrative and positioning document for Hiko: local algebraic effects, isolated-process runtime, semantic layers, non-goals, and update obligations. |
| [`dev-standard.md`](dev-standard.md) | Current workflow standard | Repository working standards for Rust style, error handling, testing, parser/formatter work, compiler/typechecker work, VM/runtime work, CLI behavior, docs, and git workflow. |
| [`how-to-document-hiko.md`](how-to-document-hiko.md) | Current documentation guidance | Explains the desired split between whitepaper narrative and definition-style specification, including formal semantics topics still missing. |
| [`bootstrap.md`](bootstrap.md) | Historical/bootstrap | Original technical bootstrap and staged roadmap. Useful for early design intent, not for current implementation facts. |
| [`language-evaluation-prompt.md`](language-evaluation-prompt.md) | Reusable prompt | Prompt for skeptical grading of Hiko correctness, performance, and safety. |

## Recommended use

- Start with [`whitepaper.md`](whitepaper.md) for why Hiko exists and how to reason about semantic layers.
- Open [`dev-standard.md`](dev-standard.md) before making code changes.
- Open [`how-to-document-hiko.md`](how-to-document-hiko.md) before adding or reorganizing documentation.
