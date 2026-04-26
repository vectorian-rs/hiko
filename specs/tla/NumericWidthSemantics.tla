---- MODULE NumericWidthSemantics ----
\* # Documentation
\*
\* ## Why this spec exists
\*
\* This is a focused semantic model for Hiko's width-specific numeric stdlib
\* modules. The Rust implementation is the executable source of truth, but this
\* model states the runtime invariants directly:
\*
\* - `Int32.t` values stored in `Value::Int` must fit in i32.
\* - `Word32.t` values stored in `Value::Word` must fit in u32.
\* - checked conversions and checked operations must not return successful
\*   values outside those bounds.
\* - wrapping and saturating variants must preserve the module invariant.
\* - `Float32.t` operations must re-enter the rounded f32 representation, while
\*   `Float32.to_float` is identity widening of the stored value.
\*
\* ## What we intentionally abstract
\*
\* TLA+ does not execute Rust's `TryFrom`, `checked_*`, `wrapping_*`, or
\* IEEE-754 implementation. Rust unit tests cover those exact APIs. This model
\* instead checks a bounded decision table at the meaningful boundaries and uses
\* symbolic Float32 tokens for the rounding invariant.

EXTENDS Integers, TLC

VARIABLES done

vars == <<done>>

I32Min == -2147483648
I32Max == 2147483647
I32Modulus == 4294967296

U32Min == 0
U32Max == 4294967295
U32Modulus == 4294967296

I64Max == 9223372036854775807

Int32Inputs == {I32Min - 1, I32Min, -1, 0, 1, I32Max, I32Max + 1}
Int32Operands == {I32Min, -1, 0, 1, I32Max}

Word32Inputs == {-1, 0, 1, U32Max, U32Max + 1}
Word32Operands == {0, 1, U32Max}

IsI32(x) == I32Min <= x /\ x <= I32Max
IsU32(x) == U32Min <= x /\ x <= U32Max

Int32OfIntSucceeds(x) == IsI32(x)
Int32CheckedOfIntSome(x) == IsI32(x)
Int32ToIntValue(x) == x

Word32OfWordSucceeds(x) == IsU32(x)
Word32CheckedOfWordSome(x) == IsU32(x)
Word32OfIntSucceeds(x) == IsU32(x)
Word32CheckedOfIntSome(x) == IsU32(x)
Word32ToWordValue(x) == x
Word32ToIntValue(x) == x

Int32CheckedAddSome(a, b) == IsI32(a) /\ IsI32(b) /\ IsI32(a + b)
Int32CheckedAddValue(a, b) == a + b

Int32WrappingAddValue(a, b) ==
    IF a + b < I32Min THEN a + b + I32Modulus
    ELSE IF a + b > I32Max THEN a + b - I32Modulus
    ELSE a + b

Int32SaturatingAddValue(a, b) ==
    IF a + b < I32Min THEN I32Min
    ELSE IF a + b > I32Max THEN I32Max
    ELSE a + b

Word32CheckedAddSome(a, b) == IsU32(a) /\ IsU32(b) /\ IsU32(a + b)
Word32CheckedAddValue(a, b) == a + b

Word32AddValue(a, b) ==
    IF a + b >= U32Modulus THEN a + b - U32Modulus ELSE a + b

Word32SaturatingAddValue(a, b) ==
    IF a + b > U32Max THEN U32Max ELSE a + b

Word32SubValue(a, b) ==
    IF a >= b THEN a - b ELSE U32Modulus - (b - a)

Word32MulByTwoValue(a) ==
    IF a * 2 >= U32Modulus THEN a * 2 - U32Modulus ELSE a * 2

Float32Inputs == {
    "one",
    "tiny-positive",
    "wide-third",
    "wide-max",
    "nan-payload",
    "negative-zero"
}

Float32Values == {
    "one",
    "positive-zero",
    "third-f32",
    "positive-infinity",
    "nan-f32",
    "negative-zero",
    "two",
    "two24"
}

IsF32Stored(x) == x \in Float32Values

RoundF32(x) ==
    CASE x = "one" -> "one"
    [] x = "tiny-positive" -> "positive-zero"
    [] x = "wide-third" -> "third-f32"
    [] x = "wide-max" -> "positive-infinity"
    [] x = "nan-payload" -> "nan-f32"
    [] x = "negative-zero" -> "negative-zero"
    [] x \in Float32Values -> x

Float32Add(a, b) ==
    CASE a = "two24" /\ b = "one" -> "two24"
    [] a = "one" /\ b = "one" -> "two"
    [] a = "positive-infinity" \/ b = "positive-infinity" -> "positive-infinity"
    [] a = "nan-f32" \/ b = "nan-f32" -> "nan-f32"
    [] OTHER -> RoundF32(a)

Float32ToFloat(x) == x

Init == done = FALSE

Next ==
    /\ done' = TRUE

TypeOK == done \in BOOLEAN

Int32ConversionInvariant ==
    /\ \A x \in Int32Inputs : Int32OfIntSucceeds(x) = IsI32(x)
    /\ \A x \in Int32Inputs : Int32CheckedOfIntSome(x) = IsI32(x)
    /\ \A x \in Int32Inputs : Int32CheckedOfIntSome(x) => IsI32(Int32ToIntValue(x))

Int32AddInvariant ==
    /\ \A a, b \in Int32Operands :
        Int32CheckedAddSome(a, b) => IsI32(Int32CheckedAddValue(a, b))
    /\ \A a, b \in Int32Operands :
        IsI32(a) /\ IsI32(b) => IsI32(Int32WrappingAddValue(a, b))
    /\ \A a, b \in Int32Operands :
        IsI32(a) /\ IsI32(b) => IsI32(Int32SaturatingAddValue(a, b))
    /\ Int32WrappingAddValue(I32Max, 1) = I32Min
    /\ Int32WrappingAddValue(I32Min, -1) = I32Max
    /\ Int32SaturatingAddValue(I32Max, 1) = I32Max
    /\ Int32SaturatingAddValue(I32Min, -1) = I32Min

Word32ConversionInvariant ==
    /\ \A x \in Word32Inputs : Word32OfWordSucceeds(x) = IsU32(x)
    /\ \A x \in Word32Inputs : Word32CheckedOfWordSome(x) = IsU32(x)
    /\ \A x \in Word32Inputs : Word32OfIntSucceeds(x) = IsU32(x)
    /\ \A x \in Word32Inputs : Word32CheckedOfIntSome(x) = IsU32(x)
    /\ \A x \in Word32Operands : IsU32(Word32ToWordValue(x))
    /\ \A x \in Word32Operands : Word32ToIntValue(x) <= I64Max

Word32ArithmeticInvariant ==
    /\ \A a, b \in Word32Operands :
        Word32CheckedAddSome(a, b) => IsU32(Word32CheckedAddValue(a, b))
    /\ \A a, b \in Word32Operands :
        IsU32(a) /\ IsU32(b) => IsU32(Word32AddValue(a, b))
    /\ \A a, b \in Word32Operands :
        IsU32(a) /\ IsU32(b) => IsU32(Word32SaturatingAddValue(a, b))
    /\ \A a, b \in Word32Operands :
        IsU32(a) /\ IsU32(b) => IsU32(Word32SubValue(a, b))
    /\ Word32AddValue(U32Max, 1) = 0
    /\ Word32SubValue(0, 1) = U32Max
    /\ Word32MulByTwoValue(U32Max) = U32Max - 1
    /\ Word32SaturatingAddValue(U32Max, 1) = U32Max

Float32Invariant ==
    /\ \A x \in Float32Inputs : IsF32Stored(RoundF32(x))
    /\ \A a, b \in Float32Values : IsF32Stored(Float32Add(a, b))
    /\ \A x \in Float32Values : Float32ToFloat(x) = x
    /\ RoundF32("tiny-positive") = "positive-zero"
    /\ RoundF32("wide-max") = "positive-infinity"
    /\ RoundF32("nan-payload") = "nan-f32"
    /\ RoundF32("negative-zero") = "negative-zero"
    /\ Float32Add("two24", "one") = "two24"

SafetyInvariant ==
    /\ TypeOK
    /\ Int32ConversionInvariant
    /\ Int32AddInvariant
    /\ Word32ConversionInvariant
    /\ Word32ArithmeticInvariant
    /\ Float32Invariant

Spec == Init /\ [][Next]_vars

====
