# tree-sitter-hiko

Tree-sitter grammar for the Hiko language.

This package now includes:

- `grammar.js` as the grammar source
- `queries/highlights.scm` for syntax highlighting
- generated parser artifacts under `src/`
- Rust bindings via `tree-sitter-hiko`
- corpus-driven parser tests under `test/corpus/`

## Layout

- `grammar.js`: Tree-sitter grammar DSL
- `queries/highlights.scm`: syntax highlight captures
- `test/corpus/`: corpus-driven parser tests
- `tree-sitter.json`: grammar metadata used by the Tree-sitter CLI
- `src/parser.c`: generated parser
- `src/node-types.json`: generated node metadata
- `src/lib.rs`: Rust bindings exposing `LANGUAGE`
- `build.rs`: compiles the generated parser for Rust consumers

## Current coverage

The current grammar covers the module/declaration subset needed for the first
published `Std` package and its effectful filesystem wrapper:

- `use` and `import`
- `val`, `val rec`, and `fun`
- `datatype` and `type`
- `signature` and `structure`
- `effect`, `perform`, `handle`, and `resume`
- the `|>` pipeline operator in expressions
- core expressions, patterns, and type expressions used by the current `Std`
  modules

## Development

Generate the parser and run corpus tests with Bun:

```sh
bunx tree-sitter generate
bunx tree-sitter test
```

## Next steps

1. Add more expression coverage and reduce internal tree verbosity where it
   helps query readability.
2. Add `queries/locals.scm` for scope/reference-aware tooling.
3. Use `tree-sitter-hiko` from a Rust doc generator to emit syntax-highlighted
   HTML for `libraries/`.
