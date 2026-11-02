# Effect Typing: Design Note

Status: **proposal, awaiting maintainer review** (plan Y, #144; issue #48).
Nothing here is implemented. Implementation starts only after the
maintainer accepts this note or a revision of it.

## Goal

Make an unhandled effect a compile error with a source location. Today the
type checker checks payloads, results and `resume`, but function types do not
record effects, so `perform` without a handler is a controlled runtime error
([effects.md](../language/effects.md#non-guarantees)).

The first stage covers the two clause forms that remain after #121 (provider
and abort). It does not add new effect features.

## Where Effects Stand

These facts constrain the design:

- Effects are nominal and global: `effect Log of string` declares one
  operation. There are no effect parameters and no effect families.
- The payload type is closed (no type variables). The result type is one
  monomorphic type shared by every `perform` and handler of that effect.
- `k` is not a value. A provider clause ends every path with `resume k e`. An
  abort clause does not mention `k`.
- A provider clause runs at the perform site **with its handler hidden**, so a
  `perform` inside a clause goes to an outer handler.
- I/O is builtins, not effects. No builtin performs an effect.
- `spawn` starts a child process with no handlers installed.
- The type checker uses Hindley-Milner with a syntactic value restriction
  (`is_syntactic_value` in `hiko-types/src/infer.rs`) and a work limit
  (`max_type_work`).

The repository's effect users are small: `Std.Filesystem` (11 effects, all
handled by `Filesystem.run`), four examples and three benchmarks.

## Decision Summary

| Question | Proposal |
| --- | --- |
| Rows or sets | Sets of effect names with an optional tail variable: `{Log, Ask \| e}`. No duplicate labels. |
| Where rows live | On function arrows only: `a -{r}-> b`. Expressions get a row during inference. |
| Polymorphism | Row variables are generalized like type variables, under the same value restriction. |
| Higher-order functions | Inferred by row polymorphism; no conservative rule needed. |
| `handle` | Removes the handled effects from the body's row and adds the clauses' rows. |
| `spawn` | The child function's row must be empty. |
| Top level | Each top-level declaration's row must be empty. |
| User annotations | None for code without type annotations. In annotations, unmarked arrows share one row variable; `! {}` marks a pure arrow. |
| Verifier | No new verifier check. The runtime error stays as a backstop for hand-written bytecode. |

The rest of the note explains each row and gives examples.

## Representation

```text
row   ::= { L1, ..., Ln }          closed: exactly these effects
        | { L1, ..., Ln | e }      open: these effects and whatever e becomes
type  ::= ... | type -{row}-> type
```

A row is a set of effect names plus an optional row variable. `Type::Arrow`
gains a third field, the row. Row variables are a separate kind from type
variables and live in the same substitution.

Row unification is the standard set-with-tail rule (Rémy, Leijen):

- `{A, B | e1} ~ {B, C | e2}`: bind `e1 := {C | e3}` and `e2 := {A | e3}`
  with a fresh `e3`.
- A closed row unifies with another row only if the label sets match, after
  binding the other side's tail.
- Occurs check as for types.

Sets, not Koka's scoped rows with duplicate labels, are enough because Hiko
effects carry no type parameters and a handler removes every occurrence of
its effect. A `perform` inside a provider clause goes to an outer handler,
which the `handle` rule below expresses by adding the clause rows to the
result, not by duplicating labels.

## Inference Rules

Every expression is inferred with a type and a row. The row of an expression
is always open during inference (it has a fresh tail), so unification acts as
set union. Only the empty-row checks at `spawn` and at the top level close a
row.

- **Literal, variable, `fn`:** the expression's row is a fresh open row. A
  `fn` body's row becomes the row on the arrow it builds.
- **Application `f x`:** the rows of `f` and `x`, and the arrow row of `f`'s
  type, are unified into the expression's row.
- **`perform E x`:** the row contains `E`, plus the row of `x`.
- **`let` / `val`:** the rows of the bound expression and the body are
  unified. Generalization is unchanged: a binding is generalized only when
  its expression is a syntactic value, and then row variables are generalized
  together with type variables. A `fn` is a syntactic value, so functions get
  row-polymorphic types.
- **`handle body with return x => r | E1 ... | En ...`:**
  - infer `body` with row `{E1, ..., En | e}`, where `e` is fresh;
  - infer the return arm and every clause with row `e`;
  - the `handle` expression has row `e`.

  The handled effects leave the row. The clauses' own effects, including a
  `perform E1` inside an `E1` clause, which goes to an outer handler, stay in
  `e`. If the body does not perform `E1`, the open row still unifies, so a
  handler for an effect that never occurs is accepted (and could become a
  warning later).
- **`resume k e`:** as today, with the row of `e` added to the clause's row.
- **`spawn f` (and `Fiber.spawn`, which calls it):** the arrow row of `f`'s
  type is unified with the closed row `{}`. A child process has no handlers,
  so any effect in it would be unhandled.
- **Top level:** the row of every top-level `val` expression, and of the
  program as a whole, is unified with `{}`. A function *declared* at top level
  may have a non-empty row; only *calling* it outside a handler is an error.

Builtin signatures (`hiko-builtin-meta/src/signatures.rs`) need no change.
When a builtin's signature is instantiated, each of its arrows gets the same
fresh open row, because no builtin performs an effect. The exception is
`spawn`, whose argument arrow is closed: `(unit -{}-> 'a) -> pid`.

## Examples

Accepted:

```hiko
effect Log of string

(* inferred: string -{Log | e}-> unit *)
fun note msg = perform Log msg

(* inferred: (unit -{Log | e}-> 'a) -{e}-> 'a *)
fun with_console f =
  handle f () with return x => x
     | Log m k => let val _ = println m in resume k () end

val _ = with_console (fn () => note "hello")         (* row {} *)

(* map is row-polymorphic: ('a -{e}-> 'b) -> 'a list -{e}-> 'b list *)
val _ = with_console (fn () => List.map note ["a", "b"])

(* Std.Filesystem: read_text : string -{ReadText | e}-> string,
   run handles all eleven effects *)
val text = Filesystem.run (fn () => Filesystem.read_text "README.md")
```

Rejected, with the location of the call that brings the effect to the top
level:

```hiko
val _ = note "hello"
(* error: effect `Log` is not handled
     performed by `note`, called here *)

val text = Filesystem.read_text "README.md"
(* error: effect `ReadText` is not handled *)

val child = spawn (fn () => note "from child")
(* error: a spawned function must not perform effects; this one may perform `Log` *)
```

Nested handlers and a provider that asks an outer handler, both from
effects.md, type as expected: the inner `Ask` clause's `perform Ask` lands in
the inner `handle`'s result row, and the outer handler removes it.

## Diagnostics

An unhandled-effect error names the effect and points at the expression whose
row could not be closed: the top-level `val`, or the `spawn` argument. To
point at the `perform` or the call that introduced the effect, the checker
keeps, for each label it adds to a row, the span of the expression that added
it. This is bookkeeping in the checker, not part of the type.

Printed types show rows only when they are non-empty after generalization:
`note : string -> unit ! {Log}`. A row variable that appears only once is not
printed.

## Annotations

Code without type annotations needs none. Hiko annotates in two places: `(e :
t)` and the `val name : t = ...` exports that many `Std` modules use. For
those:

- every arrow written without a row in one annotation shares one fresh row
  variable. `val map : ('a -> 'b) -> 'a list -> 'b list` means
  `('a -{e}-> 'b) -{e}-> 'a list -{e}-> 'b list`. This is the common
  higher-order case, and it is what an implementation that calls its argument
  needs;
- `t ! {}` marks a pure arrow, and `t ! {Log}` lists effects. They are needed
  only to state a closed row, as in a wrapper around `spawn`.

Because the shared variable is open, annotated pure functions keep working
unchanged. No repository file should need an annotation except possibly
`Std.Fiber`, if its `spawn` wrappers are given explicit types.

## Generalization and the Value Restriction

Row variables follow the existing value restriction: a non-value binding such
as `val f = make_handler ()` is not generalized, so its row variables stay
monomorphic, as its type variables already do. At the end of checking, a row
variable that is still unbound is set to `{}`. This is sound because a row
variable only ever gains labels through unification with a `perform` or a
call. If no constraint added a label, no effect can flow through it.

Effect result types stay monomorphic and global, as today.

## Verifier

Open question 6 in project-state.md asks whether the verifier needs a
protocol-level check. With static rows, every source program that compiles
has a handler for every `perform`. Bytecode that did not come from the
compiler can still perform an unhandled effect, and the VM already turns that
into a controlled runtime error. Proposal: no new verifier check. The runtime
error stays as the backstop, and effects.md says so.

## Cost

Rows add one field to every arrow and one more kind of unification. The
expected cost is a constant factor on unification. The plan's success
criterion is to measure type checking of the largest repository program
before and after, and to stay within `max_type_work`. Row operations are
charged to the same work counter, so a hostile program cannot use rows to
escape the limit.

## Alternatives Considered

- **Strategy B only** (explicit operation result types, no rows): fixes the
  readability of `effect X of t` but does not reject unhandled effects, which
  is the goal of this stage.
- **Effect sets without row variables, with a conservative rule** (a function
  passed as an argument may perform any effect): it rejects `List.map note xs`
  inside a handler or forces annotations everywhere. Row polymorphism costs
  little more and accepts it.
- **Koka-style scoped rows with duplicate labels:** needed for effect
  polymorphism and handlers that re-perform their own effect to an outer
  handler *of the same type instance*. Hiko effects have no type parameters,
  and the outer handler is reached by the `handle` rule above, so sets
  suffice. Moving to scoped rows later would be compatible with the syntax
  proposed here.
- **Subtyping on rows instead of unification:** needs constraint solving and
  makes inferred types harder to read. Not worth it for this language.

## Questions for the Maintainer

1. **Syntax for rows in annotations:** `t ! {Log}` and `t ! {}` as proposed,
   or a `requires { ... }` clause on `fun` (effects-design.md section 3)?
   **Decided: `t ! {...}`.** It is the notation of Eff and of the
   Bauer–Pretnar papers, it can be written inside any type (so it can
   express a pure argument, as `spawn` needs), and `!` is not a Hiko token
   today. A `requires` clause cannot appear inside a type.
2. **A handler for an effect the body never performs:** accept silently (as
   proposed) or warn?
3. **Printed types:** show rows in `hiko check` output and in docgen, or only
   in error messages for now?
4. **Std.Filesystem:** keep the effect-per-operation design, so that every
   caller's row lists the operations it uses, or collapse it to one
   `Filesystem` effect for shorter rows?

## Implementation Plan (after acceptance)

1. Add rows to `Type::Arrow` and row unification, with all rows open and no
   checks. Every repository program must still type-check, as a no-op stage.
2. Add the `perform`, `handle`, `spawn` and top-level rules, with tests for
   handled, unhandled, nested, an effect passed through a function argument
   (`List.map`), and `spawn`.
3. Annotation syntax and printing.
4. Update effects.md "Static Guarantees" and "Non-Guarantees", and measure
   type-checking cost.
