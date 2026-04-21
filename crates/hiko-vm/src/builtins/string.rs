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

#[cfg(test)]
mod tests {
    use super::super::test_helpers::*;
    use super::*;

    #[test]
    fn string_length_ascii() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "hello");
        let result = string_length(&[arg], &mut heap).unwrap();
        assert_int(result, 5);
    }

    #[test]
    fn string_length_unicode() {
        let mut heap = Heap::new();
        // U+00E9 (e-acute) is 1 char, U+1F30D (globe) is 1 char
        let arg = string_arg(&mut heap, "h\u{00E9}llo \u{1F30D}");
        let result = string_length(&[arg], &mut heap).unwrap();
        assert_int(result, 7);
    }

    #[test]
    fn string_length_empty() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "");
        let result = string_length(&[arg], &mut heap).unwrap();
        assert_int(result, 0);
    }

    #[test]
    fn string_length_type_error() {
        let mut heap = Heap::new();
        let result = string_length(&[Value::Int(42)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected String"));
    }

    #[test]
    fn substring_basic() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let arg = tuple3(&mut heap, s, Value::Int(6), Value::Int(5));
        let result = substring(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "world");
    }

    #[test]
    fn substring_from_start() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "abcdef");
        let arg = tuple3(&mut heap, s, Value::Int(0), Value::Int(3));
        let result = substring(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "abc");
    }

    #[test]
    fn substring_unicode() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "caf\u{00E9}");
        let arg = tuple3(&mut heap, s, Value::Int(0), Value::Int(4));
        let result = substring(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "caf\u{00E9}");
    }

    #[test]
    fn substring_out_of_bounds() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hi");
        let arg = tuple3(&mut heap, s, Value::Int(0), Value::Int(10));
        let result = substring(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("out of bounds"));
    }

    #[test]
    fn substring_negative_start() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello");
        let arg = tuple3(&mut heap, s, Value::Int(-1), Value::Int(2));
        let result = substring(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("negative start"));
    }

    #[test]
    fn string_contains_true() {
        let mut heap = Heap::new();
        let haystack = string_arg(&mut heap, "hello world");
        let needle = string_arg(&mut heap, "world");
        let arg = tuple2(&mut heap, haystack, needle);
        let result = string_contains(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn string_contains_false() {
        let mut heap = Heap::new();
        let haystack = string_arg(&mut heap, "hello world");
        let needle = string_arg(&mut heap, "xyz");
        let arg = tuple2(&mut heap, haystack, needle);
        let result = string_contains(&[arg], &mut heap).unwrap();
        assert_bool(result, false);
    }

    #[test]
    fn string_contains_empty_needle() {
        let mut heap = Heap::new();
        let haystack = string_arg(&mut heap, "hello");
        let needle = string_arg(&mut heap, "");
        let arg = tuple2(&mut heap, haystack, needle);
        let result = string_contains(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn trim_whitespace() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "  hello  ");
        let result = trim(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello");
    }

    #[test]
    fn trim_no_whitespace() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "hello");
        let result = trim(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello");
    }

    #[test]
    fn trim_empty() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "   ");
        let result = trim(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "");
    }

    #[test]
    fn split_basic() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "a,b,c");
        let sep = string_arg(&mut heap, ",");
        let arg = tuple2(&mut heap, s, sep);
        let result = split(&[arg], &mut heap).unwrap();
        assert_eq!(collect_string_list(result, &heap), vec!["a", "b", "c"]);
    }

    #[test]
    fn split_no_separator_found() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello");
        let sep = string_arg(&mut heap, ",");
        let arg = tuple2(&mut heap, s, sep);
        let result = split(&[arg], &mut heap).unwrap();
        assert_eq!(collect_string_list(result, &heap), vec!["hello"]);
    }

    #[test]
    fn split_empty_string() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "");
        let sep = string_arg(&mut heap, ",");
        let arg = tuple2(&mut heap, s, sep);
        let result = split(&[arg], &mut heap).unwrap();
        assert_eq!(collect_string_list(result, &heap), vec![""]);
    }

    #[test]
    fn string_replace_basic() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let from = string_arg(&mut heap, "world");
        let to = string_arg(&mut heap, "rust");
        let arg = tuple3(&mut heap, s, from, to);
        let result = string_replace(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello rust");
    }

    #[test]
    fn string_replace_multiple() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "aaa");
        let from = string_arg(&mut heap, "a");
        let to = string_arg(&mut heap, "bb");
        let arg = tuple3(&mut heap, s, from, to);
        let result = string_replace(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "bbbbbb");
    }

    #[test]
    fn string_replace_no_match() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello");
        let from = string_arg(&mut heap, "xyz");
        let to = string_arg(&mut heap, "abc");
        let arg = tuple3(&mut heap, s, from, to);
        let result = string_replace(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello");
    }

    #[test]
    fn starts_with_true() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let prefix = string_arg(&mut heap, "hello");
        let arg = tuple2(&mut heap, s, prefix);
        let result = starts_with(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn starts_with_false() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let prefix = string_arg(&mut heap, "world");
        let arg = tuple2(&mut heap, s, prefix);
        let result = starts_with(&[arg], &mut heap).unwrap();
        assert_bool(result, false);
    }

    #[test]
    fn starts_with_empty_prefix() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello");
        let prefix = string_arg(&mut heap, "");
        let arg = tuple2(&mut heap, s, prefix);
        let result = starts_with(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn ends_with_true() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let suffix = string_arg(&mut heap, "world");
        let arg = tuple2(&mut heap, s, suffix);
        let result = ends_with(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn ends_with_false() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let suffix = string_arg(&mut heap, "hello");
        let arg = tuple2(&mut heap, s, suffix);
        let result = ends_with(&[arg], &mut heap).unwrap();
        assert_bool(result, false);
    }

    #[test]
    fn to_upper_basic() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "hello");
        let result = to_upper(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "HELLO");
    }

    #[test]
    fn to_lower_basic() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "HELLO");
        let result = to_lower(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello");
    }

    #[test]
    fn to_upper_empty() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "");
        let result = to_upper(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "");
    }

    #[test]
    fn to_lower_type_error() {
        let mut heap = Heap::new();
        let result = to_lower(&[Value::Int(1)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected String"));
    }

    #[test]
    fn string_join_basic() {
        let mut heap = Heap::new();
        let a = string_arg(&mut heap, "a");
        let b = string_arg(&mut heap, "b");
        let c = string_arg(&mut heap, "c");
        let list = alloc_list(&mut heap, vec![a, b, c]).unwrap();
        let sep = string_arg(&mut heap, ", ");
        let arg = tuple2(&mut heap, list, sep);
        let result = string_join(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "a, b, c");
    }

    #[test]
    fn string_join_empty_list() {
        let mut heap = Heap::new();
        let list = alloc_list(&mut heap, vec![]).unwrap();
        let sep = string_arg(&mut heap, ",");
        let arg = tuple2(&mut heap, list, sep);
        let result = string_join(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "");
    }

    #[test]
    fn string_join_empty_separator() {
        let mut heap = Heap::new();
        let a = string_arg(&mut heap, "x");
        let b = string_arg(&mut heap, "y");
        let list = alloc_list(&mut heap, vec![a, b]).unwrap();
        let sep = string_arg(&mut heap, "");
        let arg = tuple2(&mut heap, list, sep);
        let result = string_join(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "xy");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Allocate a Hiko String on the heap and return its Value.
    fn alloc_str(heap: &mut Heap, s: &str) -> Value {
        let r = heap.alloc(HeapObject::String(s.to_string())).unwrap();
        Value::Heap(r)
    }

    /// Extract a Rust String from a Hiko String value.
    fn extract_str(heap: &Heap, val: Value) -> String {
        match val {
            Value::Heap(r) => match heap.get(r).unwrap() {
                HeapObject::String(s) => s.clone(),
                other => panic!("expected String, got {other:?}"),
            },
            other => panic!("expected Heap value, got {other:?}"),
        }
    }

    /// Build a Hiko tuple of (String, String) on the heap.
    fn alloc_str_pair(heap: &mut Heap, a: &str, b: &str) -> Value {
        let va = alloc_str(heap, a);
        let vb = alloc_str(heap, b);
        let r = heap
            .alloc(HeapObject::Tuple(smallvec::smallvec![va, vb]))
            .unwrap();
        Value::Heap(r)
    }

    /// Build a Hiko tuple of (String, Int, Int) on the heap.
    fn alloc_str_int_int(heap: &mut Heap, s: &str, i: i64, j: i64) -> Value {
        let vs = alloc_str(heap, s);
        let r = heap
            .alloc(HeapObject::Tuple(smallvec::smallvec![
                vs,
                Value::Int(i),
                Value::Int(j)
            ]))
            .unwrap();
        Value::Heap(r)
    }

    /// Build a Hiko tuple of (list, String) on the heap, for string_join.
    fn alloc_list_and_sep(heap: &mut Heap, parts: &[&str], sep: &str) -> Value {
        let mut elems = Vec::new();
        for p in parts {
            elems.push(alloc_str(heap, p));
        }
        let list_val = alloc_list(heap, elems).unwrap();
        let vsep = alloc_str(heap, sep);
        let r = heap
            .alloc(HeapObject::Tuple(smallvec::smallvec![list_val, vsep]))
            .unwrap();
        Value::Heap(r)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        // string_length(s) == s.chars().count()
        #[test]
        fn prop_string_length(s in ".*") {
            let mut heap = Heap::new();
            let val = alloc_str(&mut heap, &s);
            let result = string_length(&[val], &mut heap).unwrap();
            match result {
                Value::Int(n) => prop_assert_eq!(n, s.chars().count() as i64),
                other => prop_assert!(false, "expected Int, got {other:?}"),
            }
        }

        // substring(s, 0, string_length(s)) == s
        #[test]
        fn prop_substring_full(s in "\\PC{0,50}") {
            let mut heap = Heap::new();
            let len = s.chars().count() as i64;
            let arg = alloc_str_int_int(&mut heap, &s, 0, len);
            let result = substring(&[arg], &mut heap).unwrap();
            let result_str = extract_str(&heap, result);
            prop_assert_eq!(result_str, s);
        }

        // to_upper(to_upper(s)) == to_upper(s) (idempotence)
        #[test]
        fn prop_to_upper_idempotent(s in "\\PC{0,50}") {
            let mut heap = Heap::new();
            let val1 = alloc_str(&mut heap, &s);
            let upper1 = to_upper(&[val1], &mut heap).unwrap();
            let upper1_str = extract_str(&heap, upper1);

            let val2 = alloc_str(&mut heap, &upper1_str);
            let upper2 = to_upper(&[val2], &mut heap).unwrap();
            let upper2_str = extract_str(&heap, upper2);

            prop_assert_eq!(upper1_str, upper2_str);
        }

        // to_lower(to_lower(s)) == to_lower(s) (idempotence)
        #[test]
        fn prop_to_lower_idempotent(s in "\\PC{0,50}") {
            let mut heap = Heap::new();
            let val1 = alloc_str(&mut heap, &s);
            let lower1 = to_lower(&[val1], &mut heap).unwrap();
            let lower1_str = extract_str(&heap, lower1);

            let val2 = alloc_str(&mut heap, &lower1_str);
            let lower2 = to_lower(&[val2], &mut heap).unwrap();
            let lower2_str = extract_str(&heap, lower2);

            prop_assert_eq!(lower1_str, lower2_str);
        }

        // int_to_string / string_to_int roundtrip
        #[test]
        fn prop_int_to_string_roundtrip(n: i64) {
            let mut heap = Heap::new();
            // int_to_string
            let str_val =
                super::super::convert::int_to_string(&[Value::Int(n)], &mut heap).unwrap();
            // string_to_int
            let int_val =
                super::super::convert::string_to_int(&[str_val], &mut heap).unwrap();
            match int_val {
                Value::Int(m) => prop_assert_eq!(n, m),
                other => prop_assert!(false, "expected Int, got {other:?}"),
            }
        }

        // split(s, sep) joined back with sep == s
        // Condition: sep is non-empty
        #[test]
        fn prop_split_join_roundtrip(
            s in "[a-zA-Z0-9 ]{0,30}",
            sep in "[,;|]{1,2}",
        ) {
            let mut heap = Heap::new();

            // split
            let split_arg = alloc_str_pair(&mut heap, &s, &sep);
            let split_result = split(&[split_arg], &mut heap).unwrap();

            // Collect the split result into Rust strings
            let parts_values = collect_list(&heap, split_result).unwrap();
            let mut parts_strs = Vec::new();
            for pv in &parts_values {
                parts_strs.push(extract_str(&heap, *pv));
            }

            // join using string_join
            let join_arg = alloc_list_and_sep(
                &mut heap,
                &parts_strs.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                &sep,
            );
            let join_result = string_join(&[join_arg], &mut heap).unwrap();
            let joined = extract_str(&heap, join_result);

            prop_assert_eq!(joined, s);
        }

        #[test]
        fn prop_string_contains_substring(
            prefix in "[a-z]{0,10}",
            needle in "[a-z]{1,5}",
            suffix in "[a-z]{0,10}",
        ) {
            let mut heap = Heap::new();
            let haystack = format!("{prefix}{needle}{suffix}");
            let arg = alloc_str_pair(&mut heap, &haystack, &needle);
            let result = string_contains(&[arg], &mut heap).unwrap();
            prop_assert!(matches!(result, Value::Bool(true)));
        }

        #[test]
        fn prop_trim(s in "[ \t\n]*[a-z]{0,20}[ \t\n]*") {
            let mut heap = Heap::new();
            let val = alloc_str(&mut heap, &s);
            let result = trim(&[val], &mut heap).unwrap();
            let result_str = extract_str(&heap, result);
            prop_assert_eq!(result_str, s.trim().to_string());
        }

        #[test]
        fn prop_starts_with(
            prefix in "[a-z]{1,10}",
            suffix in "[a-z]{0,10}",
        ) {
            let mut heap = Heap::new();
            let full = format!("{prefix}{suffix}");
            let arg = alloc_str_pair(&mut heap, &full, &prefix);
            let result = starts_with(&[arg], &mut heap).unwrap();
            prop_assert!(matches!(result, Value::Bool(true)));
        }

        #[test]
        fn prop_ends_with(
            prefix in "[a-z]{0,10}",
            suffix in "[a-z]{1,10}",
        ) {
            let mut heap = Heap::new();
            let full = format!("{prefix}{suffix}");
            let arg = alloc_str_pair(&mut heap, &full, &suffix);
            let result = ends_with(&[arg], &mut heap).unwrap();
            prop_assert!(matches!(result, Value::Bool(true)));
        }
    }
}
