use super::*;

pub(super) fn int_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => heap_alloc(heap, HeapObject::String(n.to_string())),
        _ => Err("int_to_string: expected Int".into()),
    }
}

pub(super) fn float_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => heap_alloc(heap, HeapObject::String(f.to_string())),
        _ => Err("float_to_string: expected Float".into()),
    }
}

pub(super) fn string_to_int(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s
                .trim()
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|e| format!("string_to_int: {e}")),
            _ => Err("string_to_int: expected String".into()),
        },
        _ => Err("string_to_int: expected String".into()),
    }
}

pub(super) fn char_to_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Char(c) => Ok(Value::Int(*c as i64)),
        _ => Err("char_to_int: expected Char".into()),
    }
}

pub(super) fn int_to_char(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => {
            let codepoint =
                u32::try_from(*n).map_err(|_| format!("int_to_char: invalid codepoint {n}"))?;
            char::from_u32(codepoint)
                .map(Value::Char)
                .ok_or_else(|| format!("int_to_char: invalid codepoint {n}"))
        }
        _ => Err("int_to_char: expected Int".into()),
    }
}

pub(super) fn int_to_float(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        _ => Err("int_to_float: expected Int".into()),
    }
}

pub(super) fn word_to_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Word(w) => i64::try_from(*w)
            .map(Value::Int)
            .map_err(|_| format!("word_to_int: value out of int range: {w}")),
        _ => Err("word_to_int: expected Word".into()),
    }
}

pub(super) fn int_to_word(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => u64::try_from(*n)
            .map(Value::Word)
            .map_err(|_| format!("int_to_word: value out of word range: {n}")),
        _ => Err("int_to_word: expected Int".into()),
    }
}

pub(super) fn word_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Word(w) => heap_alloc(heap, HeapObject::String(w.to_string())),
        _ => Err("word_to_string: expected Word".into()),
    }
}

pub(super) fn string_to_word(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s
                .trim()
                .parse::<u64>()
                .map(Value::Word)
                .map_err(|e| format!("string_to_word: {e}")),
            _ => Err("string_to_word: expected String".into()),
        },
        _ => Err("string_to_word: expected String".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::*;
    use super::*;

    #[test]
    fn int_to_string_positive() {
        let mut heap = Heap::new();
        let result = int_to_string(&[Value::Int(42)], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "42");
    }

    #[test]
    fn int_to_string_negative() {
        let mut heap = Heap::new();
        let result = int_to_string(&[Value::Int(-100)], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "-100");
    }

    #[test]
    fn int_to_string_zero() {
        let mut heap = Heap::new();
        let result = int_to_string(&[Value::Int(0)], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "0");
    }

    #[test]
    fn int_to_string_type_error() {
        let mut heap = Heap::new();
        let result = int_to_string(&[Value::Float(1.0)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Int"));
    }

    #[test]
    fn float_to_string_normal() {
        let mut heap = Heap::new();
        let result = float_to_string(&[Value::Float(2.75)], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "2.75");
    }

    #[test]
    fn float_to_string_integer_value() {
        let mut heap = Heap::new();
        let result = float_to_string(&[Value::Float(5.0)], &mut heap).unwrap();
        // Rust formats 5.0f64 as "5"
        let s = heap_string(result, &heap);
        assert!(s == "5" || s == "5.0");
    }

    #[test]
    fn float_to_string_nan() {
        let mut heap = Heap::new();
        let result = float_to_string(&[Value::Float(f64::NAN)], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "NaN");
    }

    #[test]
    fn float_to_string_type_error() {
        let mut heap = Heap::new();
        let result = float_to_string(&[Value::Int(1)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Float"));
    }

    #[test]
    fn string_to_int_valid() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "42");
        let result = string_to_int(&[arg], &mut heap).unwrap();
        assert_int(result, 42);
    }

    #[test]
    fn string_to_int_negative() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "-7");
        let result = string_to_int(&[arg], &mut heap).unwrap();
        assert_int(result, -7);
    }

    #[test]
    fn string_to_int_with_whitespace() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "  123  ");
        let result = string_to_int(&[arg], &mut heap).unwrap();
        assert_int(result, 123);
    }

    #[test]
    fn string_to_int_invalid() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "not_a_number");
        let result = string_to_int(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("string_to_int"));
    }

    #[test]
    fn string_to_int_type_error() {
        let mut heap = Heap::new();
        let result = string_to_int(&[Value::Int(1)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected String"));
    }

    #[test]
    fn char_to_int_ascii() {
        let mut heap = Heap::new();
        let result = char_to_int(&[Value::Char('A')], &mut heap).unwrap();
        assert_int(result, 65);
    }

    #[test]
    fn char_to_int_unicode() {
        let mut heap = Heap::new();
        let result = char_to_int(&[Value::Char('\u{20AC}')], &mut heap).unwrap();
        assert_int(result, 0x20AC);
    }

    #[test]
    fn char_to_int_type_error() {
        let mut heap = Heap::new();
        let result = char_to_int(&[Value::Int(65)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Char"));
    }

    #[test]
    fn int_to_char_valid() {
        let mut heap = Heap::new();
        let result = int_to_char(&[Value::Int(65)], &mut heap).unwrap();
        assert_char(result, 'A');
    }

    #[test]
    fn int_to_char_unicode() {
        let mut heap = Heap::new();
        let result = int_to_char(&[Value::Int(0x1F600)], &mut heap).unwrap();
        assert_char(result, '\u{1F600}');
    }

    #[test]
    fn int_to_char_invalid_codepoint() {
        let mut heap = Heap::new();
        // 0xD800 is a surrogate, invalid as a char
        let result = int_to_char(&[Value::Int(0xD800)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid codepoint"));
    }

    #[test]
    fn int_to_char_rejects_negative_codepoint() {
        let mut heap = Heap::new();
        let result = int_to_char(&[Value::Int(-1)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid codepoint"));
    }

    #[test]
    fn int_to_char_rejects_too_large_codepoint_without_wrapping() {
        let mut heap = Heap::new();
        let result = int_to_char(&[Value::Int(0x1_0000_0041)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid codepoint"));
    }

    #[test]
    fn int_to_char_type_error() {
        let mut heap = Heap::new();
        let result = int_to_char(&[Value::Char('A')], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Int"));
    }

    #[test]
    fn int_to_float_basic() {
        let mut heap = Heap::new();
        let result = int_to_float(&[Value::Int(42)], &mut heap).unwrap();
        assert_float_approx(result, 42.0, f64::EPSILON);
    }

    #[test]
    fn int_to_float_negative() {
        let mut heap = Heap::new();
        let result = int_to_float(&[Value::Int(-10)], &mut heap).unwrap();
        assert_float_approx(result, -10.0, f64::EPSILON);
    }

    #[test]
    fn int_to_float_type_error() {
        let mut heap = Heap::new();
        let result = int_to_float(&[Value::Float(1.0)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Int"));
    }

    #[test]
    fn word_to_int_valid() {
        let mut heap = Heap::new();
        let result = word_to_int(&[Value::Word(i64::MAX as u64)], &mut heap).unwrap();
        assert_int(result, i64::MAX);
    }

    #[test]
    fn word_to_int_overflow() {
        let mut heap = Heap::new();
        let result = word_to_int(&[Value::Word(u64::MAX)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("out of int range"));
    }

    #[test]
    fn word_to_int_type_error() {
        let mut heap = Heap::new();
        let result = word_to_int(&[Value::Int(1)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Word"));
    }

    #[test]
    fn int_to_word_valid() {
        let mut heap = Heap::new();
        let result = int_to_word(&[Value::Int(i64::MAX)], &mut heap).unwrap();
        match result {
            Value::Word(w) => assert_eq!(w, i64::MAX as u64),
            other => panic!("expected Word({}), got {other:?}", i64::MAX),
        }
    }

    #[test]
    fn int_to_word_negative() {
        let mut heap = Heap::new();
        let result = int_to_word(&[Value::Int(-1)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("out of word range"));
    }

    #[test]
    fn int_to_word_type_error() {
        let mut heap = Heap::new();
        let result = int_to_word(&[Value::Word(1)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Int"));
    }

    #[test]
    fn word_to_string_max() {
        let mut heap = Heap::new();
        let result = word_to_string(&[Value::Word(u64::MAX)], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), u64::MAX.to_string());
    }

    #[test]
    fn string_to_word_overflow() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "18446744073709551616");
        let result = string_to_word(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("string_to_word"));
    }

    #[test]
    fn string_to_word_invalid() {
        let mut heap = Heap::new();
        let arg = string_arg(&mut heap, "not_a_word");
        let result = string_to_word(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("string_to_word"));
    }
}
