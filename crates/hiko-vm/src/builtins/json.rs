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
    json_to_hiko(&parsed, heap)
}

pub(super) fn json_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let result = hiko_to_json_string(args[0], heap)?;
    heap_alloc(heap, HeapObject::String(result))
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

    heap_alloc(heap, HeapObject::String(result))
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
        serde_json::Value::Object(map) => {
            let mut keys = Vec::with_capacity(map.len());
            for k in map.keys() {
                keys.push(heap_alloc(heap, HeapObject::String(k.clone()))?);
            }
            keys
        }
        _ => Vec::new(),
    };

    alloc_list(heap, keys)
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

#[cfg(test)]
mod tests {
    use super::super::test_helpers::*;
    use super::*;

    #[test]
    fn json_parse_string() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, r#""hello""#);
        let result = json_parse(&[arg], &mut heap).unwrap();
        // Should be a JStr Data node; roundtrip via json_to_string
        let str_result = json_to_string(&[result], &mut heap).unwrap();
        assert_eq!(heap_string(str_result, &heap), r#""hello""#);
    }

    #[test]
    fn json_parse_integer() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "42");
        let result = json_parse(&[arg], &mut heap).unwrap();
        let str_result = json_to_string(&[result], &mut heap).unwrap();
        assert_eq!(heap_string(str_result, &heap), "42");
    }

    #[test]
    fn json_parse_float() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "3.14");
        let result = json_parse(&[arg], &mut heap).unwrap();
        let str_result = json_to_string(&[result], &mut heap).unwrap();
        let s = heap_string(str_result, &heap);
        // Should parse as a float and round-trip
        assert!(s.starts_with("3.14"));
    }

    #[test]
    fn json_parse_boolean() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "true");
        let result = json_parse(&[arg], &mut heap).unwrap();
        let str_result = json_to_string(&[result], &mut heap).unwrap();
        assert_eq!(heap_string(str_result, &heap), "true");
    }

    #[test]
    fn json_parse_null() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "null");
        let result = json_parse(&[arg], &mut heap).unwrap();
        let str_result = json_to_string(&[result], &mut heap).unwrap();
        assert_eq!(heap_string(str_result, &heap), "null");
    }

    #[test]
    fn json_parse_malformed() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "{invalid json}");
        let result = json_parse(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("json_parse"));
    }

    #[test]
    fn json_parse_type_error() {
        let mut heap = Heap::new();
        let result = json_parse(&[Value::Int(1)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected String"));
    }

    #[test]
    fn json_roundtrip_object() {
        let mut heap = Heap::new();
        let input = r#"{"name":"Alice","age":30}"#;
        let arg = string_arg(&mut heap, input);
        let parsed = json_parse(&[arg], &mut heap).unwrap();
        let str_result = json_to_string(&[parsed], &mut heap).unwrap();
        let output = heap_string(str_result, &heap);
        // Re-parse and compare semantically
        let original: serde_json::Value = serde_json::from_str(input).unwrap();
        let roundtripped: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn json_roundtrip_array() {
        let mut heap = Heap::new();
        let input = r#"[1,2,3]"#;
        let arg = string_arg(&mut heap, input);
        let parsed = json_parse(&[arg], &mut heap).unwrap();
        let str_result = json_to_string(&[parsed], &mut heap).unwrap();
        let output = heap_string(str_result, &heap);
        let original: serde_json::Value = serde_json::from_str(input).unwrap();
        let roundtripped: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn json_roundtrip_nested() {
        let mut heap = Heap::new();
        let input = r#"{"list":[1,true,null],"nested":{"x":"y"}}"#;
        let arg = string_arg(&mut heap, input);
        let parsed = json_parse(&[arg], &mut heap).unwrap();
        let str_result = json_to_string(&[parsed], &mut heap).unwrap();
        let output = heap_string(str_result, &heap);
        let original: serde_json::Value = serde_json::from_str(input).unwrap();
        let roundtripped: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn json_get_top_level_key() {
        let mut heap = Heap::new();
        let json = string_arg(&mut heap, r#"{"name":"Alice","age":30}"#);
        let path = string_arg(&mut heap, "name");
        let arg = tuple2(&mut heap, json, path);
        let result = json_get(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "Alice");
    }

    #[test]
    fn json_get_nested_path() {
        let mut heap = Heap::new();
        let json = string_arg(&mut heap, r#"{"a":{"b":{"c":"deep"}}}"#);
        let path = string_arg(&mut heap, "a.b.c");
        let arg = tuple2(&mut heap, json, path);
        let result = json_get(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "deep");
    }

    #[test]
    fn json_get_array_index() {
        let mut heap = Heap::new();
        let json = string_arg(&mut heap, r#"{"items":["a","b","c"]}"#);
        let path = string_arg(&mut heap, "items.1");
        let arg = tuple2(&mut heap, json, path);
        let result = json_get(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "b");
    }

    #[test]
    fn json_get_missing_key() {
        let mut heap = Heap::new();
        let json = string_arg(&mut heap, r#"{"x":1}"#);
        let path = string_arg(&mut heap, "missing");
        let arg = tuple2(&mut heap, json, path);
        let result = json_get(&[arg], &mut heap).unwrap();
        // null maps to empty string
        assert_eq!(heap_string(result, &heap), "");
    }

    #[test]
    fn json_get_numeric_value() {
        let mut heap = Heap::new();
        let json = string_arg(&mut heap, r#"{"count":42}"#);
        let path = string_arg(&mut heap, "count");
        let arg = tuple2(&mut heap, json, path);
        let result = json_get(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "42");
    }

    #[test]
    fn json_keys_object() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, r#"{"b":1,"a":2}"#);
        let result = json_keys(&[arg], &mut heap).unwrap();
        let mut keys = collect_string_list(result, &heap);
        keys.sort();
        assert_eq!(keys, vec!["a", "b"]);
    }

    #[test]
    fn json_keys_empty_object() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "{}");
        let result = json_keys(&[arg], &mut heap).unwrap();
        let keys = collect_string_list(result, &heap);
        assert!(keys.is_empty());
    }

    #[test]
    fn json_keys_not_object() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "[1,2,3]");
        let result = json_keys(&[arg], &mut heap).unwrap();
        let keys = collect_string_list(result, &heap);
        // Non-object returns empty list
        assert!(keys.is_empty());
    }

    #[test]
    fn json_length_array() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "[1,2,3]");
        let result = json_length(&[arg], &mut heap).unwrap();
        assert_int(result, 3);
    }

    #[test]
    fn json_length_object() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, r#"{"a":1,"b":2}"#);
        let result = json_length(&[arg], &mut heap).unwrap();
        assert_int(result, 2);
    }

    #[test]
    fn json_length_string() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, r#""hello""#);
        let result = json_length(&[arg], &mut heap).unwrap();
        assert_int(result, 5);
    }

    #[test]
    fn json_length_number() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "42");
        let result = json_length(&[arg], &mut heap).unwrap();
        // Numbers have length 0
        assert_int(result, 0);
    }

    #[test]
    fn json_length_empty_array() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "[]");
        let result = json_length(&[arg], &mut heap).unwrap();
        assert_int(result, 0);
    }

    #[test]
    fn json_length_malformed() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "not json");
        let result = json_length(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("json_length"));
    }
}
