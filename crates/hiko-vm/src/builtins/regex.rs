use super::*;
use ::regex::Regex;

pub(super) fn regex_match(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("regex_match: expected (String, String)".into()),
        },
        _ => return Err("regex_match: expected (String, String)".into()),
    };
    let s = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("regex_match: expected String".into()),
        },
        _ => return Err("regex_match: expected String".into()),
    };
    let pattern = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("regex_match: expected String".into()),
        },
        _ => return Err("regex_match: expected String".into()),
    };
    let re = Regex::new(pattern).map_err(|e| format!("regex_match: {e}"))?;
    Ok(Value::Bool(re.is_match(s)))
}

pub(super) fn regex_replace(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1, v2) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 3 => (t[0], t[1], t[2]),
            _ => return Err("regex_replace: expected (String, String, String)".into()),
        },
        _ => return Err("regex_replace: expected (String, String, String)".into()),
    };
    let s = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("regex_replace: expected String".into()),
        },
        _ => return Err("regex_replace: expected String".into()),
    };
    let pattern = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("regex_replace: expected String".into()),
        },
        _ => return Err("regex_replace: expected String".into()),
    };
    let replacement = match v2 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("regex_replace: expected String".into()),
        },
        _ => return Err("regex_replace: expected String".into()),
    };
    let re = Regex::new(pattern).map_err(|e| format!("regex_replace: {e}"))?;
    Ok(Value::Heap(heap.alloc(HeapObject::String(
        re.replace_all(s, replacement).into_owned(),
    ))))
}
