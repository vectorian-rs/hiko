# Changelog

## 0.2.0

### Language Features

- **Algebraic effect handlers**: OCaml 5-style shallow handlers with delimited continuations. Declare effects with `effect Name of Type`, perform with `perform Name arg`, handle with `handle expr with return x => ... | Effect x k => ...`, and resume captured continuations with `resume k value`. Supports nested handlers, recursive handlers, and patterns like generators, mutable state, and structured error handling.
- **Desugaring pass**: list literals, `andalso`/`orelse`, `not`, and parentheses are desugared to core AST before type checking. The compiler and type checker no longer special-case surface syntax.
- **Constant folding**: compile-time evaluation of integer/float arithmetic, string concatenation, comparisons, negation, and branch elimination on literal conditions.
- **Effect type checking**: `perform` validates argument types against effect declarations. Unknown effects are caught at compile time.
- **Escape sequences**: `\r`, `\0`, and `\xHH` (hex byte) added to string and char literals.

### Performance

- **Rc captures**: closure captures use `Rc<[Value]>` instead of `Vec<Value>`. Cloning on every function call is now a refcount bump instead of a heap allocation.
- **O(arity) call dispatch**: `Op::Call` copies arguments in O(arity) instead of O(stack_depth) via `Vec::remove`.
- **String constant interning**: `Op::Const` for strings caches the `GcRef` after first allocation. Loops that load the same string constant pay zero clones.
- **Indexed globals**: global variables stored in `Vec<Value>` indexed by slot ID. `GetGlobal`/`SetGlobal` is a single array index instead of string hashing.
- **GC trigger fix**: removed `free_list.is_empty()` guard from `should_collect` that could suppress collection indefinitely.
- **String interning**: all AST identifiers use `Symbol(u32)` instead of `String`. Cloning AST nodes copies u32 values instead of heap-allocating strings.
- **Zero-copy parsing**: parser uses `mem::take` to move strings out of tokens instead of cloning.
- **Iterative list display**: `println` on cons-lists prints `[a, b, c]` iteratively instead of recursing (prevents stack overflow on deep lists).

### Correctness

- **Pattern test cleanup**: failed pattern match arms now pop test temporaries from the stack via per-fail-point trampolines. Fixes the `[x]` pattern scoping bug where variables from failed arms leaked into subsequent arms.
- **Checked negation**: `Op::Neg` uses `checked_neg()` to detect overflow on `i64::MIN`.
- **Safe pop**: `pop()` returns `Result` instead of panicking on stack underflow.
- **Safe heap access**: `heap.get()` returns `Result` instead of panicking on dangling or out-of-range `GcRef`.
- **Overflow checks**: `patch_jump` and `add_constant` use `try_from` to detect bytecode size and constant pool overflow instead of silently wrapping.
- **Parser depth limit**: recursion depth capped at 256 to prevent stack overflow on deeply nested input.
- **Structural equality**: `assert_eq` compares tuples and ADTs structurally (was always false for non-string heap objects).
- **Negative index safety**: `bi_substring` uses `usize::try_from` instead of `as usize` cast for negative values.

### Architecture

- **Two-pass compilation**: type inference runs across all declarations before any codegen. All type errors are reported before bytecode is generated. Imported files are parsed, desugared, and type-checked in pass 1, compiled from cache in pass 2.
- **Typed codegen**: type inference stores resolved types in `expr_types` map keyed by `Span`. The compiler queries it to emit type-specific opcodes (e.g., `NegFloat` for float negation).
- **GcRef encapsulation**: inner field changed from `pub` to `pub(crate)`. VM heap is private with `heap_live_count()` accessor.
- **Dead opcodes removed**: `EqBool`, `NeBool`, `EqChar`, `NeChar`, `EqString`, `NeString`, `EqFloat`, `NeFloat` removed (never emitted by compiler). Polymorphic `Eq`/`Ne` handle all equality.
- **Opcode renames**: `NegInt` to `Neg`, `EqInt` to `Eq`, `NeInt` to `Ne` (these are polymorphic, not int-specific).

### File Extension

- Changed from `.hk` to `.hml` (no conflicts with any known format).

### Documentation

- Added comprehensive README with language syntax, effect handler examples, architecture overview, and implementation details.
- Removed em dashes from all source comments and documentation.

### Examples

New examples: `fizzbuzz`, `fibonacci_print`, `generator` (yield/collect), `state` (get/put effects), `exceptions` (error handling via effects), `quicksort`, `binary_tree` (BST with insert/search/traverse), `word_count`.

### HTTP

- `http_get` now returns `(Int, (String, String) list, String)` with status code, response headers as key-value pairs, and body (was `String` body only).

## 0.1.0

Initial release. Core SML semantics with Hindley-Milner type inference, algebraic data types, exhaustive pattern matching (Maranget algorithm), tail-call optimization, mark-and-sweep garbage collector, bytecode VM with 57 opcodes, and import system with cycle detection.
