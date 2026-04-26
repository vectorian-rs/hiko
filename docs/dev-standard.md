# Development Standard

This document captures the working standards for changes in the Hiko repository. Prefer these rules unless a crate-level design or issue explicitly requires otherwise.

## General principles

- Preserve behavior unless the issue explicitly requests a behavior change.
- Prefer simple, explicit code over clever or overly compact code.
- Keep changes focused on the requested scope; avoid incidental rewrites.
- Make invariants visible with clear names, narrow types, and targeted tests.
- Avoid introducing new dependencies unless they remove meaningful complexity or are already part of the project architecture.
- Keep generated files in sync with their sources.

## Rust style

- Run `cargo fmt` before committing.
- Prefer small functions with descriptive names over large functions with hidden phases.
- Prefer explicit error propagation with `Result` over panics in production code.
- `unwrap`/`expect` are acceptable in tests and for truly infallible formatting into `String`; otherwise prefer contextual errors.
- Avoid unnecessary cloning and allocation, especially in compiler, formatter, and VM hot paths.
- Avoid broad visibility. Use private items by default; expose only stable crate APIs.
- Prefer concrete types and straightforward ownership over generic abstractions unless reuse is clear.
- Keep unsafe code out of normal changes unless unavoidable and documented with a safety comment.

## Error handling

- Return structured errors from library APIs where callers can recover or report diagnostics.
- CLI commands should print actionable errors and use stable exit codes.
- Preserve source locations/spans in diagnostics when available.
- Do not hide parse/type/runtime failures behind generic messages.

## Testing and verification

For Rust changes, run the narrowest relevant set first, then broaden before committing:

```sh
cargo fmt
cargo check -p <crate>
cargo test -p <crate>
cargo clippy -p <crate> --all-targets -- -D warnings
```

For cross-crate or public behavior changes, prefer:

```sh
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Add or update tests for:

- bug fixes
- public API behavior
- parser/formatter round trips
- compiler/typechecker/runtime boundary behavior
- regression-prone ABI or encoding assumptions

## Parser and formatter standards

- Keep the handwritten parser/typechecker pipeline separate from source-preserving formatting concerns.
- Tree-sitter grammar changes must update generated artifacts:
  - `crates/tree-sitter-hiko/src/grammar.json`
  - `crates/tree-sitter-hiko/src/node-types.json`
  - `crates/tree-sitter-hiko/src/parser.c`
- Validate grammar changes with:

```sh
cd crates/tree-sitter-hiko
npm test
npm run generate
cargo test -p tree-sitter-hiko
```

- `hiko fmt` must be deterministic and idempotent.
- Formatter output should remain parseable by `tree-sitter-hiko`; for valid Hiko source, it should also remain compatible with the compiler parser unless explicitly documented.
- Preserve comments intentionally, not as an afterthought.

## Compiler and typechecker standards

- Keep source-of-truth metadata centralized where possible.
- Avoid duplicated builtin/type/runtime ABI tables.
- When constructor tags or runtime encodings matter, add regression tests for ABI stability.
- Prefer clear diagnostic messages that mention the user-visible construct involved.

## VM and runtime standards

- Treat runtime encodings, builtin names, constructor tags, and process/error values as ABI-sensitive.
- Avoid unnecessary cloning in value delivery, builtin calls, and runtime scheduling paths.
- Keep concurrency behavior explicit and testable.
- For process/runtime behavior, add tests for cancellation, failure, reaping, fuel/limit handling, and sendability boundaries where relevant.

## CLI standards

- Keep command behavior predictable and script-friendly.
- Support `--check`-style CI workflows where appropriate.
- Use nonzero exit codes consistently:
  - `0`: success
  - `1`: requested check failed, such as formatting needed
  - `2` or another documented nonzero code: parse/usage/tooling error, where already established
- Do not mutate files in `--check` mode.

## Documentation standards

- Document user-facing commands, config, policies, and language behavior close to the feature.
- Keep examples executable or syntactically valid where possible.
- Update issue comments with implementation commit and validation commands when closing tracked work.

## Git and issue workflow

- Keep unrelated local changes untouched.
- Mention unrelated dirty files before committing if they exist.
- Use focused commit messages that describe the behavior or architecture change.
- For GitHub issues, comment with:
  - implementation commit
  - short summary
  - validation commands
- Close issues only when the documented acceptance criteria are satisfied or intentionally rescoped.
