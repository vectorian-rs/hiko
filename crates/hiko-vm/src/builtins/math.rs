use super::*;

pub(super) fn sqrt(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => Ok(Value::Float(f.sqrt())),
        _ => Err("sqrt: expected Float".into()),
    }
}

pub(super) fn abs_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.abs())),
        _ => Err("abs_int: expected Int".into()),
    }
}

pub(super) fn abs_float(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => Ok(Value::Float(f.abs())),
        _ => Err("abs_float: expected Float".into()),
    }
}

pub(super) fn floor(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => Ok(Value::Int(f.floor() as i64)),
        _ => Err("floor: expected Float".into()),
    }
}

pub(super) fn ceil(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => Ok(Value::Int(f.ceil() as i64)),
        _ => Err("ceil: expected Float".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::*;
    use super::*;

    #[test]
    fn sqrt_positive() {
        let mut heap = Heap::new();
        let result = sqrt(&[Value::Float(9.0)], &mut heap).unwrap();
        assert_float_approx(result, 3.0, f64::EPSILON);
    }

    #[test]
    fn sqrt_zero() {
        let mut heap = Heap::new();
        let result = sqrt(&[Value::Float(0.0)], &mut heap).unwrap();
        assert_float_approx(result, 0.0, f64::EPSILON);
    }

    #[test]
    fn sqrt_negative_returns_nan() {
        let mut heap = Heap::new();
        let result = sqrt(&[Value::Float(-1.0)], &mut heap).unwrap();
        match result {
            Value::Float(f) => assert!(f.is_nan()),
            other => panic!("expected Float(NaN), got {other:?}"),
        }
    }

    #[test]
    fn sqrt_type_error() {
        let mut heap = Heap::new();
        let result = sqrt(&[Value::Int(9)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Float"));
    }

    #[test]
    fn abs_int_positive() {
        let mut heap = Heap::new();
        let result = abs_int(&[Value::Int(42)], &mut heap).unwrap();
        assert_int(result, 42);
    }

    #[test]
    fn abs_int_negative() {
        let mut heap = Heap::new();
        let result = abs_int(&[Value::Int(-42)], &mut heap).unwrap();
        assert_int(result, 42);
    }

    #[test]
    fn abs_int_zero() {
        let mut heap = Heap::new();
        let result = abs_int(&[Value::Int(0)], &mut heap).unwrap();
        assert_int(result, 0);
    }

    #[test]
    fn abs_int_type_error() {
        let mut heap = Heap::new();
        let result = abs_int(&[Value::Float(1.0)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Int"));
    }

    #[test]
    fn abs_float_negative() {
        let mut heap = Heap::new();
        let result = abs_float(&[Value::Float(-2.75)], &mut heap).unwrap();
        assert_float_approx(result, 2.75, f64::EPSILON);
    }

    #[test]
    fn abs_float_positive() {
        let mut heap = Heap::new();
        let result = abs_float(&[Value::Float(2.5)], &mut heap).unwrap();
        assert_float_approx(result, 2.5, f64::EPSILON);
    }

    #[test]
    fn abs_float_infinity() {
        let mut heap = Heap::new();
        let result = abs_float(&[Value::Float(f64::NEG_INFINITY)], &mut heap).unwrap();
        match result {
            Value::Float(f) => assert_eq!(f, f64::INFINITY),
            other => panic!("expected Float(inf), got {other:?}"),
        }
    }

    #[test]
    fn floor_positive_fraction() {
        let mut heap = Heap::new();
        let result = floor(&[Value::Float(3.7)], &mut heap).unwrap();
        assert_int(result, 3);
    }

    #[test]
    fn floor_negative_fraction() {
        let mut heap = Heap::new();
        let result = floor(&[Value::Float(-2.3)], &mut heap).unwrap();
        assert_int(result, -3);
    }

    #[test]
    fn floor_exact_int() {
        let mut heap = Heap::new();
        let result = floor(&[Value::Float(5.0)], &mut heap).unwrap();
        assert_int(result, 5);
    }

    #[test]
    fn floor_type_error() {
        let mut heap = Heap::new();
        let result = floor(&[Value::Int(3)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Float"));
    }

    #[test]
    fn ceil_positive_fraction() {
        let mut heap = Heap::new();
        let result = ceil(&[Value::Float(3.1)], &mut heap).unwrap();
        assert_int(result, 4);
    }

    #[test]
    fn ceil_negative_fraction() {
        let mut heap = Heap::new();
        let result = ceil(&[Value::Float(-2.7)], &mut heap).unwrap();
        assert_int(result, -2);
    }

    #[test]
    fn ceil_exact_int() {
        let mut heap = Heap::new();
        let result = ceil(&[Value::Float(5.0)], &mut heap).unwrap();
        assert_int(result, 5);
    }

    #[test]
    fn ceil_type_error() {
        let mut heap = Heap::new();
        let result = ceil(&[Value::Bool(true)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Float"));
    }
}
