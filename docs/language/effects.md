# Algebraic Effects

Hiko currently has local algebraic effects with `effect`, `perform`, `handle`,
and `resume`. The current implementation is useful, but the intended design
direction is stricter than "exceptions with nicer syntax": Hiko should treat
algebraic effects as **explicit, typed capability requests**. Recoverable errors
should remain ordinary `Std.Result` values.

This page documents three things:

1. what algebraic effects are for;
2. Hiko's current implementation and non-guarantees;
3. a proposed direction for syntax/type-system evolution, with comparisons to
   OCaml, Haskell, Koka, Eff, Frank, Links, and related designs.

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

This can encode exception-like aborts, but that should not be Hiko's primary
style. Effects are more useful as capability requests:

```text
Clock.now
Random.bytes
Filesystem.read_text
Http.Client.send
Aws.S3.list_buckets
```

The handler or VM policy decides how those requests are fulfilled.

## Preferred Design Principle

Do not use effects as the primary recoverable-error mechanism.

Use:

```text
Std.Result
```

for ordinary domain/provider failures, and use effects to say that code requires
an external capability or interpreter.

For example, prefer a capability operation that returns a value-level result:

```hiko
-- Proposed syntax, not current syntax:
effect Aws.S3.list_buckets : Aws.Config -> Result Aws.S3.Error (List Aws.S3.Bucket)
```

rather than an exception-like failure operation:

```hiko
-- Discouraged as general application style:
effect Fail of string
```

The effect says "this code needs S3". The `Result` says "the S3 request may
succeed or fail". This keeps failure in normal data flow and avoids hidden
non-local exits.

See also [`error-handling.md`](error-handling.md).

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
- handler clause targets are instruction boundaries with valid stack depth.

The verifier still does not prove the full high-level source effect protocol for
arbitrary hand-written bytecode. Runtime checks must continue to reject invalid
continuation values and malformed resume attempts with controlled errors.

## Proposed Hiko Direction

The preferred direction is:

```text
effects = explicit typed capability requests
Result  = recoverable provider/domain failure
effect rows = visible operation requirements
handlers/providers = interpreter or policy boundaries
```

### 1. Explicit Operation Result Types

Move from payload-only declarations:

```hiko
effect Ask of unit
```

toward operation signatures:

```hiko
-- Proposed syntax:
effect Ask : unit -> int
effect Log : string -> unit
effect Clock.now : unit -> Instant
effect Random.bytes : int -> Result Random.Error bytes
effect Http.Client.send : Http.Request -> Result Http.Error Http.Response
effect Aws.S3.list_buckets : Aws.Config -> Result Aws.S3.Error (List Aws.S3.Bucket)
```

This makes the operation's request and response shape explicit:

```text
EffectName : payload_type -> result_type
```

`perform Ask ()` would have type `int`. `perform Log "x"` would have type
`unit`. `perform Aws.S3.list_buckets cfg` would have a `Result` type.

### 2. Capability-Oriented Request Syntax

`perform` is standard algebraic-effects terminology. It is accurate, but it can
sound exception/control-flow oriented. Hiko may want a capability-oriented alias
or replacement:

```hiko
-- Current style:
val now = perform Clock.now ()

-- Possible future style:
val now = request Clock.now ()
```

`request` reads as "ask the current interpreter/policy for this capability".
That is closer to Hiko's security model.

### 3. Effect Rows in Function Types

Function types should eventually expose which operations may be requested.

Potential displayed/inferred type:

```text
Aws.Config -> Result Report.Error Report ! { Clock.now, Aws.S3.list_buckets }
```

Potential source annotation:

```hiko
fun generate_report cfg
  : Aws.Config -> Result Report.Error Report
    ! { Clock.now, Aws.S3.list_buckets } =
  ...
```

A more readable source-level spelling could be:

```hiko
fun generate_report cfg
  requires { Clock.now, Aws.S3.list_buckets } =
  ...
```

The type printer can still display the compact form:

```text
generate_report : Aws.Config -> Result Report.Error Report
                ! { Clock.now, Aws.S3.list_buckets }
```

An effect-free function can omit the row or be shown as `! {}`.

### 4. Provider-Style Handler Sugar

The full handler form exposes continuations:

```hiko
handle program ()
with return x => x
   | Clock.now _ k => resume k fixed_time
```

This is necessary for advanced control flow, but most host-capability handlers
are one-shot providers. Hiko could add restricted sugar:

```hiko
-- Proposed syntax:
provide program ()
with
  Clock.now _ => fixed_time
  Aws.S3.list_buckets _ => Result.Ok fake_buckets
```

Desugaring conceptually to:

```hiko
handle program ()
with return x => x
   | Clock.now arg k => resume k fixed_time
   | Aws.S3.list_buckets arg k => resume k (Result.Ok fake_buckets)
```

This keeps everyday capability handling out of explicit continuation style while
retaining full handlers for generators, schedulers, state interpreters, and other
advanced uses.

### 5. Top-Level Policy Integration

For executable programs, Hiko can eventually require that all remaining top-level
effects are provided by the selected VM policy.

Example future type:

```text
main : unit -> unit ! { Filesystem.read_text, Aws.S3.list_buckets }
```

The runner should accept this only if the selected policy grants/provides those
effects. This turns effect rows into an audit surface for host capabilities.

## Implementation Strategies

There are several ways to implement algebraic effects. They make different
tradeoffs in type-system complexity, runtime machinery, and ergonomics.

### Strategy A: Dynamic Handlers Without Effect Rows

This is close to OCaml 5 and current Hiko.

- Effect operations are typed locally.
- Handlers are dynamically installed around an expression.
- Function types do not record which effects may occur.
- Unhandled effects are runtime errors.

Pros:

- simpler type inference;
- smaller change to an ML-like language;
- good ergonomics;
- suitable for local effects, tests, and controlled internal abstractions.

Cons:

- callers cannot see effect requirements in function types;
- no compile-time proof that all effects are handled;
- can feel like checked values plus unchecked control flow.

This is acceptable as a stepping stone, but it is not ideal for Hiko's
capability/security goals.

### Strategy B: Explicit Operation Signatures, No Effect Rows

This is a useful intermediate step.

```hiko
effect Clock.now : unit -> Instant
effect Aws.S3.list_buckets : Aws.Config -> Result Aws.S3.Error BucketList
```

- `perform`/`request` has the declared result type.
- Function types still do not list effect sets.

Pros:

- fixes ambiguity of `effect X of payload`;
- makes effect operations self-documenting;
- keeps inference much simpler than full rows.

Cons:

- unhandled effects are still not statically tracked at function boundaries.

This is probably the best next language-design step before full effect rows.

### Strategy C: Checked Effect Rows

Effect rows extend function types with a set/row of possible effects:

```text
A -> B ! { E1, E2 }
```

Rules:

- `request E arg` adds `E` to the current expression/function effect row.
- calling an effectful function propagates its row;
- handling `E` removes `E` from the row, or transforms it into another row;
- top-level programs must have an empty row or policy-provided row.

Pros:

- effect requirements are visible and auditable;
- unhandled effects can become compile errors;
- good fit for Hiko policies and capabilities.

Cons:

- significant type-system work;
- row polymorphism/generalization rules are needed for reusable higher-order
  functions;
- module signatures need effect annotations;
- diagnostics become harder;
- handlers that transform effects need careful typing.

This is the strongest long-term design, but it should be staged.

### Strategy D: Monadic Encoding

Effects can be encoded in ordinary types instead of language-level handlers.

Haskell's `IO`, `Either`, `State`, `Reader`, and effect libraries are examples.
A Hiko-like encoding could use values representing computations:

```text
Program effects result
```

or explicit records of callbacks/capabilities passed through the program.

Pros:

- no VM continuation machinery required;
- effects are visible in ordinary types;
- mature theoretical model.

Cons:

- less direct-style code;
- more plumbing;
- nested effects can require monad transformers or effect libraries;
- not as ergonomic for existing Hiko direct-style code.

Hiko already has direct-style local effects, so a pure monadic rewrite is
unlikely to be the best path, but Haskell is still useful as a design reference.

### Strategy E: Free Monad / Freer / Extensible Effects

Another typed encoding represents operations as data:

```text
ReadFile path next
HttpSend request next
Pure value
```

Interpreters walk this data structure.

Pros:

- very explicit;
- easy to inspect, test, serialize, or restrict;
- no captured VM continuations.

Cons:

- allocation-heavy unless optimized;
- direct-style source needs desugaring or monadic syntax;
- complex for performance-sensitive workloads.

This may be useful for DSLs or IaC plans, where inspectable operation trees are a
feature.

### Strategy F: Runtime Delimited Continuations

This is the direct algebraic-effects implementation style.

- `perform` captures the continuation up to the nearest handler;
- the handler receives a continuation object;
- `resume` reinstalls the saved continuation.

Implementation choices include:

- copying VM stack slices;
- heap-allocating continuation frames;
- one-shot continuations that can be resumed at most once;
- multi-shot continuations that clone captured continuations;
- shallow versus deep handlers.

Hiko currently has VM-level continuation objects and deep-handler behavior. This
fits the existing runtime, but Hiko should be conservative:

- prefer one-shot continuations unless multi-shot is explicitly designed;
- keep provider-style operations single-resume by construction;
- reserve exposed `resume` for advanced users and library authors.

## Language Comparisons

### Standard ML, F#, and Traditional OCaml Exceptions

Classic ML languages have typed exception constructors but unchecked exception
propagation.

OCaml-style example:

```ocaml
exception Fail of string

let parse_int s =
  if s = "" then raise (Fail "empty") else 42
```

The type is approximately:

```text
string -> int
```

It does not record that `Fail` may be raised. This is exactly the "fancy goto"
problem Hiko should avoid for ordinary recoverable errors.

### OCaml 5 Algebraic Effects

OCaml 5 has algebraic effects and handlers. Effect operations are typed, but
standard OCaml function types do not carry checked effect rows.

Conceptually:

```ocaml
type _ Effect.t += Ask : unit -> int Effect.t
```

This says:

```text
Ask : unit -> int
```

`perform Ask` has type `int`, but a function performing `Ask` still has an
ordinary function type such as:

```text
unit -> int
```

not:

```text
unit -> int ! { Ask }
```

OCaml's design is pragmatic for a mature language: typed operations and dynamic
handlers, but no checked effect rows in ordinary function types.

Hiko can learn from OCaml's operation-signature syntax, but should be cautious
about inheriting unchecked propagation for capability-sensitive code.

### Koka

Koka is designed around algebraic effects and effect rows. Function types record
which effects may occur.

A Koka-like type might be represented as:

```text
string -> int ! { fail, console }
```

Handlers remove or transform effects. Koka also has sophisticated effect-row
inference, effect polymorphism, and direct-style syntax.

Koka is a strong reference for Hiko's long-term static-effect direction, but it
is also a warning: full effect rows are a real language-design project, not a
small parser tweak.

### Eff

Eff is a research language centered on algebraic effects and handlers. It uses
explicit effect operations and handlers as core language constructs.

Eff demonstrates the clean theoretical model:

- operations are declared with parameter/result types;
- handlers interpret operations;
- resumptions are first-class within handlers.

It is useful as a semantic reference for Hiko's `perform`/`handle`/`resume`
behavior.

### Frank

Frank makes effects part of the function/interface notation. It emphasizes the
idea that functions are computations with explicit abilities/effects.

Frank is useful for thinking about readable source syntax. Hiko's possible
`requires { ... }` spelling is closer in spirit to Frank's explicit ability
interfaces than to terse internal row notation.

### Links

Links supports algebraic effects and handlers in a language aimed at web
programming. It is a useful reference for combining effects with practical
application programming and typed handlers.

Links reinforces a key point for Hiko: effects are not only for exceptions; they
can model database, web, session, and other ambient operations in a structured
way.

### Haskell

Haskell does not use built-in algebraic effects in the same direct way. It
usually represents effects in types with monads:

```haskell
readFile :: FilePath -> IO String
parseInt :: String -> Either String Int
```

This makes effects visible, but changes programming style. Advanced Haskell
libraries approximate algebraic/effect-row systems:

```haskell
Members '[Error String, Reader Env, Embed IO] r => Sem r Int
```

Haskell is the strongest reminder that hiding effects from types is not the only
option. It also shows the ergonomic cost of making all effects explicit without
language-level direct-style support.

### Unison, Scala ZIO, and Effect Libraries

Languages/libraries such as Unison abilities, Scala ZIO, Cats Effect, and
various extensible-effect systems also make effects/capabilities visible in
computation types.

They are not ML-family baselines in the same way as OCaml/SML, but they are
useful references for Hiko's capability direction:

- effects are requirements of a computation;
- handlers/interpreters provide those requirements;
- errors are usually values, not hidden throws;
- environment/capability needs are part of the API.

## Recommended Staging for Hiko

### Stage 1: Keep Current Semantics, Tighten Documentation and Tests

Status: in progress.

- Keep current `effect`, `perform`, `handle`, `resume` behavior.
- Document that `effect X of T` declares the payload type only.
- Keep hardening typechecker/verifier/runtime protocol checks.

### Stage 2: Add Explicit Operation Signature Syntax

Add:

```hiko
effect Name : Payload -> Result
```

Keep old syntax temporarily as compatibility sugar or inferred-result syntax:

```hiko
effect Name of Payload
```

Potential interpretation:

```text
payload type known, result type inferred
```

Prefer new code to use the explicit `:` form.

### Stage 3: Rename or Alias `perform` to `request`

Consider:

```hiko
request Clock.now ()
```

as capability-oriented syntax. `perform` can remain as lower-level or legacy
terminology if desired.

### Stage 4: Infer Effect Rows Internally

Before requiring source annotations, teach the typechecker to infer and print:

```text
A -> B ! { E1, E2 }
```

This gives tooling and diagnostics a way to surface effect requirements.

### Stage 5: Add Source Effect Annotations

Support one or both forms:

```hiko
fun f x : A -> B ! { E1, E2 } = ...
```

```hiko
fun f x requires { E1, E2 } = ...
```

The `requires` spelling is more readable for Hiko's capability model. The `!`
form is compact and useful in type printers/signatures.

### Stage 6: Add Provider-Style Handler Sugar

Add one-shot provider syntax:

```hiko
provide expr
with
  Clock.now _ => fixed_now
  Aws.S3.list_buckets cfg => Result.Ok buckets
```

This should be deliberately less powerful than full `handle`: no explicit
continuation, exactly one response per request.

### Stage 7: Connect Top-Level Effect Rows to VM Policy

At executable boundaries:

- source handlers can eliminate effects;
- VM policy can provide approved host effects;
- unhandled/unprovided effects should become compile/run setup errors before
  normal execution begins.

## Open Design Questions

- Should `request` replace `perform`, or only alias it?
- Should provider effects be one-shot only by default?
- Should multi-shot resumptions require an explicit marker?
- Should Hiko distinguish resumable operations from abortive operations in the
  type system?
- Should host builtins eventually become policy-provided effects, or should
  builtin functions and effects coexist?
- How should effect rows appear in `signature` and `structure` declarations?
- How much effect polymorphism is needed for common higher-order functions?
- Should old `effect X of T` remain permanently as shorthand, or be deprecated?

## Summary

Algebraic effects are powerful enough to encode exceptions, but Hiko should not
lean on that style. The more appropriate direction is:

```text
explicit operation signatures
+ visible effect requirements
+ Result-valued provider failures
+ policy/provider boundaries
```

That makes effects closer to typed capability requests than fancy non-local
jumps.
