# Algebraic Effects

Hiko has local algebraic effects: `effect`, `perform`, `handle`, and `resume`.
This page describes what is implemented today. Effects are the research track
of the language (see [project-state.md](../project/project-state.md)); design
proposals and comparisons with other languages live in
[rnd/effects-design.md](../rnd/effects-design.md).

Recoverable errors use `Std.Result`, not effects. See
[error-handling.md](error-handling.md).

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
effect Get of unit
effect Put of int
effect Fail of string
```

Perform an effect:

```hiko
val current = perform Get ()
val _ = perform Put (current + 1)
```

Handle effects:

```hiko
fun run_state init f = handle
    f ()
  with
    return x => x
  | Get _ k => run_state init (fn _ => resume k init)
  | Put n k => run_state n (fn _ => resume k ())
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

## Undecided Behavior

These are implemented but not yet settled, and may change:

- Continuations are **deep**: the handler stays installed when the
  continuation resumes.
- A continuation can be resumed more than once, because `Resume` copies the
  captured frames and stack. Whether Hiko keeps multi-shot resumption or
  restricts continuations to one-shot is an open research question.

See the open research questions in
[project-state.md](../project/project-state.md#open-research-questions).
