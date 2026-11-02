# Algebraic Effects

Hiko has local algebraic effects: `effect`, `perform`, `handle`, and `resume`.
Effects are a small, isolated research feature, not part of the core language
(see [project-state.md](../project/project-state.md)). Design proposals and
comparisons with other languages live in
[rnd/effects-design.md](../rnd/effects-design.md).

A handler clause has one of two forms:

- **Provider:** the clause answers the request (`| Op x k => resume k answer`).
  The code that performs the request does not know which provider is
  installed, so a test can install a mock.
- **Abort:** the clause does not use `k`. Its value becomes the value of the
  whole `handle` expression (early exit).

Other things are not effects:

- Recoverable errors use `Std.Result`. See
  [error-handling.md](error-handling.md).
- I/O uses builtins.
- Generators and state use closures and ordinary arguments. See
  [Generators and State Without Effects](#generators-and-state-without-effects).

## Mental Model

An effect operation is a named request made by ordinary code and answered by
surrounding code.

```hiko
effect AskName of unit

fun greet () =
  "hello " ^ perform AskName ()

val message =
  handle greet ()
  with return x => x
     | AskName _ k => resume k "alice"
```

`perform AskName ()` sends a request to the nearest matching handler. The
clause receives the payload, here `()`, and the name `k` for the request.
`resume k "alice"` answers it: `perform AskName ()` returns `"alice"` and
`greet` continues. `k` is not a value. It can only be used as `resume k e` at
the end of a clause.

## Syntax

An effect declaration states the payload type:

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

`effect Fail of string` means that the payload of `Fail` is a `string`. It does
**not** say what `perform Fail ...` returns. An effect operation has two types:

```text
operation : payload_type -> result_type
```

The result type is inferred from the `perform` sites and the `resume`
arguments.

Both types are monomorphic. The payload type cannot contain type variables, and
the result type is one type shared by every `perform` and handler of that
effect in the program. A function that performs an effect is therefore not
generalized over the effect's result type.

## Provider Clauses

A clause is a provider when every path through its body ends in `resume k e`.
The paths go through `let` bodies, both branches of `if`, and every branch of
`case`.

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

The rules:

- **The clause runs at the perform site.** Its value `e` becomes the result of
  `perform`. Nothing is copied or allocated per request, and a loop of
  requests runs in constant stack.
- **The handler stays installed.** Every later `perform` in the handled code
  reaches the same provider.
- **The clause runs outside its handler.** While it runs, its handler and every
  handler installed inside it are hidden, so a `perform` in the clause goes to
  an outer handler:

  ```hiko
  effect Ask of int

  val result =
    handle
      (handle perform Ask 1
       with return x => x
          | Ask n k => resume k (perform Ask (n + 1) * 10))
    with return x => x
       | Ask n k => resume k (n + 100)

  (* result = 1020: the inner clause asks the outer handler for 2 *)
  ```

- **Handlers installed inside stay in place.** A handler between the provider
  and the `perform` site still handles its effects after the provider answers:

  ```hiko
  effect Ask of int
  effect Fail of int

  fun body () =
    let val a = perform Ask 1 val _ = perform Fail (a + 5) in 1000 end

  val result =
    handle 1 + (handle body () with return x => x | Fail n _ => n + 100)
    with return x => x | Ask n k => resume k (n * 10)

  (* result = 116: Ask is answered with 10, the inner handler catches Fail 15 *)
  ```

`examples/http_effect_wrapper_test.hml` runs the same code against a mock HTTP
provider and a real one. `Std.Filesystem` is a provider over its
builtins.

## Abort Clauses

Any clause that is not a provider aborts. The clause must not use `k`.
`perform` unwinds the stack, call frames and handlers back to the handler,
then runs the clause in the frame that installed the handler. The clause's
value is the value of the `handle` expression.

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

## What Is Rejected

`k` is not a value. The compiler rejects, with the source location:

- reading `k` anywhere except as the first operand of `resume`: storing it,
  passing it to a function, or capturing it in a closure;
- `resume` that is not at the end of the clause, as in `1 + resume k n` or
  `resume k (resume k n)`;
- a clause where only some paths end in `resume k e`;
- `resume` inside a nested `handle` or `fn` in the clause.

```text
error: `resume k e` is supported only at the end of a handler clause, with
every path of the clause ending in `resume k e`
```

A clause can bind a new variable named `k`; that variable is an ordinary value.

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

Effect-based generators and state, which wrap `resume` in a new handler call,
do not compile.

## Static Guarantees

The typechecker guarantees:

- `perform Effect payload` refers to a declared effect.
- The `perform` payload type matches the effect declaration.
- The result type of a `perform` is the type of the `resume` arguments in that
  effect's clauses.
- Handler clauses refer to declared effects.
- Handler payload variables have the declared effect payload type.
- `resume k value` typechecks like applying `k`, of type
  `result_type -> handled-body-type`, to `value`.
- Handler clause bodies have the same type as the handler `return` arm.
- The `return` arm parameter has the type of the normally returned handled body.

The compiler then enforces the clause forms above.

## Non-Guarantees

Hiko does not put effect sets in function types. A function that may perform
`Aws.S3.list_buckets` does not have a type that records that fact. Hiko has no
function type like:

```text
Aws.Config -> Result Error Report ! { Clock.now, Aws.S3.list_buckets }
```

This means:

- unhandled effects are not rejected statically;
- function signatures do not advertise which effects they may request;
- the typechecker does not prove that every call site has a handler for every
  effect it may perform.

Unhandled effects are controlled runtime errors, not host panics.

The bytecode verifier checks the structural pieces visible after compilation:

- `Perform` and `InstallHandler` reference declared effect tags;
- `Perform` has enough stack depth for a payload;
- a provider clause's closure slot is below the stack depth at the
  `InstallHandler`;
- abort clause targets are instruction boundaries with valid stack depth;
- handler clause flags use only the defined `provider` bit.

The verifier does not prove the source-level protocol for hand-written
bytecode, such as balanced install and remove. The VM rejects a handler whose
frame has returned, or a provider slot that is not a closure, with a controlled
error.

## Implementation

- **Provider clause.** It compiles to a closure taking the payload and a `Unit`
  slot for `k`. The closure is kept on the stack as a hidden local of the
  function that installs the handler, so it lives as long as the handler.
  `InstallHandler` records its slot.
- **Perform on a provider.** `perform` moves the handler and the handlers
  inside it to a hidden list, then calls the closure. When the clause frame
  returns, `Return` reinstalls them. An abort that unwinds past the clause
  frame discards them.
- **Perform on an abort clause.** `perform` truncates the stack and call frames
  to the handler and jumps to the clause body in the handler's frame.

The code is in `ExprKind::Handle` and `ExprKind::Resume` in
`crates/hiko-compile/src/compiler.rs`, and in `Op::InstallHandler`,
`Op::Perform` and `Op::Return` in `crates/hiko-vm/src/vm/dispatch.rs`.

The threaded runtime's I/O suspensions are a separate mechanism. They do not
use effect handlers (see [`../architecture/runtime.md`](../architecture/runtime.md)).
