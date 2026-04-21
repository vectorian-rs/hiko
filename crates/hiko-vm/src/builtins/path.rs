use super::*;

pub(super) fn path_join(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) => (t[0], t[1]),
            _ => return Err("path_join: expected (String, String)".into()),
        },
        _ => return Err("path_join: expected (String, String)".into()),
    };
    let a = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("path_join: expected String".into()),
        },
        _ => return Err("path_join: expected String".into()),
    };
    let b = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("path_join: expected String".into()),
        },
        _ => return Err("path_join: expected String".into()),
    };
    let joined = std::path::Path::new(&a).join(&b);
    heap_alloc(
        heap,
        HeapObject::String(joined.to_string_lossy().to_string()),
    )
}
