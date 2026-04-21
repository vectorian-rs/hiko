use super::*;

pub(super) fn string_length(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Int(s.chars().count() as i64)),
            _ => Err("string_length: expected String".into()),
        },
        _ => Err("string_length: expected String".into()),
    }
}

pub(super) fn substring(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1, v2) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) => (t[0], t[1], t[2]),
            _ => return Err("substring: expected (String, Int, Int)".into()),
        },
        _ => return Err("substring: expected (String, Int, Int)".into()),
    };
    let s = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s,
            _ => return Err("substring: expected String".into()),
        },
        _ => return Err("substring: expected String".into()),
    };
    match (v1, v2) {
        (Value::Int(start), Value::Int(len)) => {
            let start = usize::try_from(start)
                .map_err(|_| "substring: negative start index".to_string())?;
            let len = usize::try_from(len).map_err(|_| "substring: negative length".to_string())?;
            let result: String = s.chars().skip(start).take(len).collect();
            if result.chars().count() < len {
                Err("substring: out of bounds".to_string())
            } else {
                heap_alloc(heap, HeapObject::String(result))
            }
        }
        _ => Err("substring: expected (String, Int, Int)".into()),
    }
}

pub(super) fn string_contains(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) => (t[0], t[1]),
            _ => return Err("string_contains: expected (String, String)".into()),
        },
        _ => return Err("string_contains: expected (String, String)".into()),
    };
    let a = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("string_contains: expected String".into()),
        },
        _ => return Err("string_contains: expected String".into()),
    };
    let b = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("string_contains: expected String".into()),
        },
        _ => return Err("string_contains: expected String".into()),
    };
    Ok(Value::Bool(a.contains(b)))
}

pub(super) fn trim(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => heap_alloc(heap, HeapObject::String(s.trim().to_string())),
            _ => Err("trim: expected String".into()),
        },
        _ => Err("trim: expected String".into()),
    }
}

pub(super) fn split(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) => (t[0], t[1]),
            _ => return Err("split: expected (String, String)".into()),
        },
        _ => return Err("split: expected (String, String)".into()),
    };
    let s = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("split: expected String".into()),
        },
        _ => return Err("split: expected String".into()),
    };
    let sep = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("split: expected String".into()),
        },
        _ => return Err("split: expected String".into()),
    };
    let mut parts = Vec::new();
    for p in s.split(&sep) {
        parts.push(heap_alloc(heap, HeapObject::String(p.to_string()))?);
    }
    alloc_list(heap, parts)
}

pub(super) fn string_replace(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1, v2) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 3 => (t[0], t[1], t[2]),
            _ => return Err("string_replace: expected (String, String, String)".into()),
        },
        _ => return Err("string_replace: expected (String, String, String)".into()),
    };
    let s = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("string_replace: expected String".into()),
        },
        _ => return Err("string_replace: expected String".into()),
    };
    let from = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("string_replace: expected String".into()),
        },
        _ => return Err("string_replace: expected String".into()),
    };
    let to = match v2 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("string_replace: expected String".into()),
        },
        _ => return Err("string_replace: expected String".into()),
    };
    heap_alloc(heap, HeapObject::String(s.replace(&from, &to)))
}

pub(super) fn starts_with(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("starts_with: expected (String, String)".into()),
        },
        _ => return Err("starts_with: expected (String, String)".into()),
    };
    let s = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("starts_with: expected String".into()),
        },
        _ => return Err("starts_with: expected String".into()),
    };
    let prefix = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("starts_with: expected String".into()),
        },
        _ => return Err("starts_with: expected String".into()),
    };
    Ok(Value::Bool(s.starts_with(prefix)))
}

pub(super) fn ends_with(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("ends_with: expected (String, String)".into()),
        },
        _ => return Err("ends_with: expected (String, String)".into()),
    };
    let s = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("ends_with: expected String".into()),
        },
        _ => return Err("ends_with: expected String".into()),
    };
    let suffix = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("ends_with: expected String".into()),
        },
        _ => return Err("ends_with: expected String".into()),
    };
    Ok(Value::Bool(s.ends_with(suffix)))
}

pub(super) fn to_upper(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => heap_alloc(heap, HeapObject::String(s.to_uppercase())),
            _ => Err("to_upper: expected String".into()),
        },
        _ => Err("to_upper: expected String".into()),
    }
}

pub(super) fn to_lower(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => heap_alloc(heap, HeapObject::String(s.to_lowercase())),
            _ => Err("to_lower: expected String".into()),
        },
        _ => Err("to_lower: expected String".into()),
    }
}

pub(super) fn string_join(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("string_join: expected (String list, String)".into()),
        },
        _ => return Err("string_join: expected (String list, String)".into()),
    };
    let sep = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("string_join: expected String for separator".into()),
        },
        _ => return Err("string_join: expected String for separator".into()),
    };
    let elems = collect_list(heap, v0)?;
    let mut parts = Vec::new();
    for elem in elems {
        match elem {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::String(s) => parts.push(s.clone()),
                _ => return Err("string_join: list elements must be strings".into()),
            },
            _ => return Err("string_join: list elements must be strings".into()),
        }
    }
    heap_alloc(heap, HeapObject::String(parts.join(&sep)))
}
