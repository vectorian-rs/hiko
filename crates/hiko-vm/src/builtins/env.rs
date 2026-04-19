use super::*;

pub(super) fn getenv(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let name = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("getenv: expected String".into()),
        },
        _ => return Err("getenv: expected String".into()),
    };
    match std::env::var(&name) {
        Ok(val) => Ok(Value::Heap(heap.alloc(HeapObject::String(val)))),
        Err(_) => Ok(Value::Heap(heap.alloc(HeapObject::String(String::new())))),
    }
}
