use super::*;

// Host work in opcode-equivalents, from the host_work_calibration example
// (docs/builtins/accounting-matrix.md#calibration).
const HASH_BASE_HOST_WORK: u64 = 20;
const HASH_PER_KIB_HOST_WORK: u64 = 62;

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
    charge(
        heap,
        "blake3",
        host_work(HASH_BASE_HOST_WORK, HASH_PER_KIB_HOST_WORK, bytes.len()),
    )?;
    heap_alloc(
        heap,
        HeapObject::String(hiko_common::blake3_hex(&bytes).into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_arg(heap: &mut Heap, data: &[u8]) -> Value {
        heap_alloc(heap, HeapObject::Bytes(data.into())).unwrap()
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
