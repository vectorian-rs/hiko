# Hiko

A strict, statically typed, ML-family scripting language implemented in Rust with a bytecode VM.

Hiko's semantics are anchored in Core SML (Standard ML of New Jersey), with Hindley-Milner type inference, algebraic data types, exhaustive pattern matching, and OCaml 5-style algebraic effect handlers for structured concurrency.

## Quick Start

```bash
cargo build --release

# Run a program
cargo run -- run examples/factorial.hk

# Type-check without executing
cargo run -- check examples/closures.hk
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

datatype shape = Circle of Float
               | Rect of Float * Float

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
effect Yield of Int

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

**State effect** -- get/put pattern:

```sml
effect Get of Unit
effect Put of Int

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
val x : Int = 42

(* Monomorphic operators -- SML-97 style *)
val sum = 1 + 2          (* Int arithmetic: + - * / mod *)
val avg = 1.0 +. 2.0     (* Float arithmetic: +. -. *. /. *)
val msg = "a" ^ "b"      (* String concatenation: ^ *)
```

### Imports

```sml
use "stdlib/list.hk"
use "stdlib/option.hk"

val xs = map (fn x => x * 2) [1, 2, 3]
```

### Builtins

| Function | Type | Description |
|---|---|---|
| `print` / `println` | `'a -> Unit` | Output to stdout |
| `int_to_string` | `Int -> String` | Convert int to string |
| `float_to_string` | `Float -> String` | Convert float to string |
| `string_to_int` | `String -> Int` | Parse string to int |
| `string_length` | `String -> Int` | Character count |
| `substring` | `(String, Int, Int) -> String` | Extract substring |
| `split` | `(String, String) -> String list` | Split by delimiter |
| `trim` | `String -> String` | Trim whitespace |
| `read_file` / `write_file` | File I/O | Read/write file contents |
| `http_get` | `String -> String` | Synchronous HTTP GET |
| `assert` / `assert_eq` | Testing helpers | Runtime assertions |

## Architecture

```
Source (.hk)
  |
  v
Lexer ──> Tokens
  |
  v
Parser ──> Surface AST        (precedence-climbing recursive descent)
  |
  v
Desugar ──> Core AST           (list literals, andalso/orelse, not, paren)
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

**Parser** -- Hand-written precedence-climbing recursive descent. Each precedence level is a separate function (`parse_orelse` > `parse_andalso` > `parse_comparison` > `parse_cons` > `parse_addition` > `parse_multiplication` > `parse_app` > `parse_atom`).

**Desugaring** -- AST-to-AST pass that eliminates syntactic sugar before type-checking:
- `[1, 2, 3]` becomes nested `1 :: 2 :: 3 :: []`
- `andalso`/`orelse` become `if-then-else`
- `not e` becomes `if e then false else true`
- `(e)` unwrapped

**Type Inference** -- Algorithm W with the SML value restriction. Polymorphic types are generalized only at `val` bindings of syntactic values. Types are unified with occurs-check to prevent infinite types.

**Exhaustiveness Checking** -- Based on Maranget's "Warnings for pattern matching" (JFP 2007). Uses the usefulness algorithm: a match is exhaustive iff the wildcard pattern is not useful against the existing clauses. Reports both missing cases and redundant clauses.

**Bytecode Compiler** -- Direct walk of the typed AST producing 57 opcodes. Two-pass pattern compilation (test-then-bind) with tail-call optimization propagated through `if`/`let`/`case` branches.

**VM** -- Stack-based with `Vec<Value>` stack and `Vec<CallFrame>` frames. `Value` is `Copy` (no Rc, no Drop) -- 16 bytes, passed by value. Heap objects referenced via `GcRef(u32)` indices into the GC arena.

**Garbage Collector** -- Mark-and-sweep with worklist-based marking (avoids stack overflow on deep object graphs), free-list reuse, and adaptive collection threshold.

**Effect Handlers** -- Shallow, one-shot delimited continuations. `perform` captures the stack and frames between the perform site and the nearest matching handler. `resume` restores the captured continuation. Supports nested handlers, recursive handlers, and the generator/state patterns.

**Tail-Call Optimization** -- The `TailCall` opcode reuses the current call frame instead of pushing a new one. Propagated through `if`/`case`/`let` branches so tail-recursive functions run in constant stack space.

## Standard Library

- **`stdlib/list.hk`** -- `map`, `filter`, `foldl`, `foldr`, `length`, `reverse`, `append`, `nth`, `zip`, `take`, `drop`, `all`, `any`, `find`
- **`stdlib/option.hk`** -- `is_some`, `is_none`, `map_option`, `get_or`, `flat_map_option`
- **`stdlib/either.hk`** -- `map_right`, `map_left`, `is_left`, `is_right`, `from_left`, `from_right`

## Examples

The `examples/` directory includes programs demonstrating:

| File | Feature |
|---|---|
| `hello.hk` | Basic output |
| `factorial.hk` | Clausal function definitions |
| `fibonacci.hk` | Recursion |
| `closures.hk` | Higher-order functions, composition |
| `list_ops.hk` | Map, filter, fold over lists |
| `option.hk` | Algebraic data types |
| `either.hk` | Sum types for error handling |
| `expr_eval.hk` | Recursive expression evaluator |
| `math.hk` | Float arithmetic, sqrt |
| `string_ops.hk` | String manipulation builtins |
| `file_io.hk` | File read/write |
| `http_fetch.hk` | HTTP GET request |
| `import_test.hk` | Module imports |

## Testing

```bash
cargo test           # Run all 228 tests
cargo test -p hiko-vm    # VM tests only
cargo test -p hiko-types # Type inference tests only
```

Test coverage spans lexer, parser, type inference, exhaustiveness checking, bytecode compilation, VM execution, effect handlers, GC collection, and stdlib integration.

## License

MIT
