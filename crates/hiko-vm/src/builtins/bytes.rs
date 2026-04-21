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
            HeapObject::Bytes(b) => heap_alloc(
                heap,
                HeapObject::String(String::from_utf8_lossy(b).into_owned()),
            ),
            _ => Err("bytes_to_string: expected Bytes".into()),
        },
        _ => Err("bytes_to_string: expected Bytes".into()),
    }
}

pub(super) fn string_to_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => heap_alloc(heap, HeapObject::Bytes(s.as_bytes().to_vec())),
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
    heap_alloc(heap, HeapObject::Bytes(bytes[start..end].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::*;
    use super::*;

    fn bytes_arg(heap: &mut Heap, data: &[u8]) -> Value {
        heap_alloc(heap, HeapObject::Bytes(data.to_vec())).unwrap()
    }

    fn heap_bytes(value: Value, heap: &Heap) -> Vec<u8> {
        match value {
            Value::Heap(r) => match heap.get(r).unwrap() {
                HeapObject::Bytes(b) => b.clone(),
                other => panic!("expected bytes, got {other:?}"),
            },
            other => panic!("expected heap bytes, got {other:?}"),
        }
    }

    #[test]
    fn bytes_length_nonempty() {
        let mut heap = Heap::new();
        let arg = bytes_arg(&mut heap, &[1, 2, 3, 4, 5]);
        let result = bytes_length(&[arg], &mut heap).unwrap();
        assert_int(result, 5);
    }

    #[test]
    fn bytes_length_empty() {
        let mut heap = Heap::new();
        let arg = bytes_arg(&mut heap, &[]);
        let result = bytes_length(&[arg], &mut heap).unwrap();
        assert_int(result, 0);
    }

    #[test]
    fn bytes_length_type_error() {
        let mut heap = Heap::new();
        let result = bytes_length(&[Value::Int(0)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Bytes"));
    }

    #[test]
    fn bytes_to_string_valid_utf8() {
        let mut heap = Heap::new();
        let arg = bytes_arg(&mut heap, b"hello");
        let result = bytes_to_string(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello");
    }

    #[test]
    fn bytes_to_string_invalid_utf8_lossy() {
        let mut heap = Heap::new();
        // 0xFF is not valid UTF-8
        let arg = bytes_arg(&mut heap, &[0xFF, 0xFE]);
        let result = bytes_to_string(&[arg], &mut heap).unwrap();
        let s = heap_string(result, &heap);
        // Should contain replacement characters
        assert!(s.contains('\u{FFFD}'));
    }

    #[test]
    fn bytes_to_string_empty() {
        let mut heap = Heap::new();
        let arg = bytes_arg(&mut heap, &[]);
        let result = bytes_to_string(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "");
    }

    #[test]
    fn string_to_bytes_ascii() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "abc");
        let result = string_to_bytes(&[arg], &mut heap).unwrap();
        assert_eq!(heap_bytes(result, &heap), b"abc");
    }

    #[test]
    fn string_to_bytes_unicode() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "é");
        let result = string_to_bytes(&[arg], &mut heap).unwrap();
        assert_eq!(heap_bytes(result, &heap), "é".as_bytes());
    }

    #[test]
    fn string_to_bytes_empty() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "");
        let result = string_to_bytes(&[arg], &mut heap).unwrap();
        assert_eq!(heap_bytes(result, &heap), Vec::<u8>::new());
    }

    #[test]
    fn bytes_get_first() {
        let mut heap = Heap::new();
        let b = bytes_arg(&mut heap, &[10, 20, 30]);
        let arg = tuple2(&mut heap, b, Value::Int(0));
        let result = bytes_get(&[arg], &mut heap).unwrap();
        assert_int(result, 10);
    }

    #[test]
    fn bytes_get_last() {
        let mut heap = Heap::new();
        let b = bytes_arg(&mut heap, &[10, 20, 30]);
        let arg = tuple2(&mut heap, b, Value::Int(2));
        let result = bytes_get(&[arg], &mut heap).unwrap();
        assert_int(result, 30);
    }

    #[test]
    fn bytes_get_out_of_bounds() {
        let mut heap = Heap::new();
        let b = bytes_arg(&mut heap, &[10, 20]);
        let arg = tuple2(&mut heap, b, Value::Int(5));
        let result = bytes_get(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("out of bounds"));
    }

    #[test]
    fn bytes_get_negative_index() {
        let mut heap = Heap::new();
        let b = bytes_arg(&mut heap, &[10, 20]);
        let arg = tuple2(&mut heap, b, Value::Int(-1));
        let result = bytes_get(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("non-negative"));
    }

    #[test]
    fn bytes_slice_basic() {
        let mut heap = Heap::new();
        let b = bytes_arg(&mut heap, &[1, 2, 3, 4, 5]);
        let arg = tuple3(&mut heap, b, Value::Int(1), Value::Int(3));
        let result = bytes_slice(&[arg], &mut heap).unwrap();
        assert_eq!(heap_bytes(result, &heap), vec![2, 3, 4]);
    }

    #[test]
    fn bytes_slice_whole() {
        let mut heap = Heap::new();
        let b = bytes_arg(&mut heap, &[1, 2, 3]);
        let arg = tuple3(&mut heap, b, Value::Int(0), Value::Int(3));
        let result = bytes_slice(&[arg], &mut heap).unwrap();
        assert_eq!(heap_bytes(result, &heap), vec![1, 2, 3]);
    }

    #[test]
    fn bytes_slice_past_end_clamped() {
        let mut heap = Heap::new();
        let b = bytes_arg(&mut heap, &[1, 2, 3]);
        // start=1, len=100 => clamped to end
        let arg = tuple3(&mut heap, b, Value::Int(1), Value::Int(100));
        let result = bytes_slice(&[arg], &mut heap).unwrap();
        assert_eq!(heap_bytes(result, &heap), vec![2, 3]);
    }

    #[test]
    fn bytes_slice_empty() {
        let mut heap = Heap::new();
        let b = bytes_arg(&mut heap, &[1, 2, 3]);
        let arg = tuple3(&mut heap, b, Value::Int(1), Value::Int(0));
        let result = bytes_slice(&[arg], &mut heap).unwrap();
        assert_eq!(heap_bytes(result, &heap), Vec::<u8>::new());
    }

    #[test]
    fn bytes_slice_negative_start() {
        let mut heap = Heap::new();
        let b = bytes_arg(&mut heap, &[1, 2, 3]);
        let arg = tuple3(&mut heap, b, Value::Int(-1), Value::Int(2));
        let result = bytes_slice(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("non-negative"));
    }
}
