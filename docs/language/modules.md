# Modules in Hiko

## Current Status

Hiko now has a **minimal module system**.

Implemented:

- top-level `signature`
- top-level `structure`
- opaque ascription `:>`
- ascription with `:`, which is parsed but currently behaves exactly like `:>`
  (see [Ascription Semantics](#ascription-semantics))
- qualified names like `Box.make` and `Json.JObject`

Not implemented:

- `functor`
- `open`
- `where type`
- sharing constraints
- first-class modules
- nested modules inside `struct ... end`
- transparent ascription (`:` does not keep type definitions visible)

This is deliberate. Hiko currently treats modules as a **single top-level namespace layer**, not as a full recursive module language.

## What The Current Model Is For

The module system is there to solve two practical problems:

- **namespacing**
- **representation hiding**

That is enough for APIs like:

```sml
signature LIST = sig
  val fold : ('a * 'b -> 'b) -> 'b -> 'a list -> 'b
end

structure List : LIST = struct
  fun fold f acc xs =
    case xs of
        [] => acc
      | x :: rest => fold f (f (x, acc)) rest
end
```

and for opaque abstractions like:

```sml
signature BOX = sig
  type t
  val make : int -> t
  val get : t -> int
end

structure Box :> BOX = struct
  datatype t = Box of int
  fun make x = Box x
  fun get (Box x) = x
end
```

Outside the structure:

- `Box.t` exists
- `Box.make` and `Box.get` are usable
- the concrete representation of `Box.t` is hidden

## Syntax

### Signature

```sml
signature LIST = sig
  val fold : ('a * 'b -> 'b) -> 'b -> 'a list -> 'b
end
```

### Structure

```sml
structure List = struct
  fun fold f acc xs =
    case xs of
        [] => acc
      | x :: rest => fold f (f (x, acc)) rest
end
```

### Ascription With `:`

```sml
structure List : LIST = struct
  fun fold f acc xs = acc
end
```

In SML this is transparent ascription. In Hiko it currently seals the
structure the same way `:>` does. See [Ascription Semantics](#ascription-semantics).

### Opaque Ascription

```sml
structure Box :> BOX = struct
  datatype t = Box of int
  fun make x = Box x
  fun get (Box x) = x
end
```

### Qualified Access

```sml
val sum = List.fold (fn (x, acc) => x + acc) 0 [1, 2, 3]
val b : Box.t = Box.make 1
```

## Semantics

In Hiko, modules are **compile-time only**.

- they do not have a VM/runtime representation
- qualified names are resolved in the front end
- structures are flattened before inference and code generation

This keeps the runtime simple and matches Hiko’s general design: abstraction belongs in the language front end, not in the VM.

### Ascription Semantics

Ascribing a signature to a structure does two things:

- **checking**: every `val` in the signature must exist in the structure with a
  compatible type, otherwise compilation fails
- **filtering**: names that the signature does not list are hidden outside the
  structure

Both `:` and `:>` do this. Both also make every `type t` in the signature
**abstract** outside the structure: the definition of `t` is hidden.

In SML, `:` is transparent and keeps the definition visible. Hiko does not
implement that difference. The parser records which form was used, but
signature elaboration ignores it (`signature_assertions` in
`crates/hiko-syntax/src/desugar.rs`). For example:

```sml
signature COUNTER = sig
  type t
  val zero : t
end

structure C : COUNTER = struct
  type t = int
  val zero = 0
end

val n = C.zero + 1   (* type error in Hiko today; accepted in SML *)
```

Until this is resolved, write `:>` when sealing is intended. Treat `:` as
unstable: it may become transparent or be removed from the language.

## Deliberate Limits

### No Functors

Hiko does not need functors yet.

The practical use case for `MakeSet` / `MakeMap` can be handled with:

- ordinary modules
- opaque types
- explicit comparator-passing at the value level

That avoids importing higher-order module complexity.

### No Nested Modules

Nested modules are part of SML and OCaml, but Hiko does not need them yet.

For now, one top-level module layer is enough:

- `List`
- `Queue`
- `Http`
- `Generator`

If internal organization becomes painful later, this can be revisited. Right now it is better treated as **out of scope** than as an unfinished promise.

## Imports And Packages

Modules provide namespacing and abstraction inside a program. Composing
programs from files and packages uses `use` and `import`, described in
[imports.md](imports.md).
