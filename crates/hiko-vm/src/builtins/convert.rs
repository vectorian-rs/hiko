use super::*;

pub(super) fn int_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => heap_alloc(heap, HeapObject::String(n.to_string())),
        _ => Err("int_to_string: expected Int".into()),
    }
}

pub(super) fn float_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => heap_alloc(heap, HeapObject::String(f.to_string())),
        _ => Err("float_to_string: expected Float".into()),
    }
}

pub(super) fn string_to_int(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s
                .trim()
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|e| format!("string_to_int: {e}")),
            _ => Err("string_to_int: expected String".into()),
        },
        _ => Err("string_to_int: expected String".into()),
    }
}

pub(super) fn char_to_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Char(c) => Ok(Value::Int(*c as i64)),
        _ => Err("char_to_int: expected Char".into()),
    }
}

pub(super) fn int_to_char(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => char::from_u32(*n as u32)
            .map(Value::Char)
            .ok_or_else(|| format!("int_to_char: invalid codepoint {n}")),
        _ => Err("int_to_char: expected Int".into()),
    }
}

pub(super) fn int_to_float(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        _ => Err("int_to_float: expected Int".into()),
    }
}
