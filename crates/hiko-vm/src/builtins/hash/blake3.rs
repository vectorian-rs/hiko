use super::*;

const HASH_BASE_HOST_WORK: u64 = 20;
const HASH_PER_KIB_HOST_WORK: u64 = 1;

fn hash_work(bytes: usize) -> u64 {
    HASH_BASE_HOST_WORK.saturating_add(
        (bytes as u64)
            .div_ceil(1024)
            .saturating_mul(HASH_PER_KIB_HOST_WORK),
    )
}

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    &[("blake3", blake3 as BuiltinFn)]
}

pub(super) fn blake3(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let bytes = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => b.clone(),
            _ => return Err("blake3: expected Bytes".into()),
        },
        _ => return Err("blake3: expected Bytes".into()),
    };
    heap.charge_host_work(hash_work(bytes.len()))
        .map_err(|e| format!("blake3: {e}"))?;
    heap_alloc(heap, HeapObject::String(hiko_common::blake3_hex(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_arg(heap: &mut Heap, data: &[u8]) -> Value {
        heap_alloc(heap, HeapObject::Bytes(data.to_vec())).unwrap()
    }

    #[test]
    fn blake3_respects_host_work_limit() {
        let mut heap = Heap::new();
        heap.set_max_host_work(HASH_BASE_HOST_WORK - 1);
        let arg = bytes_arg(&mut heap, b"abc");
        let result = blake3(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("host work budget exceeded"));
    }

    #[test]
    fn blake3_charges_host_work() {
        let mut heap = Heap::new();
        let arg = bytes_arg(&mut heap, b"abc");
        blake3(&[arg], &mut heap).unwrap();
        assert!(heap.host_work_used() >= HASH_BASE_HOST_WORK);
    }
}
