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
tree, but it is treated as an experiment with explicit open questions rather
than a stable surface.

Everything else (cloud providers, agent harness, editor tooling, demos) is
either supporting tooling or out of focus. See [Inventory](#inventory).

## Health

| Check                    | Result                                                                                                                                                                                                                                                                     |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cargo test --workspace` | 416 passed, 7 failed (`hiko-vm` `threaded::tests::*fiber*`)                                                                                                                                                                                                                |
| Cause of failures        | Tests import `Std.Result` from `http://127.0.0.1:8000`. The last commit changed `Result.hml`, so its hash no longer matches the cache in `~/.hiko/lib-cache`, and the fetch fails when no local server runs. With `libraries/` served on :8000 all 43 threaded tests pass. |
| Fix needed               | Unit tests must not depend on a running HTTP server. Resolve `Std` from `libraries/` on disk in tests.                                                                                                                                                                     |
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
  qualified names. `:` is parsed but behaves exactly like `:>`; transparent
  ascription is not implemented.
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
- Ascription with `:`: it is accepted but seals types like `:>`, which
  contradicts SML. Either implement transparent ascription or remove `:` and
  keep only `:>`. Removing it fits the minimal-language focus. No test covers
  transparent behavior today.
- Formal definition: [`definition.md`](../language/definition.md) is a short
  draft. A minimal language is a good candidate for a complete static and
  dynamic semantics.

## Bytecode

Source: `crates/hiko-compile/src/op.rs`, `crates/hiko-compile/src/chunk.rs`.

- Stack-based, one byte per opcode, 71 opcodes (numbered 0 to 70).
- Operands are inline `u8`, `u16`, or `i16`. Jumps are relative `i16`.
- `CompiledProgram` holds the main `Chunk`, `Arc<[FunctionProto]>`, and effect
  metadata. Each `FunctionProto` records name, arity, capture count, and
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
| Data      | `MakeTuple`, `GetField`, `GetField0-1`, `MakeData`, `GetTag`, `MakeCons`                               |
| Control   | `Jump`, `JumpIfFalse`, `Panic`, `Halt`                                                                 |
| Calls     | `MakeClosure`, `Call`, `TailCall`, `CallDirect`, `TailCallDirect`, `Return`                            |
| Effects   | `InstallHandler`, `Perform`, `Resume`, `RemoveHandler`                                                 |

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
- No superinstructions for compare-and-branch (for example `LtIntJumpIfFalse`).
  Loops and pattern matches pay for a separate compare and jump.

## VM

Source: `crates/hiko-vm/src/vm/`, `value.rs`, `heap.rs`, `verify.rs`.

### Implemented

- `Value` is a 16-byte `Copy` enum: `Int`, `Word`, `Pid`, `Float`, `Bool`,
  `Char`, `Unit`, `Heap(GcRef(u32))`, `Builtin(u16)`.
- Heap objects: `String`, `Tuple`, `Data { tag, fields }`,
  `Closure { proto_idx, captures: Arc<[Value]> }`, `Bytes`, `HostHandle`,
  `Rng`, `Continuation`.
- Dispatch is a `match` loop in `vm/dispatch.rs`. All operand reads are
  bounds-checked. Truncated bytecode is a runtime error, not a panic. Call
  frames and handler frames have fixed caps.
- GC: per-process mark-and-sweep, worklist marking, free-list reuse, adaptive
  threshold. Roots are stack, frames, handlers, and a blocked continuation.
- Execution: `run()` (unbounded unless fuel is set) and `run_slice(reductions)`,
  which returns at `Done`, `Failed`, `Cancelled`, `Yielded`, `Spawn`, `Await`,
  `WaitAny`, or `Io`.
- Process creation: about 8 µs and about 105 allocations per spawn (April 2026
  baseline, `cargo run -p hiko-vm --example process_creation_cost --release`).

### Efficiency gaps

- `examples/benchmark.hml` is about 2.2x slower than the equivalent Python
  script, mostly in system time (#81, #82). The opcode-density half of #82 is
  done. The filesystem half (per-file `canonicalize` plus `cap_std` checks) is
  not re-measured.
- `Resume` clones `saved_frames` and `saved_stack` on every call
  (`vm/dispatch.rs`, `Op::Resume`). Every resume costs time proportional to the
  captured stack depth.
- `HeapObject::String(String)` copies on transfer. #12 (`Arc<str>`) is open.
- Spawn cost is dominated by rebuilding builtin and global tables per child.
- There is no continuous benchmark baseline. `benchmarks/` has six classic
  programs (ack, fib via effects, nqueens, primes, tak) and `hiko-vm` has
  criterion benches, but results are not tracked over time.

## Security

### Implemented

| Layer                | What it does                                                                                                                                                                                                                                                           |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Two-tier gating      | A Cargo feature compiles a builtin in. A runtime policy (`VMBuilder` config, `*.policy.toml`) must also enable it. Disabled builtins are not callable.                                                                                                                 |
| Filesystem           | Per-builtin allowed `folders`. Access goes through `cap_std`, so paths cannot escape the granted directories.                                                                                                                                                          |
| HTTP                 | Per-builtin `allowed_hosts`.                                                                                                                                                                                                                                           |
| Exec                 | Explicit `allowed_commands`. No shell. Resolved paths are cached and re-validated.                                                                                                                                                                                     |
| Resource limits      | `max_fuel`/`max_work`, `max_heap`, `max_memory_bytes`, `max_host_work`, `max_io_bytes`, `max_host_resources` plus per-kind caps. Exhaustion is a controlled runtime error. See [`execution-bounds.md`](../architecture/execution-bounds.md).                           |
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
- Static checks: declared effects only, payload types match, continuation type
  is `effect-result -> handled-body-result`, handler arms agree on result
  type.
- Runtime: `Perform` captures frames and stack up to the nearest matching
  handler into a `Continuation`. It also saves that handler, and `Resume`
  reinstalls it. The handlers are therefore **deep**.
- `Resume` copies the continuation, so the same `k` can be resumed more than
  once (**multi-shot**). Verified: inside a function,
  `Choose _ k => resume k true + resume k false` returns 11 for
  `if perform Choose () then 1 else 10`.
- Runtime-handled effects (I/O, spawn, await) use a separate one-shot blocked
  continuation owned by the scheduler.

### Verified bug

Code after `resume` in a handler clause is lost when the `handle` is in the
top-level (main chunk) rather than inside a function:

```sml
effect Ask of unit

fun fa () = handle (perform Ask () + 1)
  with return x => x
     | Ask _ k => resume k 10 + 100
(* fa () = 111, correct *)

val top = handle (perform Ask () + 1)
  with return x => x
     | Ask _ k => resume k 10 + 100
(* top = 11, expected 111 *)
```

The same top-level shape with `resume k 1 + resume k 2` returns 2 instead
of 5. This needs a regression test and a fix in how `Resume` restores frames
when the handler belongs to the main chunk.

### Open research questions

These are the questions the research track should answer, in rough order:

1. **One-shot or multi-shot.** Local continuations are multi-shot by copy
   today, which is undocumented. Choose one. One-shot is cheaper (no clone) and
   matches OCaml 5. Multi-shot enables backtracking and is closer to Eff/Koka.
2. **Deep or shallow.** The implementation is deep. The README says shallow in
   two places. Decide and make docs, tests, and #29/#30 agree.
3. **Effect rows in types.** Function types do not record effects, so
   unhandled effects are runtime errors (#48, #74).
   [`effects.md`](../language/effects.md) lists strategies A to F.
4. **Effects and capabilities.** Whether host capabilities become effects
   handled by the runtime, so a policy is a top-level handler. This is the
   point where the research track meets the security focus.
5. **Resume cost.** A tail-resume path (#29) and avoiding the clone for
   one-shot use.
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
| Structured concurrency (`spawn`, `await`, `wait_any`, cancel) | Decision needed | Tied to the whitepaper thesis.                                                                       |

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
| `package.json`, `bun.lock`, `.pi/` | Out of focus    | pi coding agent setup. `.pi/agents/` and `.pi/graphs/` are untracked.                                                           |
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
| `README.md`                                              | Calls effect handlers "shallow" (lines 99 and 254). The implementation and `effects.md` say deep.                                        |
| `README.md`                                              | Builtin table lists `http_get : string -> string`. The real builtin returns a status, headers, and body tuple.                           |
| `docs/architecture/runtime.md`                           | Describes one-shot continuations for I/O only. Local continuations are multi-shot, which is not documented anywhere.                     |
| `docs/language/modules.md`                               | "What Is Next" lists a configurable cache location. Check against the current loader before keeping it.                                  |
| `docs/verification/verification-status-20260428.md`      | Six months old. Several items it lists as gaps (local-slot checks) are now done per `verification.md`.                                   |
| `docs/rnd/`, `docs/prompts/`, `docs/builtins/parquet.md` | Agent and proposal material. Out of focus.                                                                                               |
| `docs/index.md`                                          | Says "Review snapshot: 2026-05-26".                                                                                                      |

## Issue triage

| Issue                                        | Class         | Status                                                       |
| -------------------------------------------- | ------------- | ------------------------------------------------------------ |
| #80 Host resource accounting                 | Security      | Implemented in `c2ef46e`, `d3d0fe7`. Close.                  |
| #54, #55 Module cache, manifest, lockfile    | Security      | Implemented. Close or reduce to remaining items.             |
| #82 Filesystem I/O and opcode density        | VM            | Opcode half done. Re-measure filesystem half.                |
| #81 CLI run vs Python                        | VM            | Open.                                                        |
| #75 Performance predictability               | VM / Security | Open: benchmark harness, constant tuning, per-op I/O limits. |
| #12 `Arc<str>` strings                       | VM            | Open.                                                        |
| #40 Broadcast cloning                        | VM            | Open. Depends on keeping the threaded runtime.               |
| #47 Work-stealing scheduler                  | VM            | Open. Depends on keeping the threaded runtime.               |
| #25 Loom tests                               | Security      | Open. Depends on keeping the threaded runtime.               |
| #48, #74 Effect typing                       | Research      | Open.                                                        |
| #29, #30 Tail-resume, deep resume            | Research      | Open.                                                        |
| #10 Isolated-process runtime with effects    | Research      | Tracking issue.                                              |
| #59 Bundle modules and policy into artifacts | Security      | Open.                                                        |
| #53, #56, #57, #85 Harness and agent runtime | Out of focus  | Close or move with the harness.                              |

## Suggested cleanup order

1. Fix the tests so they do not need a local HTTP server.
2. Fix the top-level `resume` bug and add regression tests for deep and
   multi-shot behavior.
3. Close the finished issues (#80, #54, #55) and the out-of-focus ones.
4. Move `hiko-harness`, AWS, GitHub, Hashline, `.pi/`, `SYSTEM.md`,
   `package.json`, and their policies, examples, and libraries out of the
   repository.
5. Decide on the threaded runtime and structured concurrency.
6. Make `json`, `regex`, `hash`, and `date` non-default features.
7. Remove the unused tree-sitter dependency from `hiko-syntax`.
8. Fix the README inconsistencies listed above.
9. Add fuzz targets for the parser, type checker, verifier, and policy parser.
10. Start the effects research track with question 1 (one-shot or multi-shot).
