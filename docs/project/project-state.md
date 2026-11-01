# Project State

Snapshot: 2026-10-29. Version: 0.8.0. Last commit: `8b18b58` (2026-06-01).

This document records where Hiko stands against a narrowed focus. It is an
inventory and triage, not a plan that has been executed. Nothing listed as
"out of focus" has been removed yet.

## Focus

Hiko is being refocused on three things:

1. **Minimal language.** A small, strict, statically typed ML core anchored in
   Core SML.
2. **Efficient bytecode and VM.** A compact instruction set, a verifier, and a
   predictable interpreter.
3. **Security.** Capability-gated host access, fail-closed resource limits,
   bytecode verification, and verified module loading.

The **effect system** is the research part of the language. It stays in the
tree as a small, isolated experiment, not a stable surface. The supported uses
are providers and aborts; generators and state use closures (decided
2026-10-30).

Everything else (cloud providers, agent harness, editor tooling, demos) is
either supporting tooling or out of focus. See [Inventory](#inventory).

## Health

| Check                    | Result                                                                                                                                                                                                                                                                     |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cargo test --workspace` | All pass with no network and no module server (829 tests after PR B). |
| Module resolution in tests | Tests resolve lockfile packages (`Std`, ...) from `libraries/` on disk via `ModuleLoaderOptions::local_package_root`, still verified against `hiko.lock.toml` BLAKE3 hashes. Cache writes go to a temp dir, never `~/.hiko/lib-cache`. |
| `unsafe`                 | None in `hiko-vm/src`. Only in the allocation-counting benchmark example.                                                                                                                                                                                                  |
| Rust tests               | About 795 `#[test]` functions, plus golden/run/error tests under `tests/`.                                                                                                                                                                                                 |

Size by crate (lines of Rust):

| Crate               | Lines   | Role                                                             |
| ------------------- | ------- | ---------------------------------------------------------------- |
| `hiko-vm`           | ~22,400 | VM, verifier, GC, runtimes, builtins, config/policy              |
| `hiko-syntax`       | ~6,200  | Lexer, parser, desugar, const folding, formatter, pretty printer |
| `hiko-compile`      | ~3,300  | AST to bytecode, module loader                                   |
| `hiko-types`        | ~2,900  | HM inference, exhaustiveness                                     |
| `hiko-cli`          | ~2,100  | `run`, `check`, `fmt`, `lock`, `hash`                            |
| `hiko-builtin-meta` | ~1,900  | Builtin signatures, internal module sources                      |
| `hiko-harness`      | ~1,450  | LLM agent harness                                                |
| `hiko-docgen`       | ~1,200  | HTML docs for libraries                                          |
| `hiko-common`       | ~80     | Shared hash/HTTP helpers                                         |

## Language

### Implemented

- Strict evaluation, lexical scope, closures, recursion, tail calls.
- Hindley-Milner inference with the value restriction.
- Types: `int` (i64), `word` (u64), `float` (f64, IEEE 754 double), `bool`,
  `char` (Unicode scalar value), `string` (UTF-8), `unit`, `bytes` (raw byte
  sequence), tuples, lists, `datatype` with type parameters.
- Monomorphic operators: `+ - * / mod` for `int`, `+. -. *. /.` for `float`,
  `^` for strings. No overloading or defaulting.
- `fun` with clauses, `and` for mutual recursion, `fn`, `let`, `case`, `if`.
- Pattern matching with compile-time exhaustiveness and redundancy checks.
- `|>` pipeline, desugared to application.
- Minimal modules: top-level `structure`, `signature`, opaque ascription `:>`,
  transparent ascription `:`, qualified names.
- Imports: `use "./path.hml"` for local files and `import Package.Module` or
  `import Package.Module (a, b)` for named modules.

### Deliberately absent

- Functors, `open`, `where type`, sharing, nested modules, first-class modules.
- SML exceptions (`exception`, `raise`, `handle` for exceptions). Errors use
  `Result`, `Option`, `panic`, and effects.
- `ref` cells and mutable state.
- Records and record row inference.

### Open language questions

- Records: [`sml-deltas.md`](../language/sml-deltas.md) defers them. A minimal
  language without records forces tuples or single-constructor datatypes for
  every struct-like value. This is a real usability gap.
- Mutable state: no `ref` means no in-place algorithms. This is consistent with
  the focus, but it should be a stated decision, not an omission.
- Formal definition: [`definition.md`](../language/definition.md) is a short
  draft. A minimal language is a good candidate for a complete static and
  dynamic semantics.

## Bytecode

Source: `crates/hiko-compile/src/op.rs`, `crates/hiko-compile/src/chunk.rs`.

- Stack-based, one byte per opcode, 77 opcodes (numbered 0 to 78; 44 and 55
  are unused).
- Operands are inline `u8`, `u16`, or `i16`. Jumps are relative `i16`.
- `CompiledProgram` holds the main `Chunk`, `Arc<[FunctionProto]>`, effect
  metadata, and the global table (the VM name of each global slot).
  `GetGlobal` and `SetGlobal` take a slot. Each `FunctionProto` records name, arity, capture count, and
  maximum local slots. Child processes share the program through `Arc`.
- Each `Chunk` holds code bytes, a constant pool (int, float, word, string,
  char), and an offset-to-span table.

| Group     | Opcodes                                                                                                |
| --------- | ------------------------------------------------------------------------------------------------------ |
| Constants | `Const`, `Unit`, `True`, `False`                                                                       |
| Variables | `GetLocal`, `SetLocal`, `GetLocal0-3`, `SetLocal0-3`, `GetUpvalue`, `GetGlobal`, `SetGlobal`, `Pop`    |
| Int       | `AddInt`, `SubInt`, `MulInt`, `DivInt`, `ModInt`, `Neg`, `LtInt`, `GtInt`, `LeInt`, `GeInt`            |
| Float     | `AddFloat`, `SubFloat`, `MulFloat`, `DivFloat`, `NegFloat`, `LtFloat`, `GtFloat`, `LeFloat`, `GeFloat` |
| Word      | `AddWord`, `SubWord`, `MulWord`, `DivWord`, `ModWord`, `LtWord`, `GtWord`, `LeWord`, `GeWord`          |
| Other     | `Eq`, `Ne`, `Not`, `ConcatString`                                                                      |
| Data      | `MakeTuple`, `GetField`, `GetField0-1`, `MakeData`, `MakeCons`                                         |
| Control   | `Jump`, `JumpIfFalse`, `JumpIfNotTag`, `JumpIfNotEq`, `JumpIfEq`, `JumpIfNot{Lt,Gt,Le,Ge}Int`, `Panic`, `Halt` |
| Calls     | `MakeClosure`, `MakeRecClosures`, `Call`, `TailCall`, `CallDirect`, `TailCallDirect`, `Return` |
| Effects   | `InstallHandler`, `Perform`, `RemoveHandler`                                                           |

### Bytecode observations

- Opcode numbering has gaps in grouping (`CallDirect` = 57 sits after
  `RemoveHandler` = 56, `GeWord` = 59 is separated from the other word
  comparisons). This is harmless, but a cleanup pass could renumber opcodes by
  group before any bytecode format is considered stable.
- There is no serialized bytecode format or version tag. Bytecode only exists
  in memory after compilation.
- Equality (`Eq`, `Ne`) is structural and generic. There are no specialised
  int/char equality opcodes, so every equality test goes through the generic
  path.
- Constructor patterns test the tag with one `JumpIfNotTag`. The condition of
  an `if` that compares two values with `=`, `<>` or an int comparison is one
  compare-and-branch instruction, and so is a literal pattern. Float and word
  comparisons in conditions are still a compare followed by `JumpIfFalse`.

## VM

Source: `crates/hiko-vm/src/vm/`, `value.rs`, `heap.rs`, `verify.rs`.

### Implemented

- `Value` is a 16-byte `Copy` enum: `Int`, `Word`, `Pid`, `Float`, `Bool`,
  `Char`, `Unit`, `Heap(GcRef(u32))`, `Builtin(u16)`.
- Heap objects: `String`, `Tuple`, `Data { tag, fields }`,
  `Closure { proto_idx, captures: Arc<[Value]> }`, `Bytes`, `HostHandle`,
  `Rng`.
- Dispatch is a `match` loop in `vm/dispatch.rs`. All operand reads are
  bounds-checked. Truncated bytecode is a runtime error, not a panic. Call
  frames and handler frames have fixed caps.
- GC: per-process mark-and-sweep, worklist marking, free-list reuse, adaptive
  threshold. Roots are stack, frames, handlers (including hidden ones), globals,
  and the string cache.
- Execution: `run()` (unbounded unless fuel is set) and `run_slice(reductions)`,
  which returns at `Done`, `Failed`, `Cancelled`, `Yielded`, `Spawn`, `Await`,
  `WaitAny`, or `Io`.
- Process creation: about 0.4 µs and 7 allocations per spawn, with or without
  a filesystem policy (October 2026, #104 and #124; see
  [`performance.md`](../architecture/performance.md)).

### Efficiency gaps

- Spawning trivial children in sequence gets slower with more workers (22 ms
  at 1 worker, 87 ms at 8 for 6,000 children): the scheduler wakes a worker
  per enqueue. CPU fan-out scales 4.9x at 8 workers.
- `scripts/bench.sh` runs a reproducible benchmark set, and
  [`performance.md`](../architecture/performance.md) records a baseline.
  Results are not yet tracked automatically over time.

## Security

### Implemented

| Layer                | What it does                                                                                                                                                                                                                                                           |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Two-tier gating      | A Cargo feature compiles a builtin in. A runtime policy (`VMBuilder` config, `*.policy.toml`) must also enable it. Disabled builtins are not callable.                                                                                                                 |
| Filesystem           | Per-builtin allowed `folders`. Access goes through `cap_std`, so paths cannot escape the granted directories.                                                                                                                                                          |
| HTTP                 | Per-builtin `allowed_hosts`.                                                                                                                                                                                                                                           |
| Exec                 | Explicit `allowed_commands`. No shell. Resolved paths are cached and re-validated.                                                                                                                                                                                     |
| Resource limits      | `max_fuel`/`max_work`, `max_heap`, `max_memory_bytes`, `max_host_work`, `max_io_bytes`, `max_host_resources` plus per-kind caps. Fuel, memory, I/O, host work and host resources are shared by a process tree; `max_live_processes`, `max_spawns` and `max_message_bytes` bound spawning and values sent between processes. Exhaustion is a controlled runtime error. Type checking has its own work limit, `max_type_work`. See [`execution-bounds.md`](../architecture/execution-bounds.md).                           |
| Bytecode verifier    | Runs before execution. Checks opcode decoding, operand bounds, constant/function/upvalue/local indexes, jump and handler targets on instruction boundaries, stack depth on all reachable paths, effect tags. See [`verification.md`](../verification/verification.md). |
| Process isolation    | Per-process heap. No pointers between heaps. Values cross boundaries only as `SendableValue`.                                                                                                                                                                          |
| Module integrity     | Manifest (`hiko.toml`), lockfile (`hiko.lock.toml`) with BLAKE3 hashes, `lock verify`, content-addressed cache, policy check before download.                                                                                                                          |
| Builtin shape checks | `shape_check.rs` verifies at runtime that builtin results match their declared signatures (#76 class of bug).                                                                                                                                                          |
| Formal models        | TLA+ for process lifecycle, threaded scheduler, cancellation/I/O race, `wait_any`, numeric width semantics. Quint ports partly lag.                                                                                                                                    |

### Security gaps

- **Verifier scope.** It does not prove value types, field presence, data tag
  ownership, callee callability, or arity for indirect calls. These are runtime
  checks. A typed bytecode verifier (or type-preserving compilation evidence)
  would be the strongest addition for the focus.
- **No fuzzing.** The April 2026 self-assessment grades fuzz/property testing
  D. The parser, type checker, verifier, policy parser, and `SendableValue`
  decoder are all untrusted-input surfaces with no fuzz targets.
- **Non-preemptive host calls.** Slices bound opcode dispatch only. A builtin,
  a sync host call, or a GC cycle runs to completion once entered.
- **Coarse I/O limits.** One global `max_io_bytes`. No per-operation limits
  (#75).
- **Policy verbosity.** Each builtin needs its own `[capabilities.x.y]` table
  (74 tables in the full example). There is no grouping or default-deny summary
  view, so policies are hard to review.
- **Test dependency on a local server.** See [Health](#health).

## Effects (research)

### Implemented

- `effect E of t`, `perform E v`,
  `handle e with return x => ... | E x k => ...`, `resume k v`.
- Static checks: declared effects only, payload types match, `k` has type
  `effect-result -> handled-body-result`, handler arms agree on result type.
- Two clause forms (#121), described in
  [`effects.md`](../language/effects.md):
  - **Provider:** every path ends in `resume k e`. The clause is a closure that
    runs at the perform site with its handler hidden; its value is the result
    of `perform`. No allocation per request, constant stack.
  - **Abort:** the clause does not use `k`. `perform` unwinds to the handler.
- `k` is not a value. Any other use is a compile error with a source location.
- The threaded runtime's I/O suspension is a separate mechanism and does not
  use effect handlers.

### Decided

- **Scope (2026-10-30).** Effects keep two uses: providers and aborts.
  Generators and state use closures and ordinary arguments; the examples and
  docs were converted. Measured on a debug build: summing a 100,000-item
  effect generator overflowed the stack after 25.8s, the closure version took
  1.6s.
- **First-class continuations removed (#121).** `Resume`, `Continuation` and
  the deep/shallow rule from #119 are gone. Release build, main vs #121: a
  provider loop overflowed the stack between 10,000 and 30,000 requests on
  main and runs 1,000,000 requests in 0.29s with #121. A request performed 200
  frames deep, 20,000 times: 0.63s on main, 0.48s with #121. Abort benchmarks
  are unchanged.

### Open research questions

These are the questions the research track should answer, in rough order:

1. **One-shot or multi-shot.** Answered (#121): neither. `k` is not a value.
2. **Deep or shallow.** Answered (#121): a provider stays installed for later
   requests (deep) and runs with its handler hidden.
3. **Effect rows in types.** Function types do not record effects, so
   unhandled effects are runtime errors (#48, #74).
   [`effects-design.md`](../rnd/effects-design.md) lists strategies A to F.
4. **Effects and capabilities.** Whether host capabilities become effects
   handled by the runtime, so a policy is a top-level handler. This is the
   point where the research track meets the security focus.
5. **Resume cost.** Answered (#121): a provider clause runs at the perform
   site, so nothing is copied, and it uses constant stack (#29).
6. **Verifier support.** The verifier checks effect tags and stack depth only.
   A protocol-level check is still open.

## Inventory

Classification of each part of the repo against the focus. "Out of focus"
means a candidate to move to a separate repository or delete. No action has
been taken.

### Crates

| Crate               | Class        | Notes                                                                                                                    |
| ------------------- | ------------ | ------------------------------------------------------------------------------------------------------------------------ |
| `hiko-syntax`       | Core         | Declares a `tree-sitter` and `tree-sitter-hiko` dependency, but no source file references them. Probably removable.      |
| `hiko-types`        | Core         |                                                                                                                          |
| `hiko-compile`      | Core         | Also contains the HTTP module loader behind `loader-http`.                                                               |
| `hiko-vm`           | Core         | Also contains all builtin domains, AWS, GitHub, the threaded runtime, and the async I/O backend. Largest cleanup target. |
| `hiko-cli`          | Core         | Default features enable every builtin, including GitHub.                                                                 |
| `hiko-builtin-meta` | Core         | Embeds Hiko source for internal modules such as `Hashline`.                                                              |
| `hiko-common`       | Core         |                                                                                                                          |
| `tree-sitter-hiko`  | Tooling      | Editor support. Used by `hiko-docgen`.                                                                                   |
| `hiko-docgen`       | Tooling      | Generates the HTML under `libraries/`.                                                                                   |
| `hiko-harness`      | Out of focus | LLM agent harness with OpenAI/Anthropic providers.                                                                       |

### Builtin domains in `hiko-vm`

| Domain                                                | Class             | Notes                                                                                                |
| ----------------------------------------------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| stdio, convert, string, math, bytes, numeric, testing | Core              | Pure or minimal host access.                                                                         |
| filesystem, exec, env, process, path, time, random    | Security showcase | These are what the capability model protects. Keep, but they are the main fuzzing and audit targets. |
| http                                                  | Security showcase | Host allowlist. Also the module loader transport.                                                    |
| json, regex, hash, date                               | Library           | Useful, but not part of a minimal language. Could move behind non-default features.                  |
| AWS (config, S3, SQS)                                 | Out of focus      | Pulls in `aws-sdk-*` and `tokio`. Already non-default.                                               |
| GitHub                                                | Out of focus      | Enabled by default in `hiko-cli`.                                                                    |
| Hashline                                              | Out of focus      | Agent editing support.                                                                               |

### Runtime

| Part                                                          | Class           | Notes                                                                                                |
| ------------------------------------------------------------- | --------------- | ---------------------------------------------------------------------------------------------------- |
| Single-threaded runtime                                       | Core            |                                                                                                      |
| Threaded runtime (`threaded.rs`, about 2,100 lines)           | Decision needed | Most of the TLA+ work targets it. It is valuable for the isolation story but large for a minimal VM. |
| Structured concurrency (`spawn`, `await`, `wait_any`, cancel) | Decision needed | Core of the isolated-process design.                                                                 |

### Libraries (`libraries/`)

| Library                | Class        |
| ---------------------- | ------------ |
| `Std`                  | Core         |
| `Hash`, `Http`         | Library      |
| `Aws`, `Gh`, `Harness` | Out of focus |

### Top-level files and folders

| Path                               | Class           | Notes                                                                                                                           |
| ---------------------------------- | --------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `initial.md`                       | Historical      | Original project brief. Could move to `docs/project/`.                                                                          |
| `SYSTEM.md`, `hiko-harness.toml`   | Out of focus    | Harness system prompt and config.                                                                                               |
| `test_hashline.hml`                | Stray           | Test file at the repository root.                                                                                               |
| `vim-sml.zip`                      | Stray           | Binary in git.                                                                                                                  |
| `_apalache-out/`                   | Generated       | Already in `.gitignore`. Safe to delete locally.                                                                                |
| `policies/`                        | Mixed           | `aws-*`, `github-*`, `sre-aws-*`, `harness-tools`, `work-demo-*` are out of focus.                                              |
| `tools/`                           | Mixed           | `run_all.hml` is the test runner. The rest (`bash`, `edit`, `grep`, ...) are harness tools.                                     |
| `examples/`                        | Mixed           | 77 files. Includes AWS, GitHub, HTTP, and several `benchmark_*` variants. Needs pruning to language, VM, and security examples. |
| `benchmarks/`                      | Core            |                                                                                                                                 |
| `specs/`                           | Core (security) |                                                                                                                                 |
| `examples/fizzbuzz.hml`            | Uncommitted     | Contains an invalid sketch of a second `fizzbuzz`.                                                                              |

## Documentation health

| Doc                                                      | Problem                                                                                                                                  |
| -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `docs/language/imports.md`                               | "What Is Next" lists a configurable cache location. Check against the current loader before keeping it.                                  |
| `docs/verification/verification-status-20260428.md`      | Six months old. Several items it lists as gaps (local-slot checks) are now done per `verification.md`.                                   |
| `docs/rnd/`, `docs/prompts/`, `docs/builtins/parquet.md` | Agent and proposal material. Out of focus.                                                                                               |
| `docs/index.md`                                          | Says "Review snapshot: 2026-05-26".                                                                                                      |

## Issue triage

| Issue                                        | Class         | Status                                                                                       |
| -------------------------------------------- | ------------- | -------------------------------------------------------------------------------------------- |
| #54 Module cache directory setting           | Security      | Cache implemented. Remaining: choose the directory from the CLI or `hiko.toml`.             |
| #59 Bundle modules and policy into artifacts | Security      | Open. Needs a decision on whether generated artifacts stay.                                 |
| #48, #74 Effect typing                       | Research      | Open.                                                                                        |
| #10 Isolated-process runtime with effects    | Research      | Tracking issue.                                                                              |
| #53, #56, #57, #85 Harness and agent runtime | Out of focus  | Close or move with the harness.                                                              |

Closed since the first snapshot: #12 (`Arc<str>` strings, #125), #25 (Loom
tests, #103), #29 and #30 (effects, #121), #40, #47, #81 and #82 (#104), #55
(manifest and lockfile), #75 (host-work calibration, #129), #80 (tree-wide
host resource limits, #128). Not covered by #75's closing: tail-latency
percentiles are not measured; `scripts/bench.sh` reports means, minimums and
maximums.

## Suggested cleanup order

1. Fix the tests so they do not need a local HTTP server.
2. Done: effects reduced to provider and abort clauses (#121).
3. Close the finished issues (#80, #54, #55) and the out-of-focus ones.
4. Move `hiko-harness`, AWS, GitHub, Hashline, `SYSTEM.md`, and their
   policies, examples, and libraries out of the
   repository.
5. Decide on the threaded runtime and structured concurrency.
6. Make `json`, `regex`, `hash`, and `date` non-default features.
7. Remove the unused tree-sitter dependency from `hiko-syntax`.
8. Fix the README inconsistencies listed above.
9. Add fuzz targets for the parser, type checker, verifier, and policy parser.
10. Continue the effects research track with question 3 (effect rows).
