# Numeric Policy

This document is the source of truth for Hiko's numeric representation and
width-specific numeric module policy.

## Core Numeric Representations

Hiko has three core numeric primitives:

```text
int   = i64
word  = u64
float = f64
```

| Hiko type | VM/Rust representation | Notes |
| --------- | ---------------------- | ----- |
| `int`     | `i64`                  | Signed 64-bit integer |
| `word`    | `u64`                  | Unsigned 64-bit integer |
| `float`   | `f64`                  | IEEE-754 binary64 double |

These representations are part of the language/runtime contract. Conversion
rules and future width-specific modules may rely on them.

## Core Operators

Core numeric operators are a closed compiler-resolved surface over the core
numeric primitives. They are not an extensible typeclass, trait, or implicit
conversion mechanism.

Width-specific numeric types must not add new meanings to core operators. For
example, `Int32.t` values must use `Int32.add`, not `+`.

## Option A: Width-Specific Stdlib Modules

This policy is named **Option A**. Older planning text referred to the same
direction as "Option B"; that name is superseded and should not be used in new
docs, issues, or reviews.

Hiko's width-specific numeric design is OCaml-style module APIs:

- keep `int`, `word`, and `float` as the only core numeric primitives
- expose fixed-width domains through stdlib modules such as `Int32`, `Word32`,
  and `Float32`
- expose opaque `type t` values from those modules
- use concrete functions such as `Int32.add`, `Word32.wrapping_add`, and
  `Float32.mul`
- do not add new operators or numeric inference rules for these module types

The initial stdlib module set is:

- `Int32`
- `Word32`
- `Float32`

These modules reuse existing immediate VM values:

| Module type | Runtime representation | Required invariant |
| ----------- | ---------------------- | ------------------ |
| `Int32.t`   | `Value::Int(i64)`      | Stored value fits `i32` |
| `Word32.t`  | `Value::Word(u64)`     | Stored value fits `u32` |
| `Float32.t` | `Value::Float(f64)`    | Stored value was rounded through `f32` |

Rust builtins validate inputs and canonicalize outputs at every boundary. This
keeps the implementation allocation-free without adding `Value::I32`,
`Value::U32`, or `Value::F32`. Dedicated value variants and opcodes remain a
future optimization if profiling shows the builtin path is too slow.

## Conversion Rules

Widening conversions from width-specific module types to wider core types are
infallible:

| Conversion | Reason |
| ---------- | ------ |
| `Int32.to_int` | `i32 -> i64` always fits |
| `Word32.to_word` | `u32 -> u64` always fits |
| `Word32.to_int` | `u32 -> i64` always fits because `u32::MAX < i64::MAX` |
| `Float32.to_float` | `f32 -> f64` is exact; the stored `f64` is returned unchanged |

Narrowing integer conversions from core types are checked by default:

- `Int32.of_int` checks `i32::MIN..=i32::MAX`
- `Word32.of_word` checks `0..=u32::MAX`
- `Word32.of_int` rejects negative values and values above `u32::MAX`

Modules may also expose explicit alternatives:

- `checked_of_int` / `checked_of_word` returns an option/result
- `wrapping_of_int` / `wrapping_of_word` performs an explicitly named lossy
  conversion

Core `word_to_int` is not a widening conversion: `word` is `u64`, and not every
`u64` value fits in `i64`. Its semantics should be documented separately from
the width-specific widening guarantees.

## Arithmetic Semantics

Signed width modules should be checked by default and expose explicit variants:

- `Int32.add`, `sub`, `mul`, `neg`, and `abs` check overflow
- `Int32.checked_add` returns an option/result instead of failing
- `Int32.wrapping_add` uses two's-complement wrapping
- `Int32.saturating_add` saturates at the type bounds
- `Int32.div` and `rem` check divide-by-zero and signed overflow such as
  `i32::MIN / -1`

Unsigned width modules should follow `word`-style arithmetic:

- `Word32.add`, `sub`, and `mul` wrap by default
- `Word32.checked_add` and `saturating_add` make alternate behavior explicit
- `Word32.div` and `rem` check divide-by-zero
- shifts should check the shift amount by default, with explicit wrapping shift
  functions if needed

Float width modules should follow IEEE-754 behavior for their width:

- `Float32.of_float` rounds through `f32`
- overflow, underflow, subnormals, infinity, NaN, and signed zero are valid
  `Float32.t` values
- arithmetic must operate in `f32` precision and then store the canonical widened
  `f64` value

The correct implementation shape is:

```rust
let a: f32 = unpack_f32(lhs)?;
let b: f32 = unpack_f32(rhs)?;
pack_f32(a + b)
```

The incorrect implementation shape is:

```rust
Value::Float(lhs_f64 + rhs_f64)
```

The latter silently accumulates `f64` precision and violates `Float32.t`
semantics.

## Verification Expectations

Width-specific modules need boundary tests for:

- signed min/max values
- signed overflow and underflow
- `MIN / -1`
- divide-by-zero and remainder-by-zero
- negative and too-large conversions into unsigned widths
- wrapping and saturating variants
- shift amounts at `width - 1`, `width`, and `width + 1`
- `Float32` rounding, overflow to infinity, underflow/subnormals, NaN, signed
  zero, and no accidental `f64` precision accumulation

The bounded TLA+ model in
[`specs/tla/NumericWidthSemantics.tla`](../specs/tla/NumericWidthSemantics.tla)
captures the semantic invariant checks for Int32/Word32 conversion and add
variants, plus a symbolic Float32 rounding invariant. Rust unit tests remain the
source of truth for actual `TryFrom`, `checked_*`, `wrapping_*`, `saturating_*`,
and IEEE-754 behavior.
