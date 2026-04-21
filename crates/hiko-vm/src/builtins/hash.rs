use super::*;

pub(super) fn blake3(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => {
                heap_alloc(heap, HeapObject::String(hiko_common::blake3_hex(b)))
            }
            _ => Err("blake3: expected Bytes".into()),
        },
        _ => Err("blake3: expected Bytes".into()),
    }
}
