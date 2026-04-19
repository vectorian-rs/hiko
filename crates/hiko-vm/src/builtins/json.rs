use super::*;

pub(super) fn json_parse(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let json_str = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("json_parse: expected String".into()),
        },
        _ => return Err("json_parse: expected String".into()),
    };
    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("json_parse: {e}"))?;
    Ok(json_to_hiko(&parsed, heap))
}

pub(super) fn json_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let result = hiko_to_json_string(args[0], heap)?;
    Ok(Value::Heap(heap.alloc(HeapObject::String(result))))
}

pub(super) fn json_get(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("json_get: expected (String, String)".into()),
        },
        _ => return Err("json_get: expected (String, String)".into()),
    };
    let json_str = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("json_get: expected String".into()),
        },
        _ => return Err("json_get: expected String".into()),
    };
    let path = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("json_get: expected String".into()),
        },
        _ => return Err("json_get: expected String".into()),
    };

    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("json_get: {e}"))?;

    let mut current = &parsed;
    for key in path.split('.') {
        current = if let Ok(idx) = key.parse::<usize>() {
            current.get(idx).unwrap_or(&serde_json::Value::Null)
        } else {
            current.get(key).unwrap_or(&serde_json::Value::Null)
        };
    }

    let result = match current {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    };

    Ok(Value::Heap(heap.alloc(HeapObject::String(result))))
}

pub(super) fn json_keys(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let json_str = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("json_keys: expected String".into()),
        },
        _ => return Err("json_keys: expected String".into()),
    };

    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("json_keys: {e}"))?;

    let keys: Vec<Value> = match &parsed {
        serde_json::Value::Object(map) => map
            .keys()
            .map(|k| Value::Heap(heap.alloc(HeapObject::String(k.clone()))))
            .collect(),
        _ => Vec::new(),
    };

    Ok(alloc_list(heap, keys))
}

pub(super) fn json_length(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let json_str = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("json_length: expected String".into()),
        },
        _ => return Err("json_length: expected String".into()),
    };

    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("json_length: {e}"))?;

    let len = match &parsed {
        serde_json::Value::Array(arr) => arr.len(),
        serde_json::Value::Object(map) => map.len(),
        serde_json::Value::String(s) => s.len(),
        _ => 0,
    };

    Ok(Value::Int(len as i64))
}
