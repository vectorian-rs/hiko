# Language Documentation

Language semantics, source syntax, public surface conventions, and deliberate differences from SML.

| File | Status | Summary |
|---|---|---|
| [`definition.md`](definition.md) | Draft spec entry point | Short first draft of the future Definition-style specification: what syntax is valid, how it is typed, and what it does. |
| [`modules.md`](modules.md) | Current source of truth | Module-system status, syntax, semantics, `use`/`import` relationship, and explicit non-goals. |
| [`effects.md`](effects.md) | Current policy/direction | Algebraic effect mental model, current guarantees/non-guarantees, and proposed direction for capability-oriented effect handling. |
| [`error-handling.md`](error-handling.md) | Current source of truth | Recoverable error policy, library-owned error types, wrapping, rendering, `Result` combinators, fiber joins, and cancellation. |
| [`numerics.md`](numerics.md) | Current source of truth | Numeric representation, operators, width-specific stdlib module policy, conversions, arithmetic semantics, and verification expectations. |
| [`sml-deltas.md`](sml-deltas.md) | Current migration policy | Hiko-vs-SML divergence policy and known SML defect clusters Hiko avoids or must guard against. |
| [`hiko.ebnf`](hiko.ebnf) | Grammar artifact | EBNF grammar snapshot for parser/grammar work. Treat the parser and tests as the implementation source of truth when they disagree. |

## Recommended use

- Looking for the language definition/spec: start with [`definition.md`](definition.md), then fall back to the relevant current source-of-truth doc.
- Changing public syntax: start with [`hiko.ebnf`](hiko.ebnf), then the relevant semantic doc.
- Changing module behavior: start with [`modules.md`](modules.md).
- Changing `effect`, `perform`, `handle`, or `resume`: start with [`effects.md`](effects.md).
- Changing errors or stdlib conventions: start with [`error-handling.md`](error-handling.md).
- Changing arithmetic or numeric conversions: start with [`numerics.md`](numerics.md).
