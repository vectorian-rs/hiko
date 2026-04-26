use super::*;
use smallvec::smallvec;

fn make_bool_int_pair(heap: &mut Heap, ok: bool, value: i64) -> Result<Value, String> {
    heap_alloc(
        heap,
        HeapObject::Tuple(smallvec![Value::Bool(ok), Value::Int(value)]),
    )
}

fn make_bool_word_pair(heap: &mut Heap, ok: bool, value: u64) -> Result<Value, String> {
    heap_alloc(
        heap,
        HeapObject::Tuple(smallvec![Value::Bool(ok), Value::Word(value)]),
    )
}

fn extract_int(value: Value, name: &str) -> Result<i64, String> {
    match value {
        Value::Int(n) => Ok(n),
        _ => Err(format!("{name}: expected Int")),
    }
}

fn extract_word(value: Value, name: &str) -> Result<u64, String> {
    match value {
        Value::Word(w) => Ok(w),
        _ => Err(format!("{name}: expected Word")),
    }
}

fn extract_float(value: Value, name: &str) -> Result<f64, String> {
    match value {
        Value::Float(f) => Ok(f),
        _ => Err(format!("{name}: expected Float")),
    }
}

fn extract_pair(args: &[Value], heap: &Heap, name: &str) -> Result<(Value, Value), String> {
    match args.first().copied() {
        Some(Value::Heap(r)) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => Ok((t[0], t[1])),
            _ => Err(format!("{name}: expected pair")),
        },
        _ => Err(format!("{name}: expected pair")),
    }
}

fn extract_int_pair(args: &[Value], heap: &Heap, name: &str) -> Result<(i64, i64), String> {
    let (left, right) = extract_pair(args, heap, name)?;
    Ok((extract_int(left, name)?, extract_int(right, name)?))
}

fn extract_word_pair(args: &[Value], heap: &Heap, name: &str) -> Result<(u64, u64), String> {
    let (left, right) = extract_pair(args, heap, name)?;
    Ok((extract_word(left, name)?, extract_word(right, name)?))
}

fn extract_float_pair(args: &[Value], heap: &Heap, name: &str) -> Result<(f64, f64), String> {
    let (left, right) = extract_pair(args, heap, name)?;
    Ok((extract_float(left, name)?, extract_float(right, name)?))
}

fn unpack_i32(value: i64, name: &str) -> Result<i32, String> {
    i32::try_from(value).map_err(|_| format!("{name}: Int32.t invariant violated: {value}"))
}

fn pack_i32(value: i32) -> Value {
    Value::Int(i64::from(value))
}

fn unpack_u32(value: u64, name: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{name}: Word32.t invariant violated: {value}"))
}

fn pack_u32(value: u32) -> Value {
    Value::Word(u64::from(value))
}

fn unpack_f32(value: f64) -> f32 {
    value as f32
}

fn pack_f32(value: f32) -> Value {
    Value::Float(f64::from(value))
}

fn int32_unary(
    args: &[Value],
    name: &str,
    f: impl FnOnce(i32) -> Option<i32>,
) -> Result<Value, String> {
    let value = extract_int(args[0], name)?;
    let value = unpack_i32(value, name)?;
    f(value)
        .map(pack_i32)
        .ok_or_else(|| format!("{name}: overflow"))
}

fn int32_pair(args: &[Value], heap: &Heap, name: &str) -> Result<(i32, i32), String> {
    let (left, right) = extract_int_pair(args, heap, name)?;
    Ok((unpack_i32(left, name)?, unpack_i32(right, name)?))
}

fn int32_checked_pair(
    args: &[Value],
    heap: &mut Heap,
    name: &str,
    f: impl FnOnce(i32, i32) -> Option<i32>,
) -> Result<Value, String> {
    let (left, right) = int32_pair(args, heap, name)?;
    match f(left, right) {
        Some(value) => make_bool_int_pair(heap, true, i64::from(value)),
        None => make_bool_int_pair(heap, false, 0),
    }
}

fn int32_error_pair(
    args: &[Value],
    heap: &Heap,
    name: &str,
    f: impl FnOnce(i32, i32) -> Option<i32>,
) -> Result<Value, String> {
    let (left, right) = int32_pair(args, heap, name)?;
    f(left, right)
        .map(pack_i32)
        .ok_or_else(|| format!("{name}: overflow or invalid operation"))
}

pub(super) fn int32_min_value(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Ok(pack_i32(i32::MIN))
}

pub(super) fn int32_max_value(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Ok(pack_i32(i32::MAX))
}

pub(super) fn int32_of_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let value = extract_int(args[0], "numeric_int32_of_int")?;
    unpack_i32(value, "numeric_int32_of_int").map(pack_i32)
}

pub(super) fn int32_checked_of_int(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let value = extract_int(args[0], "numeric_int32_checked_of_int")?;
    match i32::try_from(value) {
        Ok(value) => make_bool_int_pair(heap, true, i64::from(value)),
        Err(_) => make_bool_int_pair(heap, false, 0),
    }
}

pub(super) fn int32_to_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let value = extract_int(args[0], "numeric_int32_to_int")?;
    unpack_i32(value, "numeric_int32_to_int").map(|value| Value::Int(i64::from(value)))
}

pub(super) fn int32_add(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    int32_error_pair(args, heap, "numeric_int32_add", i32::checked_add)
}

pub(super) fn int32_checked_add(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    int32_checked_pair(args, heap, "numeric_int32_checked_add", i32::checked_add)
}

pub(super) fn int32_wrapping_add(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = int32_pair(args, heap, "numeric_int32_wrapping_add")?;
    Ok(pack_i32(left.wrapping_add(right)))
}

pub(super) fn int32_saturating_add(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = int32_pair(args, heap, "numeric_int32_saturating_add")?;
    Ok(pack_i32(left.saturating_add(right)))
}

pub(super) fn int32_sub(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    int32_error_pair(args, heap, "numeric_int32_sub", i32::checked_sub)
}

pub(super) fn int32_mul(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    int32_error_pair(args, heap, "numeric_int32_mul", i32::checked_mul)
}

pub(super) fn int32_div(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    int32_error_pair(args, heap, "numeric_int32_div", i32::checked_div)
}

pub(super) fn int32_rem(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    int32_error_pair(args, heap, "numeric_int32_rem", i32::checked_rem)
}

pub(super) fn int32_neg(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    int32_unary(args, "numeric_int32_neg", i32::checked_neg)
}

fn word32_pair(args: &[Value], heap: &Heap, name: &str) -> Result<(u32, u32), String> {
    let (left, right) = extract_word_pair(args, heap, name)?;
    Ok((unpack_u32(left, name)?, unpack_u32(right, name)?))
}

fn word32_checked_pair(
    args: &[Value],
    heap: &mut Heap,
    name: &str,
    f: impl FnOnce(u32, u32) -> Option<u32>,
) -> Result<Value, String> {
    let (left, right) = word32_pair(args, heap, name)?;
    match f(left, right) {
        Some(value) => make_bool_word_pair(heap, true, u64::from(value)),
        None => make_bool_word_pair(heap, false, 0),
    }
}

pub(super) fn word32_min_value(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Ok(pack_u32(u32::MIN))
}

pub(super) fn word32_max_value(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Ok(pack_u32(u32::MAX))
}

pub(super) fn word32_of_word(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let value = extract_word(args[0], "numeric_word32_of_word")?;
    unpack_u32(value, "numeric_word32_of_word").map(pack_u32)
}

pub(super) fn word32_checked_of_word(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let value = extract_word(args[0], "numeric_word32_checked_of_word")?;
    match u32::try_from(value) {
        Ok(value) => make_bool_word_pair(heap, true, u64::from(value)),
        Err(_) => make_bool_word_pair(heap, false, 0),
    }
}

pub(super) fn word32_of_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let value = extract_int(args[0], "numeric_word32_of_int")?;
    let value = u32::try_from(value)
        .map_err(|_| format!("numeric_word32_of_int: value out of Word32 range: {value}"))?;
    Ok(pack_u32(value))
}

pub(super) fn word32_checked_of_int(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let value = extract_int(args[0], "numeric_word32_checked_of_int")?;
    match u32::try_from(value) {
        Ok(value) => make_bool_word_pair(heap, true, u64::from(value)),
        Err(_) => make_bool_word_pair(heap, false, 0),
    }
}

pub(super) fn word32_to_word(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let value = extract_word(args[0], "numeric_word32_to_word")?;
    unpack_u32(value, "numeric_word32_to_word").map(|value| Value::Word(u64::from(value)))
}

pub(super) fn word32_to_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let value = extract_word(args[0], "numeric_word32_to_int")?;
    unpack_u32(value, "numeric_word32_to_int").map(|value| Value::Int(i64::from(value)))
}

pub(super) fn word32_add(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = word32_pair(args, heap, "numeric_word32_add")?;
    Ok(pack_u32(left.wrapping_add(right)))
}

pub(super) fn word32_checked_add(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    word32_checked_pair(args, heap, "numeric_word32_checked_add", u32::checked_add)
}

pub(super) fn word32_saturating_add(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = word32_pair(args, heap, "numeric_word32_saturating_add")?;
    Ok(pack_u32(left.saturating_add(right)))
}

pub(super) fn word32_sub(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = word32_pair(args, heap, "numeric_word32_sub")?;
    Ok(pack_u32(left.wrapping_sub(right)))
}

pub(super) fn word32_mul(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = word32_pair(args, heap, "numeric_word32_mul")?;
    Ok(pack_u32(left.wrapping_mul(right)))
}

pub(super) fn word32_div(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = word32_pair(args, heap, "numeric_word32_div")?;
    left.checked_div(right)
        .map(pack_u32)
        .ok_or_else(|| "numeric_word32_div: divide by zero".into())
}

pub(super) fn word32_rem(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = word32_pair(args, heap, "numeric_word32_rem")?;
    left.checked_rem(right)
        .map(pack_u32)
        .ok_or_else(|| "numeric_word32_rem: remainder by zero".into())
}

pub(super) fn float32_of_float(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let value = extract_float(args[0], "numeric_float32_of_float")?;
    Ok(pack_f32(value as f32))
}

pub(super) fn float32_to_float(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let value = extract_float(args[0], "numeric_float32_to_float")?;
    Ok(Value::Float(value))
}

pub(super) fn float32_neg(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let value = extract_float(args[0], "numeric_float32_neg")?;
    Ok(pack_f32(-unpack_f32(value)))
}

fn float32_pair(args: &[Value], heap: &Heap, name: &str) -> Result<(f32, f32), String> {
    let (left, right) = extract_float_pair(args, heap, name)?;
    Ok((unpack_f32(left), unpack_f32(right)))
}

pub(super) fn float32_add(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = float32_pair(args, heap, "numeric_float32_add")?;
    Ok(pack_f32(left + right))
}

pub(super) fn float32_sub(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = float32_pair(args, heap, "numeric_float32_sub")?;
    Ok(pack_f32(left - right))
}

pub(super) fn float32_mul(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = float32_pair(args, heap, "numeric_float32_mul")?;
    Ok(pack_f32(left * right))
}

pub(super) fn float32_div(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (left, right) = float32_pair(args, heap, "numeric_float32_div")?;
    Ok(pack_f32(left / right))
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::*;
    use super::*;

    fn assert_word(value: Value, expected: u64) {
        match value {
            Value::Word(w) => assert_eq!(w, expected),
            other => panic!("expected Word({expected}), got {other:?}"),
        }
    }

    fn assert_float_bits(value: Value, expected: f64) {
        match value {
            Value::Float(f) => assert_eq!(f.to_bits(), expected.to_bits()),
            other => panic!("expected Float({expected:?}), got {other:?}"),
        }
    }

    fn assert_float_nan(value: Value) {
        match value {
            Value::Float(f) => assert!(f.is_nan(), "expected NaN, got {f:?}"),
            other => panic!("expected Float(NaN), got {other:?}"),
        }
    }

    fn bool_int_pair(value: Value, heap: &Heap) -> (bool, i64) {
        match value {
            Value::Heap(r) => match heap.get(r).unwrap() {
                HeapObject::Tuple(fields) => {
                    let ok = match fields[0] {
                        Value::Bool(ok) => ok,
                        other => panic!("expected Bool, got {other:?}"),
                    };
                    let value = match fields[1] {
                        Value::Int(n) => n,
                        other => panic!("expected Int, got {other:?}"),
                    };
                    (ok, value)
                }
                other => panic!("expected tuple, got {other:?}"),
            },
            other => panic!("expected tuple value, got {other:?}"),
        }
    }

    fn bool_word_pair(value: Value, heap: &Heap) -> (bool, u64) {
        match value {
            Value::Heap(r) => match heap.get(r).unwrap() {
                HeapObject::Tuple(fields) => {
                    let ok = match fields[0] {
                        Value::Bool(ok) => ok,
                        other => panic!("expected Bool, got {other:?}"),
                    };
                    let value = match fields[1] {
                        Value::Word(w) => w,
                        other => panic!("expected Word, got {other:?}"),
                    };
                    (ok, value)
                }
                other => panic!("expected tuple, got {other:?}"),
            },
            other => panic!("expected tuple value, got {other:?}"),
        }
    }

    fn int_args(heap: &mut Heap, left: i64, right: i64) -> Value {
        tuple2(heap, Value::Int(left), Value::Int(right))
    }

    fn word_args(heap: &mut Heap, left: u64, right: u64) -> Value {
        tuple2(heap, Value::Word(left), Value::Word(right))
    }

    fn float_args(heap: &mut Heap, left: f64, right: f64) -> Value {
        tuple2(heap, Value::Float(left), Value::Float(right))
    }

    #[test]
    fn int32_boundaries_and_conversions() {
        let mut heap = Heap::new();

        assert_int(
            int32_min_value(&[], &mut heap).unwrap(),
            i64::from(i32::MIN),
        );
        assert_int(
            int32_max_value(&[], &mut heap).unwrap(),
            i64::from(i32::MAX),
        );

        assert_int(
            int32_of_int(&[Value::Int(i64::from(i32::MIN))], &mut heap).unwrap(),
            i64::from(i32::MIN),
        );
        assert_int(
            int32_of_int(&[Value::Int(i64::from(i32::MAX))], &mut heap).unwrap(),
            i64::from(i32::MAX),
        );
        assert!(int32_of_int(&[Value::Int(i64::from(i32::MIN) - 1)], &mut heap).is_err());
        assert!(int32_of_int(&[Value::Int(i64::from(i32::MAX) + 1)], &mut heap).is_err());

        let checked_min =
            int32_checked_of_int(&[Value::Int(i64::from(i32::MIN))], &mut heap).unwrap();
        assert_eq!(
            bool_int_pair(checked_min, &heap),
            (true, i64::from(i32::MIN))
        );

        let checked_above =
            int32_checked_of_int(&[Value::Int(i64::from(i32::MAX) + 1)], &mut heap).unwrap();
        assert_eq!(bool_int_pair(checked_above, &heap), (false, 0));

        assert_int(
            int32_to_int(&[Value::Int(i64::from(i32::MAX))], &mut heap).unwrap(),
            i64::from(i32::MAX),
        );
        assert!(int32_to_int(&[Value::Int(i64::from(i32::MAX) + 1)], &mut heap).is_err());
        assert!(int32_to_int(&[Value::Int(i64::from(i32::MIN) - 1)], &mut heap).is_err());
    }

    #[test]
    fn int32_checked_arithmetic_boundaries() {
        let mut heap = Heap::new();

        let args = int_args(&mut heap, 40, 2);
        assert_int(int32_add(&[args], &mut heap).unwrap(), 42);

        let args = int_args(&mut heap, i64::from(i32::MAX), 1);
        assert!(int32_add(&[args], &mut heap).is_err());

        let args = int_args(&mut heap, i64::from(i32::MIN), -1);
        assert!(int32_add(&[args], &mut heap).is_err());

        let args = int_args(&mut heap, i64::from(i32::MAX), 1);
        let result = int32_checked_add(&[args], &mut heap).unwrap();
        assert_eq!(bool_int_pair(result, &heap), (false, 0));

        let args = int_args(&mut heap, 40, 2);
        let result = int32_checked_add(&[args], &mut heap).unwrap();
        assert_eq!(bool_int_pair(result, &heap), (true, 42));

        let args = int_args(&mut heap, i64::from(i32::MIN) + 1, 1);
        assert_int(int32_sub(&[args], &mut heap).unwrap(), i64::from(i32::MIN));

        let args = int_args(&mut heap, i64::from(i32::MIN), 1);
        assert!(int32_sub(&[args], &mut heap).is_err());

        let args = int_args(&mut heap, -7, 6);
        assert_int(int32_mul(&[args], &mut heap).unwrap(), -42);

        let args = int_args(&mut heap, i64::from(i32::MAX), 2);
        assert!(int32_mul(&[args], &mut heap).is_err());

        let args = int_args(&mut heap, 6, 2);
        assert_int(int32_div(&[args], &mut heap).unwrap(), 3);

        let args = int_args(&mut heap, 6, 0);
        assert!(int32_div(&[args], &mut heap).is_err());

        let args = int_args(&mut heap, i64::from(i32::MIN), -1);
        assert!(int32_div(&[args], &mut heap).is_err());

        let args = int_args(&mut heap, 7, 3);
        assert_int(int32_rem(&[args], &mut heap).unwrap(), 1);

        let args = int_args(&mut heap, 7, 0);
        assert!(int32_rem(&[args], &mut heap).is_err());

        let args = int_args(&mut heap, i64::from(i32::MIN), -1);
        assert!(int32_rem(&[args], &mut heap).is_err());

        assert_int(int32_neg(&[Value::Int(42)], &mut heap).unwrap(), -42);
        assert!(int32_neg(&[Value::Int(i64::from(i32::MIN))], &mut heap).is_err());
    }

    #[test]
    fn int32_explicit_overflow_variants_are_stable() {
        let mut heap = Heap::new();

        let args = int_args(&mut heap, i64::from(i32::MAX), 1);
        assert_int(
            int32_wrapping_add(&[args], &mut heap).unwrap(),
            i64::from(i32::MIN),
        );

        let args = int_args(&mut heap, i64::from(i32::MIN), -1);
        assert_int(
            int32_wrapping_add(&[args], &mut heap).unwrap(),
            i64::from(i32::MAX),
        );

        let args = int_args(&mut heap, i64::from(i32::MAX), 1);
        assert_int(
            int32_saturating_add(&[args], &mut heap).unwrap(),
            i64::from(i32::MAX),
        );

        let args = int_args(&mut heap, i64::from(i32::MIN), -1);
        assert_int(
            int32_saturating_add(&[args], &mut heap).unwrap(),
            i64::from(i32::MIN),
        );
    }

    #[test]
    fn int32_arithmetic_rejects_forged_out_of_range_values() {
        let mut heap = Heap::new();

        let args = int_args(&mut heap, i64::from(i32::MAX) + 1, 0);
        assert!(int32_add(&[args], &mut heap).is_err());

        let args = int_args(&mut heap, 0, i64::from(i32::MIN) - 1);
        assert!(int32_wrapping_add(&[args], &mut heap).is_err());
    }

    #[test]
    fn word32_boundaries_and_conversions() {
        let mut heap = Heap::new();

        assert_word(
            word32_min_value(&[], &mut heap).unwrap(),
            u64::from(u32::MIN),
        );
        assert_word(
            word32_max_value(&[], &mut heap).unwrap(),
            u64::from(u32::MAX),
        );

        assert_word(
            word32_of_word(&[Value::Word(u64::from(u32::MAX))], &mut heap).unwrap(),
            u64::from(u32::MAX),
        );
        assert!(word32_of_word(&[Value::Word(u64::from(u32::MAX) + 1)], &mut heap).is_err());

        let checked_above =
            word32_checked_of_word(&[Value::Word(u64::from(u32::MAX) + 1)], &mut heap).unwrap();
        assert_eq!(bool_word_pair(checked_above, &heap), (false, 0));

        assert_word(
            word32_of_int(&[Value::Int(i64::from(u32::MAX))], &mut heap).unwrap(),
            u64::from(u32::MAX),
        );
        assert!(word32_of_int(&[Value::Int(-1)], &mut heap).is_err());
        assert!(word32_of_int(&[Value::Int(i64::from(u32::MAX) + 1)], &mut heap).is_err());

        let checked_negative = word32_checked_of_int(&[Value::Int(-1)], &mut heap).unwrap();
        assert_eq!(bool_word_pair(checked_negative, &heap), (false, 0));

        assert_word(
            word32_to_word(&[Value::Word(u64::from(u32::MAX))], &mut heap).unwrap(),
            u64::from(u32::MAX),
        );

        let result = word32_to_int(&[Value::Word(u64::from(u32::MAX))], &mut heap).unwrap();
        assert_int(result, i64::from(u32::MAX));

        assert!(word32_to_word(&[Value::Word(u64::from(u32::MAX) + 1)], &mut heap).is_err());
        assert!(word32_to_int(&[Value::Word(u64::from(u32::MAX) + 1)], &mut heap).is_err());
    }

    #[test]
    fn word32_arithmetic_boundaries() {
        let mut heap = Heap::new();

        let args = word_args(&mut heap, u64::from(u32::MAX), 1);
        assert_word(word32_add(&[args], &mut heap).unwrap(), 0);

        let args = word_args(&mut heap, u64::from(u32::MAX), 1);
        let result = word32_checked_add(&[args], &mut heap).unwrap();
        assert_eq!(bool_word_pair(result, &heap), (false, 0));

        let args = word_args(&mut heap, 40, 2);
        let result = word32_checked_add(&[args], &mut heap).unwrap();
        assert_eq!(bool_word_pair(result, &heap), (true, 42));

        let args = word_args(&mut heap, u64::from(u32::MAX), 1);
        assert_word(
            word32_saturating_add(&[args], &mut heap).unwrap(),
            u64::from(u32::MAX),
        );

        let args = word_args(&mut heap, 0, 1);
        assert_word(word32_sub(&[args], &mut heap).unwrap(), u64::from(u32::MAX));

        let args = word_args(&mut heap, u64::from(u32::MAX), 2);
        assert_word(
            word32_mul(&[args], &mut heap).unwrap(),
            u64::from(u32::MAX - 1),
        );

        let args = word_args(&mut heap, 6, 2);
        assert_word(word32_div(&[args], &mut heap).unwrap(), 3);

        let args = word_args(&mut heap, 6, 0);
        assert!(word32_div(&[args], &mut heap).is_err());

        let args = word_args(&mut heap, 7, 3);
        assert_word(word32_rem(&[args], &mut heap).unwrap(), 1);

        let args = word_args(&mut heap, 7, 0);
        assert!(word32_rem(&[args], &mut heap).is_err());
    }

    #[test]
    fn word32_arithmetic_rejects_forged_out_of_range_values() {
        let mut heap = Heap::new();

        let args = word_args(&mut heap, u64::from(u32::MAX) + 1, 0);
        assert!(word32_add(&[args], &mut heap).is_err());

        let args = word_args(&mut heap, 0, u64::from(u32::MAX) + 1);
        assert!(word32_sub(&[args], &mut heap).is_err());
    }

    #[test]
    fn float32_of_float_rounds_and_preserves_special_values() {
        let mut heap = Heap::new();

        assert_float_bits(
            float32_of_float(&[Value::Float(0.1)], &mut heap).unwrap(),
            f64::from(0.1_f32),
        );
        assert_float_bits(
            float32_of_float(&[Value::Float(f64::MAX)], &mut heap).unwrap(),
            f64::INFINITY,
        );
        assert_float_bits(
            float32_of_float(&[Value::Float(-0.0)], &mut heap).unwrap(),
            -0.0,
        );
        assert_float_bits(
            float32_of_float(&[Value::Float(1.0e-46)], &mut heap).unwrap(),
            0.0,
        );
        assert_float_nan(float32_of_float(&[Value::Float(f64::NAN)], &mut heap).unwrap());
    }

    #[test]
    fn float32_to_float_is_identity_widening() {
        let mut heap = Heap::new();
        let stored = f64::from_bits(0x7ff8_0000_0000_1234);
        let result = float32_to_float(&[Value::Float(stored)], &mut heap).unwrap();
        assert_float_bits(result, stored);
    }

    #[test]
    fn float32_arithmetic_rounds_after_each_operation() {
        let mut heap = Heap::new();

        let args = float_args(&mut heap, 16_777_216.0, 1.0);
        let result = float32_add(&[args], &mut heap).unwrap();
        assert_float_bits(result, f64::from(16_777_216.0_f32 + 1.0_f32));

        let args = float_args(&mut heap, 16_777_216.0, -1.0);
        let result = float32_sub(&[args], &mut heap).unwrap();
        assert_float_bits(result, f64::from(16_777_216.0_f32 - (-1.0_f32)));

        let args = float_args(&mut heap, f64::from(f32::MAX), 2.0);
        let result = float32_mul(&[args], &mut heap).unwrap();
        assert_float_bits(result, f64::from(f32::INFINITY));

        let args = float_args(&mut heap, 1.0, 3.0);
        let result = float32_div(&[args], &mut heap).unwrap();
        assert_float_bits(result, f64::from(1.0_f32 / 3.0_f32));

        assert_float_bits(float32_neg(&[Value::Float(0.0)], &mut heap).unwrap(), -0.0);
    }
}
