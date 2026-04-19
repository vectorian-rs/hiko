use super::*;

pub(super) fn panic(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Err(s.clone()),
            _ => Err("panic: expected String".into()),
        },
        _ => Err("panic: expected String".into()),
    }
}

pub(super) fn assert(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) => (t[0], t[1]),
            _ => return Err("assert: expected (Bool, String)".into()),
        },
        _ => return Err("assert: expected (Bool, String)".into()),
    };
    match (v0, v1) {
        (Value::Bool(true), _) => Ok(Value::Unit),
        (Value::Bool(false), Value::Heap(r)) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(msg) => Err(format!("assertion failed: {msg}")),
            _ => Err("assertion failed".into()),
        },
        _ => Err("assert: expected (Bool, String)".into()),
    }
}

pub(super) fn assert_eq(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1, v2) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 3 => (t[0], t[1], t[2]),
            _ => return Err("assert_eq: expected (a, a, String)".into()),
        },
        _ => return Err("assert_eq: expected (a, a, String)".into()),
    };
    if crate::vm::values_equal(v0, v1, heap) {
        Ok(Value::Unit)
    } else {
        let msg = match v2 {
            Value::Heap(r) => match heap.get(r) {
                Ok(HeapObject::String(s)) => s.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        Err(format!(
            "assertion failed: {msg}: expected {:?}, got {:?}",
            v1, v0
        ))
    }
}
