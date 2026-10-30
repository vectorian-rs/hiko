# Algebraic Effects

Hiko has local algebraic effects: `effect`, `perform`, `handle`, and `resume`.
This page describes what is implemented today. Effects are the research track
of the language (see [project-state.md](../project/project-state.md)); design
proposals and comparisons with other languages live in
[rnd/effects-design.md](../rnd/effects-design.md).

Effects are a small, isolated research feature, not part of the core
language. Two uses are supported:

- **Provider:** a handler answers a request and resumes at once
  (`| Op x k => resume k answer`). The code that performs the request does not
  know which provider is installed, so a test can install a mock. See
  [Supported Uses](#supported-uses).
- **Abort:** a handler ignores the continuation and returns a value for the
  whole `handle` expression (early exit).

Other things are not effects:

- Recoverable errors use `Std.Result`. See
  [error-handling.md](error-handling.md).
- I/O uses builtins.
- Generators and state use closures and ordinary arguments. See
  [Generators and State Without Effects](#generators-and-state-without-effects).

Effects can do more than these two uses today: a handler can store `k`,
resume it later, resume it twice, or resume it inside another handler. That
behavior is described under
[Current Continuation Semantics](#current-continuation-semantics), but it is
not supported. The plan is to remove first-class continuations and keep only
the two forms above (PR #121).

## Mental Model

An algebraic effect operation is a named request made by ordinary code and
interpreted by surrounding code.

```hiko
effect AskName of unit

fun greet () =
  "hello " ^ perform AskName ()

val message =
  handle greet ()
  with return x => x
     | AskName _ k => resume k "alice"
```

`perform AskName ()` pauses the current computation and sends a request to the
nearest matching handler. The handler receives:

- the request payload, here `()`;
- a continuation `k`, representing the rest of the suspended computation.

`resume k "alice"` continues from the `perform` site, using `"alice"` as the
result of `perform AskName ()`.

## Current Hiko Syntax

Current Hiko declares only the payload side of an effect:

```hiko
effect Ask of unit
effect Log of string
effect Fail of string
```

Perform an effect:

```hiko
val limit = perform Ask ()
val _ = perform Log ("limit is " ^ int_to_string limit)
```

Handle effects:

```hiko
fun with_settings limit f = handle
    f ()
  with
    return x => x
  | Ask _ k => resume k limit
  | Log msg k => let val _ = println msg in resume k () end
```

In current syntax, `effect Fail of string` means:

```text
Fail's payload type is string
```

It does **not** mean:

```text
perform Fail ... always returns string
```

There are two types involved in an effect operation:

```text
operation : payload_type -> resume_result_type
```

The current `of` syntax states the payload type. The resume/result type is
inferred from use and handlers.

Both types are monomorphic. The payload type cannot contain type variables, and
the resume/result type is one type shared by every `perform` and handler of
that effect in the program. A function that performs an effect is therefore not
generalized over the effect's result type.

## Supported Uses

### Provider

The handler answers each request and resumes at once. Because `resume`
reinstalls the handler, the same provider also answers every later request
from the resumed code.

```hiko
effect Log of string

fun with_console_log f =
  handle
    f ()
  with
    return x => x
  | Log msg k => let
      val _ = println ("[log] " ^ msg)
    in
      resume k ()
    end
```

`examples/http_effect_wrapper_test.hml` runs the same code against a mock HTTP
provider and a real one. `Std.Filesystem` and `Std.Hashline` are written as
providers over their builtins.

### Abort

The clause does not use `k`, so the computation stops and the clause's value
becomes the value of the `handle` expression. The VM unwinds without capturing
a continuation (see [Abortive Handler Clauses](#abortive-handler-clauses)).

```hiko
effect Found of int

fun scan xs =
  case xs of
      [] => ()
    | x :: rest => if x < 0 then perform Found x else scan rest

fun first_negative xs =
  handle
    let val _ = scan xs in 0 end
  with
    return x => x
  | Found n _ => n

(* first_negative [1, ~2, ~3] = ~2 *)
```

## Generators and State Without Effects

A generator is a function that returns the next item and a function that
produces the rest. It needs only a datatype and closures, it can be infinite,
and it runs in constant stack:

```hiko
datatype 'a stream = Done | Next of 'a * (unit -> 'a stream)

fun range lo hi () = if lo > hi then Done else Next (lo, range (lo + 1) hi)

fun take n s = if n = 0 then [] else case s () of
      Done => []
    | Next (x, rest) => x :: take (n - 1) rest

(* take 3 (range 1 10) = [1, 2, 3] *)
```

State is an argument that a function takes and returns, usually as an
accumulator or a `(result, new_state)` pair. See `examples/generator.hml` and
`examples/state.hml`.

The effect-based versions (`perform Yield` with a handler that re-handles
around `resume`, and `Get`/`Put` handlers) are not supported. They depend on
the unsupported continuation behavior below. They are also slower: summing a
100,000-item effect generator overflowed the stack after 25 seconds in a debug
build, while the closure version finished in 1.6 seconds.

## Current Static Guarantees

The typechecker currently guarantees:

- `perform Effect payload` refers to a declared effect.
- `perform` payload type matches the effect declaration.
- The result type of a `perform` is the type expected by resumptions of that
  effect in handlers.
- Handler clauses refer to declared effects.
- Handler payload variables have the declared effect payload type.
- Handler continuation variables have type:

  ```text
  effect-result-type -> handled-body-result-type
  ```

  The continuation resumes the interrupted body, not the handler's `return` arm
  directly. This supports generator-style handlers that wrap `resume` in another
  handler invocation.

- `resume k value` typechecks like applying `k` to `value`.
- Handler clause bodies must return the same type as the handler `return` arm.
- The `return` arm parameter has the type of the normally returned handled body.

These checks catch common protocol errors: wrong payload types, undeclared
effects, resuming with the wrong value type, and handler branches returning
incompatible values.

## Current Non-Guarantees

Hiko does not yet put effect sets in function types. A function that may perform
`Aws.S3.list_buckets` currently does not have a type that records that fact.

For example, Hiko currently has no function type like:

```text
Aws.Config -> Result Error Report ! { Clock.now, Aws.S3.list_buckets }
```

This means:

- escaping/unhandled effects are not statically rejected by effect rows;
- function signatures do not advertise which effects they may request;
- the typechecker does not prove every call site has installed or been granted
  all required handlers.

Unhandled effects are controlled runtime errors, not host panics.

The bytecode verifier checks structural protocol pieces visible after
compilation:

- `Perform` and `InstallHandler` reference declared effect tags;
- `Perform` has enough stack depth for a payload;
- `Resume` has enough stack depth for a continuation and argument;
- handler clause targets are instruction boundaries with valid stack depth;
- handler clause flags use only the defined `resumes` bit.

The verifier still does not prove the full high-level source effect protocol for
arbitrary hand-written bytecode. Runtime checks must continue to reject invalid
continuation values and malformed resume attempts with controlled errors.

## Abortive Handler Clauses

A clause whose body never refers to its continuation, for example
`Fail msg _ => Err msg`, cannot resume. The compiler marks each clause with a
`resumes` flag, set only when the clause body refers to the continuation
variable (a shadowed `k` does not count). For a clause without the flag,
`perform` unwinds the stack, call frames and handlers back to the handler and
runs the clause directly. It does not copy the frames and stack between the
`perform` site and the handler, and it does not allocate a continuation. The
continuation slot of such a clause holds a placeholder that the clause never
reads.

## Current Continuation Semantics

This section describes what the VM does today. It is implemented, but it is
not a stable guarantee. Only the provider and abort forms in
[Supported Uses](#supported-uses) are supported. The rest of this section
describes behavior that PR #121 plans to remove.

**Handlers are deep when the clause resumes directly.** `perform` removes the
matching handler, and every handler installed inside it, and stores them in the
continuation (`ContinuationData::saved_handlers` in
`crates/hiko-vm/src/value.rs`). When a clause calls `resume k v` directly,
`resume` reinstalls all of them. A second `perform` of the same effect inside
the resumed computation therefore reaches the same handler:

```hiko
effect Tick of unit

fun two () =
  let
    val _ = perform Tick ()
    val _ = perform Tick ()
  in
    0
  end

fun count () =
  handle
    two ()
  with
    return x => x
  | Tick _ k => 1 + resume k ()

(* count () = 2 *)
```

**Inner handlers are part of the continuation.** A handler installed between
the catching handler and the `perform` site is always reinstalled by `resume`,
so effects it handles still reach it after the resume:

```hiko
effect Ask of int
effect Fail of int

fun body () =
  let val a = perform Ask 1 val _ = perform Fail (a + 5) in 1000 end

val result =
  handle 1 + (handle body () with return x => x | Fail n _ => n + 100)
  with return x => x | Ask n k => resume k (n * 10)

(* result = 116: Ask resumes with 10, the inner handler catches Fail 15 *)
```

**Exception: resuming under a new handler is shallow.** If the clause resumes
inside a handler it installed itself, as in the generator pattern below, the
catching handler is not reinstalled; the new handler takes its place.
`resume` reinstalls the catching handler only when fewer handlers are
installed at the resume point than when the effect was caught
(`ContinuationData::catching_handler_depth`). Inner handlers are reinstalled
in both cases.

```hiko
effect Yield of int

fun run_gen f =
  handle f ()
  with return _ => 0
     | Yield n k => n + run_gen (fn _ => resume k ())

fun gen () =
  let val _ = perform Yield 1 val _ = perform Yield 2 in () end

(* run_gen gen = 3. With fully deep handlers (as in OCaml or Koka) the
   original handler would also be reinstalled, catch Yield 2, and the result
   would be 1. *)
```

This mixed rule is what the VM does today; it is not a designed semantics.
Choosing between deep, shallow, or an explicit choice per handler is part of
the open research questions below.

**A continuation runs to the end of its `handle` expression.** The captured
continuation starts at the `perform` site and ends where the `handle`
expression ends. After the resumed computation finishes, control returns to
the handler clause that called `resume`. This holds for a `handle` at the top
level of a script and for a `handle` nested inside a larger expression, such
as `(handle ... ) + 100`. The compiler puts a `handle` that is not in function
tail position into its own zero-argument function, so the `handle` always ends
its own call frame (`ExprKind::Handle` in `crates/hiko-compile/src/compiler.rs`).

**Local continuations are multi-shot by copy.** `resume` copies the saved stack
slice and frames back onto the VM stack and does not consume the continuation.
The same `k` can be resumed more than once:

```hiko
effect Choose of unit

fun body () = if perform Choose () then 10 else 20

fun both () =
  handle
    body ()
  with
    return x => x
  | Choose _ k => resume k true + resume k false

(* both () = 30 *)
```

**Direction.** First-class continuations are planned to go away (PR #121).
A provider clause will run in place at the `perform` site, and an abort clause
will unwind as it does now. Storing `k`, resuming it twice, or resuming it
inside another handler will no longer be possible, so do not rely on them. The
threaded runtime's I/O suspensions are a separate mechanism and are always
one-shot (see [`../architecture/runtime.md`](../architecture/runtime.md)).
