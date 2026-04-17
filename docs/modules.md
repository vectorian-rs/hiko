# Modules in Hiko

## Current Status

Hiko now has a **minimal module system**.

Implemented:

- top-level `signature`
- top-level `structure`
- transparent ascription `:`
- opaque ascription `:>`
- qualified names like `List.fold` and `Queue.t`

Not implemented:

- `functor`
- `open`
- `where type`
- sharing constraints
- first-class modules
- nested modules inside `struct ... end`

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

### Transparent Ascription

```sml
structure List : LIST = struct
  fun fold f acc xs = acc
end
```

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
val q : Queue.t = Queue.empty
```

## Semantics

In Hiko, modules are **compile-time only**.

- they do not have a VM/runtime representation
- qualified names are resolved in the front end
- structures are flattened before inference and code generation

This keeps the runtime simple and matches Hiko’s general design: abstraction belongs in the language front end, not in the VM.

## Relationship To `use`

`use "file.hml"` still handles file-level composition.

Modules do not replace `use`. They complement it:

- `use` brings declarations from another file into scope
- those declarations may define `signature`s and `structure`s
- users consume namespaced APIs through qualified names

So today:

- `use` is the file mechanism
- modules are the namespace/abstraction mechanism

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

## What Is Next

The next module work is not deeper module theory. It is the **library/import boundary**:

- separate local file inclusion from named module imports
- add a manifest/lockfile story
- add a cache for remote modules

That is what turns the current module syntax into a real library system.
