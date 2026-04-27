use super::*;
use ::regex::Regex;
use std::borrow::Cow;
use std::mem::size_of;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    &[
        ("regex_match", regex_match as BuiltinFn),
        ("regex_replace", regex_replace),
    ]
}

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
    let replaced = re.replace_all(s, replacement);
    let output_len = match &replaced {
        Cow::Borrowed(text) => text.len(),
        Cow::Owned(text) => text.len(),
    };
    heap.ensure_can_allocate_bytes(size_of::<HeapObject>().saturating_add(output_len))
        .map_err(|e| format!("regex_replace: {e}"))?;
    heap_alloc(heap, HeapObject::String(replaced.into_owned()))
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::*;
    use super::*;

    #[test]
    fn regex_match_basic_true() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let pat = string_arg(&mut heap, r"hello \w+");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn regex_match_basic_false() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let pat = string_arg(&mut heap, r"^\d+$");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap).unwrap();
        assert_bool(result, false);
    }

    #[test]
    fn regex_match_partial() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "abc123def");
        let pat = string_arg(&mut heap, r"\d+");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn regex_match_empty_string() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "");
        let pat = string_arg(&mut heap, r"^$");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn regex_match_invalid_pattern() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello");
        let pat = string_arg(&mut heap, r"[invalid");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("regex_match"));
    }

    #[test]
    fn regex_match_type_error() {
        let mut heap = Heap::new();
        let result = regex_match(&[Value::Int(42)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected"));
    }

    #[test]
    fn regex_replace_basic() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello 123 world 456");
        let pat = string_arg(&mut heap, r"\d+");
        let rep = string_arg(&mut heap, "NUM");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello NUM world NUM");
    }

    #[test]
    fn regex_replace_no_match() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello");
        let pat = string_arg(&mut heap, r"\d+");
        let rep = string_arg(&mut heap, "X");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello");
    }

    #[test]
    fn regex_replace_with_capture_group() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "foo bar baz");
        let pat = string_arg(&mut heap, r"(\w+)");
        let rep = string_arg(&mut heap, "[$1]");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "[foo] [bar] [baz]");
    }

    #[test]
    fn regex_replace_invalid_pattern() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "test");
        let pat = string_arg(&mut heap, r"(unclosed");
        let rep = string_arg(&mut heap, "X");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("regex_replace"));
    }

    #[test]
    fn regex_replace_empty_pattern() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "abc");
        let pat = string_arg(&mut heap, "");
        let rep = string_arg(&mut heap, "X");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap).unwrap();
        // Empty pattern matches every position: before each char and at end
        assert_eq!(heap_string(result, &heap), "XaXbXcX");
    }

    #[test]
    fn regex_replace_checks_output_against_memory_limit() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "abc");
        let pat = string_arg(&mut heap, "");
        let rep = string_arg(&mut heap, "XXXX");
        let arg = tuple3(&mut heap, s, pat, rep);
        heap.set_max_bytes(heap.live_bytes() + size_of::<HeapObject>() + 18);

        let err = regex_replace(&[arg], &mut heap).unwrap_err();
        assert!(err.contains("regex_replace: memory limit exceeded"));
    }
}
