# Rust Memory Layout Notes

This document records Rust memory-layout rules that matter for Hiko VM data
structures. The goal is to prevent accidental per-object memory bloat in hot
runtime types such as `HeapObject`.

## Value composition is inline by default

In Rust, struct and enum fields are stored inline unless the type explicitly uses
an indirection such as `Box`, `Arc`, `Rc`, `Vec`, or a reference. This differs
from languages such as Java, Python, and JavaScript, where object fields are
usually references and `null` costs roughly one pointer.

For example:

```rust
struct BigStruct {
    frames: Vec<SavedFrame>,
    stack: Vec<Value>,
    handler: Option<SavedHandler>,
}

struct Parent {
    maybe_big: Option<BigStruct>,
}
```

`Parent` contains the storage for `BigStruct` inline. `Option<BigStruct>` costs
approximately `size_of::<BigStruct>()` even when the value is `None`; the parent
must reserve enough space for the `Some(BigStruct { ... })` case.

By contrast:

```rust
struct Parent {
    maybe_big: Option<Box<BigStruct>>,
}
```

`Option<Box<BigStruct>>` costs one pointer-sized word in the parent. The actual
`BigStruct` contents are allocated separately only when present. This mirrors the
reference-style behavior that many garbage-collected languages provide
implicitly.

The broader rule is:

> In Rust, composition is value composition by default, not reference
> composition.

When a large field is rarely populated, storing it inline can waste memory equal
to the large field size multiplied by the number of parent instances. Boxing
breaks that relationship: common parent values pay only the pointer-sized
indirection, while rare populated values pay an extra allocation.

## Enum variants are sized for the largest variant

Rust enums are also affected. An enum value must be large enough to hold its
largest variant plus its discriminant/niche metadata. If one rare variant is very
large, every enum value pays for that variant.

This matters for Hiko because the GC heap stores objects as:

```rust
Vec<Option<HeapObject>>
```

Every heap slot is sized for `HeapObject`, not for the particular variant stored
in that slot. A large rare variant therefore inflates every heap slot, including
common `String`, `Tuple`, `Data`, and `Bytes` objects.

Hiko's continuation payload is a good example. Continuations are important but
rare compared with ordinary heap objects. Keeping all continuation fields inline
inside `HeapObject::Continuation` made every heap slot larger. Boxing the
continuation payload moved those fields behind one pointer:

```rust
pub struct ContinuationData {
    pub saved_frames: Vec<SavedFrame>,
    pub saved_stack: Vec<Value>,
    pub saved_handler: Option<SavedHandler>,
}

pub enum HeapObject {
    // common variants ...
    Continuation(Box<ContinuationData>),
}
```

Measured on the current target at the time of the change:

| Type | Size |
| --- | ---: |
| `HeapObject` before boxing continuation payload | 112 bytes |
| `HeapObject` after boxing continuation payload | 56 bytes |
| `Value` | 16 bytes |
| `SavedFrame` | 40 bytes |
| `SavedHandler` | 64 bytes |
| `Fields` | 48 bytes |

This halves the size of every `HeapObject` slot. The tradeoff is one additional
allocation and one pointer indirection when creating or accessing a continuation.
That tradeoff is appropriate here because continuations are not the common heap
object case.

## When to box

Boxing is a good candidate when all of these are true:

1. The field or enum variant is large.
2. The field or variant is absent or uncommon in normal execution.
3. The parent type is instantiated many times or appears in hot storage such as a
   heap, arena, cache, or table.
4. The extra allocation and pointer indirection are acceptable for the uncommon
   case.

Boxing is usually a poor candidate when the field or variant is common and hot.
For example, `HeapObject::Tuple` and `HeapObject::Data` contain `Fields`, which
is relatively large, but those variants are common in Hiko programs. Boxing them
would reduce the enum size but add allocations to common paths. Do not make that
tradeoff without measurements.

## Optional fields

The same principle applies to optional fields:

```rust
struct State {
    rare: Option<BigStruct>,
}
```

This stores `BigStruct` inline and pays its full size even when `rare` is
`None`. If `rare` is large and usually absent, prefer:

```rust
struct State {
    rare: Option<Box<BigStruct>>,
}
```

This keeps the common `None` case compact.

## Serde and wire formats

The memory-layout choice does not need to dictate the wire format. When
serializing or deserializing with Serde, it is often possible to keep the same
external representation while choosing a compact internal representation.

For example, after deserializing a large optional payload, code can decide that
the payload is semantically empty and store `None` instead of `Some(Box::new(...))`.
That keeps the in-memory representation compact without changing the serialized
schema.

## Checklist for Hiko changes

When adding fields to hot structs or variants to hot enums, check:

- What is `size_of::<Parent>()` before and after?
- Is this field/variant common or rare?
- Does the parent appear in a `Vec`, arena, GC heap, process table, or cache?
- Would `Box<T>` or `Option<Box<T>>` reduce common-case memory use?
- Would boxing add allocations to a hot path?
- Can a test assert the intended size relationship without making the exact size
  too brittle?

For layout-sensitive changes, prefer tests such as:

```rust
assert!(std::mem::size_of::<HeapObject>() < OLD_SIZE);
```

rather than asserting every platform's exact enum size, unless the exact size is
known to be stable across the supported targets.
