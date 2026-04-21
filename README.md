# Hiko

A strict, statically typed, ML-family scripting language implemented in Rust with a bytecode VM.

Hiko's semantics are anchored in Core SML (Standard ML), with Hindley-Milner type inference, algebraic data types, exhaustive pattern matching, and OCaml 5-style algebraic effect handlers for structured concurrency. Hiko is SML-derived, but it deliberately repairs or omits several historically messy parts of the SML specification; see [docs/sml-deltas.md](docs/sml-deltas.md).

## Quick Start

```bash
cargo build --release

# Run a program
cargo run -- run examples/factorial.hml

# Type-check without executing
cargo run -- check examples/closures.hml
```

## Language Overview

### Values and Bindings

```sml
val x = 42
val pi = 3.14159
val name = "Hiko"
val yes = true
val ch = #"A"
val pair = (1, 2)
val xs = [1, 2, 3]
val _ = println "hello"
```

### Functions

```sml
(* Named functions *)
fun double x = x * 2

(* Multi-argument (curried) *)
fun add a b = a + b

(* Anonymous functions *)
val inc = fn x => x + 1

(* Clausal definition with pattern matching *)
fun fact 0 = 1
  | fact n = n * fact (n - 1)

(* Mutual recursion *)
fun even 0 = true
  | even n = odd (n - 1)
and odd 0 = false
  | odd n = even (n - 1)
```

### Algebraic Data Types

```sml
datatype 'a option = None | Some of 'a

datatype shape = Circle of float
               | Rect of float * float

fun area s = case s of
    Circle r => 3.14159 *. r *. r
  | Rect (w, h) => w *. h
```

### Pattern Matching

```sml
fun describe xs = case xs of
    []          => "empty"
  | [x]         => "singleton"
  | x :: y :: _ => "two or more"

(* Exhaustiveness and redundancy checked at compile time *)
```

### Algebraic Effect Handlers

OCaml 5 / Eio-inspired shallow effect handlers with delimited continuations:

```sml
(* Declare an effect *)
effect Yield of int

(* Generator: yields values *)
fun gen () =
  let val _ = perform Yield 1
      val _ = perform Yield 2
      val _ = perform Yield 3
  in () end

(* Handler: collects yielded values into a sum *)
fun run_gen f = handle f ()
  with return _ => 0
     | Yield n k => n + run_gen (fn _ => resume k ())

val result = run_gen gen   (* result = 6 *)
```

**State effect** (get/put pattern):

```sml
effect Get of unit
effect Put of int

fun run_state init f =
  handle f ()
  with return x => x
     | Get _ k => run_state init (fn _ => resume k init)
     | Put n k => run_state n (fn _ => resume k ())

val result = run_state 0 (fn _ =>
  let val _ = perform Put 42
  in perform Get () end)       (* result = 42 *)
```

### Type System

Hindley-Milner with the value restriction:

```sml
(* Types are inferred *)
fun compose f g = fn x => f (g x)
(* compose : ('a -> 'b) -> ('c -> 'a) -> 'c -> 'b *)

(* Type annotations supported *)
val x : int = 42

(* Monomorphic operators, SML-97 style *)
val sum = 1 + 2          (* int arithmetic: + - * / mod *)
val avg = 1.0 +. 2.0     (* float arithmetic: +. -. *. /. *)
val msg = "a" ^ "b"      (* string concatenation: ^ *)
```

### Imports

```sml
use "libraries/Std-v0.1.0/modules/List.hml"
use "libraries/Std-v0.1.0/modules/Option.hml"

val xs = List.map (fn x => x * 2) [1, 2, 3]
```

### Pipelines

```sml
fun lines s = split (s, "\n")
fun count xs = List.length xs

val fn_count =
  "examples/pipeline.hml"
  |> read_file
  |> lines
  |> List.filter (fn line => regex_match (line, "^fun "))
  |> count
```

`|>` is left-associative and desugars to ordinary application, so `x |> f` means `f x`.

### Builtins

| Function | Type | Description |
|---|---|---|
| `print` / `println` | `'a -> unit` | Output to stdout |
| `int_to_string` | `int -> string` | Convert int to string |
| `float_to_string` | `float -> string` | Convert float to string |
| `string_to_int` | `string -> int` | Parse string to int |
| `string_length` | `string -> int` | Character count |
| `substring` | `(string, int, int) -> string` | Extract substring |
| `split` | `(string, string) -> string list` | Split by delimiter |
| `trim` | `string -> string` | Trim whitespace |
| `read_file` / `write_file` | File I/O | Read/write file contents |
| `http_get` | `string -> string` | Synchronous HTTP GET |
| `assert` / `assert_eq` | Testing helpers | Runtime assertions |

## Architecture

```
Source (.hml)
  |
  v
Lexer ──> Tokens
  |
  v
Parser ──> Surface AST        (precedence-climbing recursive descent)
  |
  v
Desugar ──> Core AST           (list literals, andalso/orelse, not, |>, paren)
  |
  v
Type Inference ──> Typed AST   (Algorithm W, Hindley-Milner)
  |
  v
Exhaustiveness ──> Warnings    (Maranget usefulness algorithm)
  |
  v
Compiler ──> Bytecode          (57 opcodes, two-pass pattern compilation)
  |
  v
VM ──> Execution               (stack-based, mark-and-sweep GC)
```

### Crate Structure

| Crate | Description |
|---|---|
| `hiko-syntax` | Lexer, parser, AST, pretty-printer, desugaring pass |
| `hiko-types` | HM type inference, exhaustiveness/redundancy checking |
| `hiko-compile` | Bytecode compiler, opcode definitions, chunk format |
| `hiko-vm` | Stack-based VM, mark-and-sweep GC, builtins |
| `hiko-cli` | CLI entry point (`run` and `check` commands) |

### Key Implementation Details

**Parser.** Hand-written precedence-climbing recursive descent. Each precedence level is a separate function (`parse_pipe` > `parse_orelse` > `parse_andalso` > `parse_comparison` > `parse_cons` > `parse_addition` > `parse_multiplication` > `parse_app` > `parse_atom`), from loosest to tightest binding.

**Desugaring.** AST-to-AST pass that eliminates syntactic sugar before type-checking:
- `[1, 2, 3]` becomes nested `1 :: 2 :: 3 :: []`
- `andalso`/`orelse` become `if-then-else`
- `not e` becomes `if e then false else true`
- `x |> f` becomes `f x`
- `(e)` unwrapped

**Type Inference.** Algorithm W with the SML value restriction. Polymorphic types are generalized only at `val` bindings of syntactic values. Types are unified with occurs-check to prevent infinite types.

**Exhaustiveness Checking.** Based on Maranget's "Warnings for pattern matching" (JFP 2007). Uses the usefulness algorithm: a match is exhaustive iff the wildcard pattern is not useful against the existing clauses. Reports both missing cases and redundant clauses.

**Bytecode Compiler.** Direct walk of the typed AST producing 57 opcodes. Two-pass pattern compilation (test-then-bind) with tail-call optimization propagated through `if`/`let`/`case` branches.

**VM.** Stack-based with `Vec<Value>` stack and `Vec<CallFrame>` frames. `Value` is `Copy` (no Rc, no Drop), 16 bytes, passed by value. Heap objects referenced via `GcRef(u32)` indices into the GC arena.

**Garbage Collector.** Mark-and-sweep with worklist-based marking (avoids stack overflow on deep object graphs), free-list reuse, and adaptive collection threshold.

**Effect Handlers.** Shallow, one-shot delimited continuations. `perform` captures the stack and frames between the perform site and the nearest matching handler. `resume` restores the captured continuation. Supports nested handlers, recursive handlers, and the generator/state patterns.

**Tail-Call Optimization.** The `TailCall` opcode reuses the current call frame instead of pushing a new one. Propagated through `if`/`case`/`let` branches so tail-recursive functions run in constant stack space.

**Runtime Limits.** Heap size and fuel are configurable through `VMBuilder` or run config files. The VM also has fixed hard limits of `hiko_vm::DEFAULT_MAX_STACK_SLOTS` (`65536` value-stack slots) and `hiko_vm::DEFAULT_MAX_CALL_FRAMES` (`65536` call frames). These are current runtime guards, not config knobs.

## Standard Library

- **`libraries/Std-v0.1.0/modules/List.hml`**: `map`, `filter`, `foldl`, `foldr`, `length`, `reverse`, `append`, `nth`, `zip`, `take`, `drop`, `all`, `any`, `find`
- **`libraries/Std-v0.1.0/modules/Option.hml`**: `is_some`, `is_none`, `map_option`, `get_or`, `flat_map_option`
- **`libraries/Std-v0.1.0/modules/Either.hml`**: `map_right`, `map_left`, `is_left`, `is_right`, `from_left`, `from_right`
- **`libraries/Std-v0.1.0/modules/Result.hml`**: `Ok` / `Err`, `map`, `map_err`, `and_then`, `flatten`, `fold`, `or_else`, `unwrap_or`
- **`libraries/Std-v0.1.0/modules/Fiber.hml`**: `spawn`, `join`, `cancel`, `both`, `all`, `first`, `any`

## Examples

The `examples/` directory includes programs demonstrating:

| File | Feature |
|---|---|
| `hello.hml` | Basic output |
| `factorial.hml` | Clausal function definitions |
| `fibonacci.hml` | Recursion |
| `closures.hml` | Higher-order functions, composition |
| `list_ops.hml` | Map, filter, fold over lists |
| `option.hml` | Algebraic data types |
| `either.hml` | Sum types for error handling |
| `result.hml` | Canonical success/error datatype |
| `error_handling.hml` | Error handling via algebraic effects |
| `spawn_stress.hml` | Isolated-process concurrency stress test |
| `expr_eval.hml` | Recursive expression evaluator |
| `math.hml` | float arithmetic, sqrt |
| `string_ops.hml` | string manipulation builtins |
| `file_io.hml` | File read/write |
| `http_fetch.hml` | HTTP GET request |
| `import_test.hml` | Module imports |

## Agent Workflow

Hiko is designed for building sandboxed agent scripts with least-privilege run configs. The typical setup uses an AI agent as the orchestrator, [mise](https://mise.jdx.dev/) as the task runner, and hiko as the execution engine.

```
Agent (Claude, etc.)
  └── mise run analyze        → ./dist/data-reader analyze.hml
  └── mise run deploy         → ./dist/infra-prod-deploy deploy.hml
  └── mise run notify         → ./dist/slack-notifier notify.hml
```

### Setup

```bash
# Install hiko
cargo install hiko-cli

# Write a run config — defines what the VM can do
cat > reader.toml << 'EOF'
[limits]
max_fuel = 10_000_000
max_heap = 500_000

[capabilities.stdio.println]
enabled = true

[capabilities.filesystem.read_file]
enabled = true
folders = ["."]
EOF

# Generate a config-locked VM binary
hiko build-vm reader.toml
cd hiko-vm-reader && cargo build --release

# Run scripts with it
./target/release/hiko-vm-reader my_script.hml
```

### Run Config Files

Each run config defines a capability boundary.
- `hiko run --config ...` loads the config at runtime and only registers the allowed builtins for that invocation.
- `hiko build-vm ...` bakes the same config into a generated binary, so disallowed builtins are not compiled into that artifact.

Path rules:
- relative `entry` and filesystem `folders` are resolved from the current working directory
- absolute paths are allowed
- `..` is rejected in run config paths; the intended workflow is to run from the repo root and use `./...` paths
- direct CLI script arguments are separate from run config paths and may still use `..` for one-off invocations

```toml
# infra-prod-deploy.toml
[limits]
max_fuel = 50_000_000
max_heap = 1_000_000

[capabilities.stdio.println]
enabled = true

[capabilities.filesystem.read_file]
enabled = true
folders = ["/deploy"]

[capabilities.filesystem.write_file]
enabled = true
folders = ["/deploy"]

[capabilities.http.http]
enabled = true
allowed_hosts = ["deploy.internal.example.com"]

[capabilities.system.exit]
enabled = true
```

`hiko build-vm` reads the run config TOML once and generates a standalone Rust crate (`hiko-vm-{config-name}/`) with the config compiled into `src/main.rs` as hardcoded VMBuilder calls. The generated binary has no runtime config files — the config is the code.

```
configs/reader.toml             ← human writes this
    │
    ▼  hiko build-vm
hiko-vm-reader/
├── Cargo.toml                  ← pulls hiko crates from crates.io
└── src/main.rs                 ← config baked in as Rust code
    │
    ▼  cargo build --release
hiko-vm-reader/target/release/  ← single binary, config-locked
```

The generated `src/main.rs` is the auditable source of truth for what the VM can do. Commit it alongside your run config TOML — reviewers can see exactly which builtins are registered, which commands are whitelisted, and what limits are enforced, without running any tools. The `target/` directory is gitignored.

### Orchestration with mise

[mise](https://mise.jdx.dev/) provides a self-documenting task layer so anyone on the team can see what the project can do with `mise tasks`.

```toml
# mise.toml
[tasks.build]
description = "Build a config-locked VM binary"
run = """
set -e
config="configs/${1}.toml"
hiko build-vm "$config"
cargo build --release --manifest-path "hiko-vm-${1}/Cargo.toml"
mkdir -p dist
cp "hiko-vm-${1}/target/release/hiko-vm-${1}" "dist/${1}"
"""

[tasks.run]
description = "Run an agent script with its config-locked VM"
run = """
agent="$1"; shift; script="$1"; shift
"dist/${agent}" "$script" "$@"
"""

[tasks.dev]
description = "Run a script with the full CLI (core only unless --config is provided)"
run = "hiko run \"$@\""

[tasks.check]
description = "Type-check a script without running it"
run = "hiko check \"$@\""

[tasks.build-all]
description = "Build all config VMs"
run = """
for config in configs/*.toml; do
  name=$(basename "$config" .toml)
  mise run build "$name"
done
"""
```

```bash
mise run build reader          # Build the reader VM
mise run run reader analyze.hml # Run a script sandboxed
mise run dev analyze.hml        # Quick iteration, core only by default
mise run check analyze.hml      # Type-check only
mise run build-all              # Build all config VMs
```

## Testing

```bash
cargo test           # Run all 228 tests
cargo test -p hiko-vm    # VM tests only
cargo test -p hiko-types # Type inference tests only
```

Test coverage spans lexer, parser, type inference, exhaustiveness checking, bytecode compilation, VM execution, effect handlers, GC collection, and stdlib integration.

## License

MIT
