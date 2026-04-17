# Modules in Hiko

## Status

Hiko currently implements the **Core SML** language, but not the **SML module language**. That exclusion is already called out in [bootstrap.md](./bootstrap.md) and the module MVP is tracked in GitHub issue `#9`.

The current runtime and language direction is described in:

- [whitepaper.md](./whitepaper.md)
- [system.md](./system.md)
- [bootstrap.md](./bootstrap.md)

Those documents already establish the larger design:

- Core SML as the semantic base
- local algebraic effects for intra-process control flow
- an isolated-process runtime
- host-provided, policy-controlled capabilities

What they do **not** yet provide is a clean way to package reusable libraries with hidden internal representations. That is the purpose of the module MVP.

## Why Hiko Needs Modules

There are two separate problems to solve.

### 1. Namespacing

Today, Hiko code is effectively flattened into a top-level environment. That is workable for small scripts, but it becomes awkward for stdlib-quality APIs. A list library wants to expose:

```sml
List.map
List.fold
List.filter
```

instead of:

```sml
map
fold
filter
```

The first form is clearer, avoids name collisions, and scales better as the language grows.

### 2. Representation hiding

Some abstractions can be implemented in Hiko already, especially with local algebraic effects, but they cannot yet be **delivered cleanly**.

A generator is the standard example:

- internally, it may need continuations and a continuation-carrying state representation
- externally, users should only see `make`, `next`, and an abstract generator type

Without opaque abstraction, users can depend on the concrete representation and the library boundary collapses.

## The Hiko Module MVP

The proposed MVP is deliberately narrow:

- `structure`
- `signature`
- opaque ascription `:>`
- qualified names `X.y`

This is enough to unlock:

- namespaced stdlib APIs like `List.fold`
- abstract library-defined types like `Generator.t`
- compile-time-only module resolution with no VM representation

It is **not** full SML modules.

### Explicitly out of scope for the MVP

- `functor`
- `open`
- sharing constraints
- `where type`
- first-class modules
- separate interface files in the OCaml `.mli` style

That omission is intentional. Hiko is a scripting/tooling language first. The goal is to get the useful abstraction boundary without importing the entire complexity of mature ML module systems in one step.

It is also consistent with Hiko's broader SML policy: adopt the strong parts of the language, but avoid known defect clusters where they do not buy much. See [sml-deltas.md](./sml-deltas.md) for the larger triage.

### Why functors are out of scope

The main traditional motivation for functors is library generators such as:

```sml
functor MakeSet (Ord : ORDERED) = ...
functor MakeMap (Ord : ORDERED) = ...
```

Hiko does not need functors to cover the practical version of that use case.

For Hiko, the simpler alternative is:

- ordinary modules for namespacing and abstraction
- opaque types for representation hiding
- explicit comparator-passing at the value level

That means a useful set or map API can look like this instead:

```sml
signature SET = sig
  type 'a t
  val empty : ('a * 'a -> int) -> 'a t
  val insert : 'a -> 'a t -> 'a t
  val member : 'a -> 'a t -> bool
end
```

Internally, the set simply stores the comparison function alongside its tree or table representation.

That is not as elegant as `MakeSet(Ord)`, but it is much cheaper in language complexity:

- no module-level functions
- no higher-order module elaboration
- no functor generativity story
- no additional signature-matching machinery

So the rejection reason is practical, not ideological: **Hiko can get useful `Set`/`Map` abstractions without functors, and functors import a large amount of module complexity that Hiko does not currently need**.

## Proposed Syntax

### Structure definition

```sml
structure List = struct
  fun fold f acc xs =
    case xs of
        [] => acc
      | x :: rest => fold f (f (x, acc)) rest
end
```

### Qualified access

```sml
val sum = List.fold (fn (x, acc) => x + acc) 0 [1, 2, 3]
```

### Signature definition

```sml
signature LIST = sig
  val fold : ('a * 'b -> 'b) -> 'b -> 'a list -> 'b
end
```

### Opaque ascription

```sml
structure List :> LIST = struct
  fun fold f acc xs =
    case xs of
        [] => acc
      | x :: rest => fold f (f (x, acc)) rest
end
```

In this example the opacity does not matter much because `LIST` only exposes values, not abstract types. It becomes important when the signature exposes an abstract type.

## What Opaque Ascription Means

Opaque ascription means:

- a structure is checked against a signature
- only the interface described by the signature remains visible from outside
- concrete type representations not explicitly exposed by the signature are hidden

Example:

```sml
signature QUEUE = sig
  type 'a t
  val empty : 'a t
  val insert : 'a * 'a t -> 'a t
  val remove : 'a t -> 'a * 'a t
end

structure Queue :> QUEUE = struct
  type 'a t = 'a list * 'a list

  val empty = ([], [])

  fun insert (x, (ins, outs)) = (x :: ins, outs)

  fun remove (ins, x :: outs) = (x, (ins, outs))
    | remove (ins, []) =
        case rev ins of
            [] => raise Empty
          | x :: outs => (x, ([], outs))
end
```

Outside the structure:

- `Queue.t` exists
- `Queue.empty`, `Queue.insert`, and `Queue.remove` are usable
- the fact that `Queue.t` is implemented as `'a list * 'a list` is hidden

That is the feature needed for effect-based library abstractions in Hiko.

## Semantics and Compilation Model

For Hiko, the intended model is simple:

- modules are **compile-time namespaces**
- they do **not** have a runtime representation in the VM
- qualified names are resolved during elaboration / type checking / compilation

This keeps the runtime architecture aligned with the rest of the language:

- VM changes should be minimal or unnecessary
- module structure belongs to the front end
- the runtime should continue to execute ordinary compiled code, not module values

This is also why the MVP excludes first-class modules and functors.

## Relationship to `use`

Hiko already has `use "file.hml"` as a practical composition mechanism. Modules do not replace `use`; they complement it.

The likely long-term relationship is:

- `use` brings another file's declarations into scope
- that file can define structures and signatures
- users consume namespaced APIs such as `List.fold` or `Generator.next`

So the module MVP improves **library packaging**, while `use` continues to handle **file-level inclusion**.

## Why Hiko Should Prefer the SML Style

Hiko is not trying to become OCaml with all of OCaml's module machinery, nor Haskell with a namespace/import system plus type classes. It needs the smallest module system that fits:

- script editing
- readable single-file abstractions
- explicit abstraction boundaries
- no unnecessary runtime complication

That points strongly toward the SML style:

- `structure`
- `signature`
- explicit opaque ascription `:>`

This gives good source-level clarity. In a script or library file, the abstraction boundary is visible exactly where the implementation is defined.

## Comparison with Standard ML

Hiko's module MVP is directly inspired by SML.

### What Hiko takes from SML

- `structure`
- `signature`
- explicit opaque ascription `:>`
- qualified path access

### What Hiko does not initially take

- `functor`
- `open`
- sharing constraints
- full module-language expressiveness

### Why this is a good fit

SML's module system is a natural match for Hiko because Hiko already uses Core SML as its language foundation. The conceptual vocabulary is consistent with the rest of the language.

The SML/NJ overview describes the core idea well: structures are modules, signatures are interfaces, and the signature controls what components and types are visible externally. See:

- [Standard ML of New Jersey overview](https://smlnj.org/sml.html)

For the formal language background:

- _The Definition of Standard ML (Revised)_, Milner, Tofte, Harper, MacQueen, 1997

## Comparison with OCaml

OCaml has a far richer and more industrial-strength module system than the Hiko MVP.

### OCaml features Hiko is not trying to copy yet

- functors
- `open`
- `include`
- recursive module patterns
- `.ml` / `.mli` interface split
- a larger collection of advanced signature/path features

### OCaml's abstraction style

In OCaml, abstraction usually lives in the exposed module type or in an `.mli` file. The manual states that:

- `module M : S = ...` checks a module against a module type
- components not specified in `S` are hidden

See:

- [OCaml manual: Modules](https://ocaml.org/manual/modules.html)

This style is powerful, but it is also tied closely to separate compilation and interface files.

### Why Hiko should not copy the `.mli` model first

For Hiko's scripting-oriented workflow:

- explicit source-local abstraction is easier to read
- a second interface file is less attractive while editing small libraries or scripts
- the language does not yet need OCaml's compilation-unit machinery

Xavier Leroy's 1994 paper explains why OCaml went in this direction: explicit opaque signatures plus manifest types support separate compilation much better than classic SML transparency. That is an important engineering result, but it is not Hiko's immediate priority.

Reference:

- Leroy, _Manifest types, modules, and separate compilation_, POPL 1994  
  <https://people.mpi-sws.org/~dreyer/courses/modules/leroy94.pdf>

Modern retrospective work also notes that OCaml modules are highly useful but can become irregular in advanced cases. See:

- Clément Blaudeau, _Retrofitting the ML module system_, 2024  
  <https://clement.blaudeau.net/thesis.html>

That supports Hiko's choice to start small rather than re-create the full OCaml module surface immediately.

## Comparison with Haskell

Haskell's module system solves a somewhat different problem.

### What Haskell modules are good at

- namespace control
- import/export filtering
- qualified imports
- abstract data via hidden constructors

The Haskell 2010 Report describes modules as a top-level way to control namespaces and reuse software in large programs. It also states that:

- modules are not first-class values
- if an export list is omitted, all locally defined values, types, and classes are exported
- exporting a type without its constructors supports abstract datatypes

See:

- [Haskell 2010 Report](https://www.haskell.org/definition/haskell2010.pdf)

### What Haskell does not provide in the same way

Haskell's base language does **not** have the SML/OCaml notion of:

- structures
- signatures
- opaque ascription
- functors

Its abstraction story is centered more around:

- export lists
- hidden constructors
- type classes
- packages/build tooling

### Why Hiko should not model its MVP on Haskell

Haskell's module system is very effective for namespace control, but it is not the right conceptual template for Hiko's immediate need, which is **library packaging with explicit abstract interfaces**.

Haskell can hide a constructor by not exporting it, but that is not the same design vocabulary as:

- “this structure implements this signature”
- “this type stays abstract outside the module”

For Hiko, the SML-style interface language is a better fit because it matches the rest of the language and makes abstraction explicit in the source.

## Hiko Design Recommendation

The recommended path is:

### Phase 1

- `structure`
- qualified names `X.y`

This already unlocks `List.fold`, `List.map`, and other namespaced stdlib APIs.

### Phase 2

- `signature`
- structure checking against signatures

This enables explicit module APIs.

### Phase 3

- opaque ascription `:>`

This unlocks true hidden-representation library types such as generators, parsers, or future resource-handle abstractions.

## Why This Matters for the Rest of Hiko

The module MVP is not an isolated feature. It connects directly to the rest of the current language/runtime plan:

- The [whitepaper](./whitepaper.md) argues that local effects remain local control flow.
- That means useful abstractions such as generators should be definable **in Hiko**, not forced into VM builtins.
- But those abstractions need a proper delivery vehicle.

Modules are that delivery vehicle.

They allow Hiko to keep the right architectural split:

- **Rust / runtime side**: capabilities, I/O, process execution, cloud integrations, parquet/DB access
- **Hiko side**: composition, orchestration, local control abstractions, stdlib APIs

## MVP Constraints to Keep

To avoid overreaching, the initial implementation should keep the following constraints:

- no functors
- no `open`
- no `include`
- no separate interface files
- no runtime module representation
- no VM changes if it can be avoided

That keeps the feature aligned with Hiko's current philosophy:

- explicit
- auditable
- small
- useful immediately

## References

### Internal Hiko documents

- [whitepaper.md](./whitepaper.md)
- [system.md](./system.md)
- [bootstrap.md](./bootstrap.md)

### Standard ML

- Milner, Tofte, Harper, MacQueen. _The Definition of Standard ML (Revised)_, 1997.
- SML/NJ overview: <https://smlnj.org/sml.html>

### OCaml

- OCaml manual, modules chapter: <https://ocaml.org/manual/modules.html>
- Leroy, _Manifest types, modules, and separate compilation_, POPL 1994:  
  <https://people.mpi-sws.org/~dreyer/courses/modules/leroy94.pdf>
- Blaudeau, _Retrofitting the ML module system_, 2024:  
  <https://clement.blaudeau.net/thesis.html>

### Haskell

- _Haskell 2010 Language Report_, Chapter 5 (Modules):  
  <https://www.haskell.org/definition/haskell2010.pdf>
