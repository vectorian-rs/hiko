use super::*;

pub(super) fn bytes_length(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => Ok(Value::Int(b.len() as i64)),
            _ => Err("bytes_length: expected Bytes".into()),
        },
        _ => Err("bytes_length: expected Bytes".into()),
    }
}

pub(super) fn bytes_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => Ok(Value::Heap(
                heap.alloc(HeapObject::String(String::from_utf8_lossy(b).into_owned())),
            )),
            _ => Err("bytes_to_string: expected Bytes".into()),
        },
        _ => Err("bytes_to_string: expected Bytes".into()),
    }
}

pub(super) fn string_to_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Heap(
                heap.alloc(HeapObject::Bytes(s.as_bytes().to_vec())),
            )),
            _ => Err("string_to_bytes: expected String".into()),
        },
        _ => Err("string_to_bytes: expected String".into()),
    }
}

pub(super) fn bytes_get(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("bytes_get: expected (Bytes, Int)".into()),
        },
        _ => return Err("bytes_get: expected (Bytes, Int)".into()),
    };
    let bytes = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => b,
            _ => return Err("bytes_get: expected Bytes".into()),
        },
        _ => return Err("bytes_get: expected Bytes".into()),
    };
    let idx = match v1 {
        Value::Int(n) if n >= 0 => n as usize,
        Value::Int(n) => return Err(format!("bytes_get: index must be non-negative, got {n}")),
        _ => return Err("bytes_get: expected Int for index".into()),
    };
    if idx >= bytes.len() {
        return Err(format!(
            "bytes_get: index {} out of bounds (length {})",
            idx,
            bytes.len()
        ));
    }
    Ok(Value::Int(bytes[idx] as i64))
}

pub(super) fn bytes_slice(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1, v2) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 3 => (t[0], t[1], t[2]),
            _ => return Err("bytes_slice: expected (Bytes, Int, Int)".into()),
        },
        _ => return Err("bytes_slice: expected (Bytes, Int, Int)".into()),
    };
    let bytes = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => b,
            _ => return Err("bytes_slice: expected Bytes".into()),
        },
        _ => return Err("bytes_slice: expected Bytes".into()),
    };
    let start = match v1 {
        Value::Int(n) if n >= 0 => n as usize,
        Value::Int(n) => return Err(format!("bytes_slice: start must be non-negative, got {n}")),
        _ => return Err("bytes_slice: expected Int for start".into()),
    };
    let len = match v2 {
        Value::Int(n) if n >= 0 => n as usize,
        Value::Int(n) => return Err(format!("bytes_slice: length must be non-negative, got {n}")),
        _ => return Err("bytes_slice: expected Int for length".into()),
    };
    let end = (start + len).min(bytes.len());
    let start = start.min(bytes.len());
    Ok(Value::Heap(
        heap.alloc(HeapObject::Bytes(bytes[start..end].to_vec())),
    ))
}
