# Hiko Language Definition — Draft

Status: **first draft, incomplete**. This is the entry point for the future formal Hiko specification. For now, it states the scope and points to the current source-of-truth docs.

## Purpose

This document should eventually answer three questions:

1. What Hiko syntax is valid?
2. How is valid Hiko source typed?
3. What does a well-typed Hiko program do at runtime?

## Current source of truth

Until this draft is expanded, use:

- [`hiko.ebnf`](hiko.ebnf) for the grammar snapshot.
- [`modules.md`](modules.md) for module semantics.
- [`effects.md`](effects.md) for `effect`, `perform`, `handle`, and `resume`.
- [`error-handling.md`](error-handling.md) for recoverable error conventions.
- [`numerics.md`](numerics.md) for numeric behavior.
- [`sml-deltas.md`](sml-deltas.md) for deliberate SML divergences.
- [`../architecture/runtime.md`](../architecture/runtime.md) for process/runtime behavior.
- [`../architecture/vm.md`](../architecture/vm.md) for the VM/runtime boundary.
- [`../verification/verification.md`](../verification/verification.md) for bytecode verifier coverage, tests, and the `specs/` formal-model inventory.
- The Rust implementation and tests when docs and code disagree.

## First specification outline

The first complete version should cover:

1. Program structure
2. Lexical syntax
3. Core expressions and declarations
4. Types and type inference
5. Pattern matching and exhaustiveness
6. Modules
7. Dynamic evaluation semantics
8. Algebraic effects
9. Process/runtime transitions
10. Builtin basis and implementation-defined behavior

## Relationship to `specs/`

This Definition should describe the intended language and runtime semantics. The [`../../specs/`](../../specs/) directory contains executable formal models for selected high-risk parts of those semantics, especially process lifecycle, scheduling, cancellation, `wait_any`, and numeric-width behavior.

The models are evidence and regression tools, not a replacement for this Definition. Absence of a model does not mean absence of semantics.

## Key rule

Effects are local control semantics. Async, I/O, scheduling, and cancellation are runtime transition semantics, not user-defined effect interpretation.
