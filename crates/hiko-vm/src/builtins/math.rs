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
