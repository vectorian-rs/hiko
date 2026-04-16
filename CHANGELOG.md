# Changelog

## Unreleased

### Runtime and Process Execution

- **Lighter child processes**: spawned VMs now share immutable compiled program/code data instead of cloning bytecode, constants, and effect metadata per child.
- **Cheaper process startup**: new VMs start with a much smaller default stack and no eager 4096-slot heap reservation, reducing per-process overhead.
- **Runtime-backed local runner**: `hiko-vm-hiko-run-all-policy` now executes scripts through `Runtime`, so runtime-managed examples such as `spawn_stress.hml` are exercised by `tools/run_all.hml`.
- **Root failure reporting**: the local runtime runner now exits nonzero when the root process ends in `Failed(...)` instead of silently succeeding.
- **Documented VM stack/frame guards**: the fixed value-stack and call-frame limits are now exposed as public constants and documented alongside heap and fuel limits.
- **Boundary-triggered local GC**: long-lived processes now opportunistically collect at suspension boundaries after moderate allocation bursts, reclaiming request-local garbage sooner without introducing any global collector.
- **Typed process handles**: `spawn` now returns `Pid` instead of `Int`, and process operations use first-class `Pid` values instead of raw integers.

## 0.5.1

### Runtime and Tooling

- **Streaming output**: `print` and `println` now write to a runtime output sink immediately in `hiko-cli` and generated policy VMs instead of buffering until process exit.
- **Opt-in output capture**: VM output buffering is now disabled by default to avoid unbounded memory growth in long-running services, while runtime helpers and the harness enable capture explicitly when they need it.
- **Generated VM support**: policy-generated binaries now install the streaming stdout sink by default so tools like `tools/run_all.hml` emit progress incrementally.

## 0.5.0

### Runtime and Concurrency

- **Process runtime**: added `run_slice`, `Runtime`, `ThreadedRuntime`, `Process`, `Scheduler`, and `SendableValue` for cooperative and multi-threaded execution of Hiko programs.
- **Structured concurrency builtins**: added `spawn` and `await_process` for child process orchestration.
- **Async I/O suspension**: unhandled effect-style I/O now suspends processes through the runtime instead of forcing direct blocking calls.
- **I/O backend abstraction**: added pluggable I/O backends plus a mock backend for deterministic tests.

### Capabilities and Tooling

- **Capability-based VM builder**: added `VMBuilder` policies for core, filesystem, HTTP, exec, heap, and fuel limits.
- **Policy-compiled VMs**: `hiko build-vm` now generates standalone binaries from TOML policy files.
- **Agent harness**: added `hiko-harness` plus repo tools for agentic coding workflows.
- **Example runner tool**: moved the example harness to `tools/run_all.hml`, with per-example pass/fail output and elapsed milliseconds.

### Builtins and Standard Library Surface

- **Filesystem capabilities**: added sandboxed file and directory builtins, hashline file editing helpers, and recursive/glob helpers.
- **HTTP surface**: added general HTTP builtins plus JSON, MessagePack, and raw-bytes response helpers.
- **Bytes and randomness**: added `Bytes`, `http_bytes`, `random_bytes`, and a pure RNG API with seeded generators.
- **Utility builtins**: added regex helpers, JSON helpers, `sleep`, `string_join`, `epoch_ms`, and `monotonic_ms`.

### Correctness

- **Deep resume fixes**: fixed continuation corruption in non-tail resume contexts and added regressions for large direct-resume loops and pending-application restore.
- **Blocked continuation errors**: `resume_blocked` now reports invalid continuations instead of failing silently.
- **Filesystem root enforcement**: path checks now canonicalize roots and targets, reject traversal and symlink escape, and share one implementation across VM and heap checks.
- **Sandboxed filesystem builtins**: `glob` and recursive directory walking now re-check resolved paths so sandbox escapes fail closed.

### Examples and Documentation

- Added examples for exec, regex, JSON, pipelines, effects with I/O, typed JSON, testing, benchmarking, and agent-oriented workflows.
- Expanded README coverage for policies, generated VMs, and runtime architecture.

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

Initial release. A strict, statically typed, ML-family scripting language anchored in Core SML semantics.

### Language

- **Core SML expressions**: let/val bindings, if/then/else, case/of, fn lambdas, function application, tuples, lists (`[]`, `::`, `[a, b, c]`), type annotations.
- **Declarations**: `val`, `val rec`, `fun` (including clausal definitions with `|` and mutual recursion with `and`), `datatype`, `type` aliases, `local ... in ... end`.
- **Type system**: Hindley-Milner type inference (Algorithm W) with value restriction for let-polymorphism. Built-in types: `Int` (i64), `Float` (f64), `Bool`, `Char`, `String`, `Unit`. Type constructors: tuples, lists, arrows, user-defined datatypes.
- **Pattern matching**: wildcards, variables, literals (int, float, string, char, bool), constructors, tuples, cons (`x :: xs`), list patterns, as-patterns, nested patterns. Left-to-right, first-match semantics.
- **Exhaustiveness checking**: Maranget usefulness algorithm. Non-exhaustive matches are compile-time errors. Redundant clauses produce warnings.
- **Monomorphic operators**: `+`, `-`, `*`, `/`, `mod` for Int; `+.`, `-.`, `*.`, `/.` for Float; `^` for String; `<`, `>`, `<=`, `>=` for Int; `<.`, `>.`, `<=.`, `>=.` for Float; `=`, `<>` for scalar equality.
- **Imports**: `use "path/to/file.hk"` with relative path resolution, cycle detection, and single-evaluation semantics.

### Compiler and VM

- **Bytecode compiler**: direct AST walk producing 57 opcodes. Two-pass pattern compilation (test phase then bind phase).
- **Stack-based VM**: `Vec<Value>` stack with `Vec<CallFrame>` frames. `Value` is `Copy` (16 bytes, no reference counting).
- **Tail-call optimization**: `TailCall` opcode reuses the current call frame. Propagated through if/case/let branches.
- **Mark-and-sweep GC**: index-based `GcRef(u32)` into a `Vec<Option<HeapObject>>` arena. Worklist-based marking avoids stack overflow. Free-list reuse with adaptive collection threshold.
- **Rich diagnostics**: source-located error messages via `codespan-reporting`. Bytecode spans track back to source positions.

### Builtins

27 built-in functions: `print`, `println`, `read_line`, `int_to_string`, `float_to_string`, `string_to_int`, `char_to_int`, `int_to_char`, `int_to_float`, `string_length`, `substring`, `string_contains`, `trim`, `split`, `sqrt`, `abs_int`, `abs_float`, `floor`, `ceil`, `read_file`, `write_file`, `file_exists`, `list_dir`, `remove_file`, `create_dir`, `http_get`, `exit`, `panic`, `assert`, `assert_eq`.

### Standard Library

Written in Hiko: `stdlib/list.hk` (map, filter, foldl, foldr, length, reverse, append, nth, zip, take, drop, all, any, find), `stdlib/option.hk` (is_some, is_none, map_option, get_or, flat_map_option), `stdlib/either.hk` (map_right, map_left, is_left, is_right, from_left, from_right).

### Correctness Fixes (included in 0.1.0)

- Four type inference bugs (occurs check, let-polymorphism, constructor arity, recursive binding).
- Five cross-phase correctness issues (type checker/compiler interaction).
- Four exhaustiveness checker gaps (nested ADTs, fn patterns, distinct literals).
- TCO for clausal functions and builtins.
- Safe opcode decoding with explicit discriminants.
- Circular import detection (separate loading vs loaded state).
- Integer overflow detection with checked arithmetic.
- Span tracking for source-located runtime errors.
