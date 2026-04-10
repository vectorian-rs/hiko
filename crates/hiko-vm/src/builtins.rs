use crate::heap::Heap;
use crate::value::{BuiltinFn, HeapObject, Value};
use crate::vm::{TAG_CONS, TAG_NIL, values_equal};

fn alloc_list(heap: &mut Heap, elems: Vec<Value>) -> Value {
    let mut list = Value::Heap(heap.alloc(HeapObject::Data {
        tag: TAG_NIL,
        fields: vec![],
    }));
    for elem in elems.into_iter().rev() {
        list = Value::Heap(heap.alloc(HeapObject::Data {
            tag: TAG_CONS,
            fields: vec![elem, list],
        }));
    }
    list
}

pub(crate) fn bi_print(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    // Return a marker; the VM handles display
    Ok(args[0]) // VM will display this value
}

pub(crate) fn bi_println(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Ok(args[0])
}

pub(crate) fn bi_read_line(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("read_line: {e}"))?;
    if line.ends_with('\n') {
        line.pop();
    }
    Ok(Value::Heap(heap.alloc(HeapObject::String(line))))
}

pub(crate) fn bi_int_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => Ok(Value::Heap(heap.alloc(HeapObject::String(n.to_string())))),
        _ => Err("int_to_string: expected Int".into()),
    }
}

pub(crate) fn bi_float_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => Ok(Value::Heap(heap.alloc(HeapObject::String(f.to_string())))),
        _ => Err("float_to_string: expected Float".into()),
    }
}

pub(crate) fn bi_string_to_int(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

pub(crate) fn bi_char_to_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Char(c) => Ok(Value::Int(*c as i64)),
        _ => Err("char_to_int: expected Char".into()),
    }
}

pub(crate) fn bi_int_to_char(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => char::from_u32(*n as u32)
            .map(Value::Char)
            .ok_or_else(|| format!("int_to_char: invalid codepoint {n}")),
        _ => Err("int_to_char: expected Int".into()),
    }
}

pub(crate) fn bi_int_to_float(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        _ => Err("int_to_float: expected Int".into()),
    }
}

pub(crate) fn bi_string_length(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Int(s.chars().count() as i64)),
            _ => Err("string_length: expected String".into()),
        },
        _ => Err("string_length: expected String".into()),
    }
}

pub(crate) fn bi_substring(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
                Ok(Value::Heap(heap.alloc(HeapObject::String(result))))
            }
        }
        _ => Err("substring: expected (String, Int, Int)".into()),
    }
}

pub(crate) fn bi_string_contains(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

pub(crate) fn bi_trim(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Heap(
                heap.alloc(HeapObject::String(s.trim().to_string())),
            )),
            _ => Err("trim: expected String".into()),
        },
        _ => Err("trim: expected String".into()),
    }
}

pub(crate) fn bi_split(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
    let parts: Vec<Value> = s
        .split(&sep)
        .map(|p| Value::Heap(heap.alloc(HeapObject::String(p.to_string()))))
        .collect();
    let list = alloc_list(heap, parts);
    Ok(list)
}

pub(crate) fn bi_sqrt(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => Ok(Value::Float(f.sqrt())),
        _ => Err("sqrt: expected Float".into()),
    }
}

pub(crate) fn bi_abs_int(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.abs())),
        _ => Err("abs_int: expected Int".into()),
    }
}

pub(crate) fn bi_abs_float(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => Ok(Value::Float(f.abs())),
        _ => Err("abs_float: expected Float".into()),
    }
}

pub(crate) fn bi_floor(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => Ok(Value::Int(f.floor() as i64)),
        _ => Err("floor: expected Float".into()),
    }
}

pub(crate) fn bi_ceil(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Float(f) => Ok(Value::Int(f.ceil() as i64)),
        _ => Err("ceil: expected Float".into()),
    }
}

pub(crate) fn bi_read_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("read_file: expected String".into()),
        },
        _ => return Err("read_file: expected String".into()),
    };
    let contents = std::fs::read_to_string(&path).map_err(|e| format!("read_file: {e}"))?;
    Ok(Value::Heap(heap.alloc(HeapObject::String(contents))))
}

pub(crate) fn bi_write_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) => (t[0], t[1]),
            _ => return Err("write_file: expected (String, String)".into()),
        },
        _ => return Err("write_file: expected (String, String)".into()),
    };
    let path = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("write_file: expected String".into()),
        },
        _ => return Err("write_file: expected String".into()),
    };
    let contents = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("write_file: expected String".into()),
        },
        _ => return Err("write_file: expected String".into()),
    };
    std::fs::write(&path, &contents).map_err(|e| format!("write_file: {e}"))?;
    Ok(Value::Unit)
}

pub(crate) fn bi_file_exists(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Bool(std::path::Path::new(s.as_str()).exists())),
            _ => Err("file_exists: expected String".into()),
        },
        _ => Err("file_exists: expected String".into()),
    }
}

pub(crate) fn bi_list_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("list_dir: expected String".into()),
        },
        _ => return Err("list_dir: expected String".into()),
    };
    let entries: Vec<Value> = std::fs::read_dir(&path)
        .map_err(|e| format!("list_dir: {e}"))?
        .filter_map(|entry| {
            entry.ok().map(|e| {
                Value::Heap(heap.alloc(HeapObject::String(
                    e.file_name().to_string_lossy().to_string(),
                )))
            })
        })
        .collect();
    let list = alloc_list(heap, entries);
    Ok(list)
}

pub(crate) fn bi_remove_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => {
                std::fs::remove_file(s.as_str()).map_err(|e| format!("remove_file: {e}"))?;
                Ok(Value::Unit)
            }
            _ => Err("remove_file: expected String".into()),
        },
        _ => Err("remove_file: expected String".into()),
    }
}

pub(crate) fn bi_create_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => {
                std::fs::create_dir_all(s.as_str()).map_err(|e| format!("create_dir: {e}"))?;
                Ok(Value::Unit)
            }
            _ => Err("create_dir: expected String".into()),
        },
        _ => Err("create_dir: expected String".into()),
    }
}

pub(crate) fn bi_is_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Bool(std::path::Path::new(s.as_str()).is_dir())),
            _ => Err("is_dir: expected String".into()),
        },
        _ => Err("is_dir: expected String".into()),
    }
}

pub(crate) fn bi_is_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Bool(std::path::Path::new(s.as_str()).is_file())),
            _ => Err("is_file: expected String".into()),
        },
        _ => Err("is_file: expected String".into()),
    }
}

pub(crate) fn bi_path_join(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
    Ok(Value::Heap(heap.alloc(HeapObject::String(
        joined.to_string_lossy().to_string(),
    ))))
}

pub(crate) fn bi_http_get(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let url = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("http_get: expected String".into()),
        },
        _ => return Err("http_get: expected String".into()),
    };
    let response = ureq::get(&url)
        .call()
        .map_err(|e| format!("http_get: {e}"))?;

    let status = Value::Int(response.status().as_u16() as i64);

    // Collect headers as a list of (name, value) tuples
    let mut header_values: Vec<Value> = Vec::new();
    for name in response.headers().keys() {
        if let Some(val) = response.headers().get(name) {
            let k = Value::Heap(heap.alloc(HeapObject::String(name.to_string())));
            let v =
                Value::Heap(heap.alloc(HeapObject::String(val.to_str().unwrap_or("").to_string())));
            let pair = Value::Heap(heap.alloc(HeapObject::Tuple(vec![k, v])));
            header_values.push(pair);
        }
    }
    let headers = alloc_list(heap, header_values);

    let body_str = response
        .into_body()
        .read_to_string()
        .map_err(|e| format!("http_get: {e}"))?;
    let body = Value::Heap(heap.alloc(HeapObject::String(body_str)));

    // Return (status, headers, body)
    Ok(Value::Heap(
        heap.alloc(HeapObject::Tuple(vec![status, headers, body])),
    ))
}

pub(crate) fn bi_exit(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(code) => std::process::exit(*code as i32),
        _ => Err("exit: expected Int".into()),
    }
}

pub(crate) fn bi_panic(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Err(s.clone()),
            _ => Err("panic: expected String".into()),
        },
        _ => Err("panic: expected String".into()),
    }
}

pub(crate) fn bi_assert(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) => (t[0], t[1]),
            _ => return Err("assert: expected (Bool, String)".into()),
        },
        _ => return Err("assert: expected (Bool, String)".into()),
    };
    match (v0, v1) {
        (Value::Bool(true), _) => Ok(Value::Unit),
        (Value::Bool(false), Value::Heap(r)) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(msg) => Err(format!("assertion failed: {msg}")),
            _ => Err("assertion failed".into()),
        },
        _ => Err("assert: expected (Bool, String)".into()),
    }
}

pub(crate) fn bi_assert_eq(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1, v2) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 3 => (t[0], t[1], t[2]),
            _ => return Err("assert_eq: expected (a, a, String)".into()),
        },
        _ => return Err("assert_eq: expected (a, a, String)".into()),
    };
    let eq = values_equal(v0, v1, heap);
    if eq {
        Ok(Value::Unit)
    } else {
        let msg = match v2 {
            Value::Heap(r) => match heap.get(r) {
                Ok(HeapObject::String(s)) => s.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        Err(format!(
            "assertion failed: {msg}: expected {:?}, got {:?}",
            v1, v0
        ))
    }
}

pub(crate) fn builtin_entries() -> Vec<(&'static str, BuiltinFn)> {
    vec![
        ("print", bi_print),
        ("println", bi_println),
        ("read_line", bi_read_line),
        ("int_to_string", bi_int_to_string),
        ("float_to_string", bi_float_to_string),
        ("string_to_int", bi_string_to_int),
        ("char_to_int", bi_char_to_int),
        ("int_to_char", bi_int_to_char),
        ("int_to_float", bi_int_to_float),
        ("string_length", bi_string_length),
        ("substring", bi_substring),
        ("string_contains", bi_string_contains),
        ("trim", bi_trim),
        ("split", bi_split),
        ("sqrt", bi_sqrt),
        ("abs_int", bi_abs_int),
        ("abs_float", bi_abs_float),
        ("floor", bi_floor),
        ("ceil", bi_ceil),
        ("read_file", bi_read_file),
        ("write_file", bi_write_file),
        ("file_exists", bi_file_exists),
        ("list_dir", bi_list_dir),
        ("remove_file", bi_remove_file),
        ("create_dir", bi_create_dir),
        ("is_dir", bi_is_dir),
        ("is_file", bi_is_file),
        ("path_join", bi_path_join),
        ("http_get", bi_http_get),
        ("exit", bi_exit),
        ("panic", bi_panic),
        ("assert", bi_assert),
        ("assert_eq", bi_assert_eq),
    ]
}
