use crate::heap::Heap;
use crate::value::{BuiltinFn, HeapObject, Value};
use crate::vm::{TAG_CONS, TAG_NIL, values_equal};
use smallvec::smallvec;

fn alloc_list(heap: &mut Heap, elems: Vec<Value>) -> Value {
    let mut list = Value::Heap(heap.alloc(HeapObject::Data {
        tag: TAG_NIL,
        fields: smallvec![],
    }));
    for elem in elems.into_iter().rev() {
        list = Value::Heap(heap.alloc(HeapObject::Data {
            tag: TAG_CONS,
            fields: smallvec![elem, list],
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

pub(crate) fn bi_string_replace(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
    let result = s.replace(&from, &to);
    Ok(Value::Heap(heap.alloc(HeapObject::String(result))))
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

/// FNV-1a hash of a line, returned as 2-char base62.
fn fnv1a_tag(line: &str) -> [u8; 2] {
    const BASIS: u32 = 2166136261;
    const PRIME: u32 = 16777619;
    const BASE62: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut h = BASIS;
    for &b in line.as_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(PRIME);
    }
    let n = (h % 3844) as usize; // 62*62
    [BASE62[n / 62], BASE62[n % 62]]
}

/// Read a file with hashline tags. Takes (String, Int, Int) -> String.
/// (path, offset, limit) — offset 0 and limit 0 means read all.
/// Returns lines formatted as "lineno:hash\tcontent\n".
pub(crate) fn bi_read_file_tagged(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v_path, v_offset, v_limit) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 3 => (t[0], t[1], t[2]),
            _ => return Err("read_file_tagged: expected (String, Int, Int)".into()),
        },
        _ => return Err("read_file_tagged: expected (String, Int, Int)".into()),
    };
    let path = match v_path {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("read_file_tagged: expected String for path".into()),
        },
        _ => return Err("read_file_tagged: expected String for path".into()),
    };
    let offset = match v_offset {
        Value::Int(n) => n as usize,
        _ => return Err("read_file_tagged: expected Int for offset".into()),
    };
    let limit = match v_limit {
        Value::Int(n) => n as usize,
        _ => return Err("read_file_tagged: expected Int for limit".into()),
    };

    let content = std::fs::read_to_string(path).map_err(|e| format!("read_file_tagged: {e}"))?;

    let lines: Vec<&str> = content.lines().collect();
    let start = if offset > 0 {
        offset.min(lines.len())
    } else {
        0
    };
    let end = if limit > 0 {
        (start + limit).min(lines.len())
    } else {
        lines.len()
    };

    let mut out = String::new();
    for (i, line) in lines.iter().enumerate().skip(start).take(end - start) {
        let tag = fnv1a_tag(line);
        let tag_str = std::str::from_utf8(&tag).unwrap();
        out.push_str(&format!("{}:{}\t{}\n", i + 1, tag_str, line));
    }

    Ok(Value::Heap(heap.alloc(HeapObject::String(out))))
}

/// Edit a file using hashline anchors. Takes (path, edits_string) -> String.
/// Each edit line: ACTION LINE:HASH CONTENT
/// Actions: R (replace), I (insert after), D (delete)
/// Verifies hashes before applying. Returns status message.
pub(crate) fn bi_edit_file_tagged(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v_path, v_edits) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("edit_file_tagged: expected (String, String)".into()),
        },
        _ => return Err("edit_file_tagged: expected (String, String)".into()),
    };
    let path = match v_path {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("edit_file_tagged: expected String for path".into()),
        },
        _ => return Err("edit_file_tagged: expected String for path".into()),
    };
    let edits_raw = match v_edits {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("edit_file_tagged: expected String for edits".into()),
        },
        _ => return Err("edit_file_tagged: expected String for edits".into()),
    };

    // Read the file
    let content = std::fs::read_to_string(&path).map_err(|e| format!("edit_file_tagged: {e}"))?;
    let lines: Vec<&str> = content.lines().collect();

    // Compute hashes for all lines
    let hashes: Vec<String> = lines
        .iter()
        .map(|line| {
            let tag = fnv1a_tag(line);
            std::str::from_utf8(&tag).unwrap().to_string()
        })
        .collect();

    // Parse edit operations
    struct Edit {
        action: char,
        line_num: usize,
        hash: String,
        content: String,
    }

    let mut edits = Vec::new();
    for edit_line in edits_raw.lines() {
        let edit_line = edit_line.trim();
        if edit_line.is_empty() {
            continue;
        }
        let action = edit_line.chars().next().unwrap_or(' ');
        if !matches!(action, 'R' | 'I' | 'D') {
            return Err(format!(
                "edit_file_tagged: unknown action '{}' (expected R, I, or D)",
                action
            ));
        }

        let rest = edit_line[1..].trim_start();
        let (anchor, edit_content) = rest.split_once(' ').unwrap_or((rest, ""));

        let (num_str, hash) = anchor.split_once(':').ok_or_else(|| {
            format!(
                "edit_file_tagged: invalid anchor '{}' (expected LINE:HASH)",
                anchor
            )
        })?;

        let line_num: usize = num_str
            .parse()
            .map_err(|_| format!("edit_file_tagged: invalid line number '{}'", num_str))?;

        edits.push(Edit {
            action,
            line_num,
            hash: hash.to_string(),
            content: edit_content.to_string(),
        });
    }

    // Verify all hashes before applying any changes
    let mut errors = Vec::new();
    for edit in &edits {
        if edit.line_num == 0 || edit.line_num > lines.len() {
            errors.push(format!(
                "line {} out of range (file has {} lines)",
                edit.line_num,
                lines.len()
            ));
            continue;
        }
        let idx = edit.line_num - 1;
        let actual_hash = &hashes[idx];
        if actual_hash != &edit.hash {
            errors.push(format!(
                "hash mismatch at line {}: expected {}, got {} (file changed since last read)",
                edit.line_num, edit.hash, actual_hash
            ));
        }
    }

    if !errors.is_empty() {
        let msg = format!("REJECTED: {}", errors.join("; "));
        return Ok(Value::Heap(heap.alloc(HeapObject::String(msg))));
    }

    // Apply edits in reverse order to preserve line numbers
    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    let mut sorted_edits: Vec<&Edit> = edits.iter().collect();
    sorted_edits.sort_by(|a, b| b.line_num.cmp(&a.line_num));

    for edit in &sorted_edits {
        let idx = edit.line_num - 1;
        match edit.action {
            'R' => {
                result[idx] = edit.content.clone();
            }
            'I' => {
                result.insert(idx + 1, edit.content.clone());
            }
            'D' => {
                result.remove(idx);
            }
            _ => {}
        }
    }

    // Write back
    let output = result.join("\n");
    // Preserve trailing newline if original had one
    let final_output = if content.ends_with('\n') {
        format!("{output}\n")
    } else {
        output
    };
    std::fs::write(&path, &final_output).map_err(|e| format!("edit_file_tagged: {e}"))?;

    let msg = format!("Applied {} edit(s) to {}", edits.len(), path);
    Ok(Value::Heap(heap.alloc(HeapObject::String(msg))))
}

/// Glob file search. Takes a pattern string, returns a list of matching paths.
pub(crate) fn bi_glob(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let pattern = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("glob: expected String".into()),
        },
        _ => return Err("glob: expected String".into()),
    };
    let paths: Vec<Value> = glob::glob(&pattern)
        .map_err(|e| format!("glob: {e}"))?
        .filter_map(|entry| {
            entry.ok().map(|p| {
                Value::Heap(heap.alloc(HeapObject::String(p.to_string_lossy().to_string())))
            })
        })
        .collect();
    Ok(alloc_list(heap, paths))
}

/// Recursive directory walk. Takes a directory path, returns all file paths recursively.
pub(crate) fn bi_walk_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let dir = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("walk_dir: expected String".into()),
        },
        _ => return Err("walk_dir: expected String".into()),
    };
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("walk_dir: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("walk_dir: {e}"))?;
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else {
                out.push(path.to_string_lossy().to_string());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(std::path::Path::new(&dir), &mut files)?;
    let values: Vec<Value> = files
        .into_iter()
        .map(|f| Value::Heap(heap.alloc(HeapObject::String(f))))
        .collect();
    Ok(alloc_list(heap, values))
}

/// Regex match. Takes (string, pattern), returns Bool.
pub(crate) fn bi_regex_match(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
    let re = regex::Regex::new(pattern).map_err(|e| format!("regex_match: {e}"))?;
    Ok(Value::Bool(re.is_match(s)))
}

/// Regex replace. Takes (string, pattern, replacement), returns String.
pub(crate) fn bi_regex_replace(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
    let re = regex::Regex::new(pattern).map_err(|e| format!("regex_replace: {e}"))?;
    let result = re.replace_all(s, replacement).into_owned();
    Ok(Value::Heap(heap.alloc(HeapObject::String(result))))
}

// ── JSON datatype tags (must match stdlib/json.hml declaration order) ──
const TAG_JNULL: u16 = 0;
const TAG_JBOOL: u16 = 1;
const TAG_JINT: u16 = 2;
const TAG_JFLOAT: u16 = 3;
const TAG_JSTR: u16 = 4;
const TAG_JARRAY: u16 = 5;
const TAG_JOBJECT: u16 = 6;

/// Convert a serde_json::Value into a hiko json datatype value.
fn json_to_hiko(val: &serde_json::Value, heap: &mut Heap) -> Value {
    match val {
        serde_json::Value::Null => Value::Heap(heap.alloc(HeapObject::Data {
            tag: TAG_JNULL,
            fields: smallvec![],
        })),
        serde_json::Value::Bool(b) => Value::Heap(heap.alloc(HeapObject::Data {
            tag: TAG_JBOOL,
            fields: smallvec![Value::Bool(*b)],
        })),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Heap(heap.alloc(HeapObject::Data {
                    tag: TAG_JINT,
                    fields: smallvec![Value::Int(i)],
                }))
            } else if let Some(f) = n.as_f64() {
                Value::Heap(heap.alloc(HeapObject::Data {
                    tag: TAG_JFLOAT,
                    fields: smallvec![Value::Float(f)],
                }))
            } else {
                Value::Heap(heap.alloc(HeapObject::Data {
                    tag: TAG_JNULL,
                    fields: smallvec![],
                }))
            }
        }
        serde_json::Value::String(s) => {
            let sv = Value::Heap(heap.alloc(HeapObject::String(s.clone())));
            Value::Heap(heap.alloc(HeapObject::Data {
                tag: TAG_JSTR,
                fields: smallvec![sv],
            }))
        }
        serde_json::Value::Array(arr) => {
            let elems: Vec<Value> = arr.iter().map(|v| json_to_hiko(v, heap)).collect();
            let list = alloc_list(heap, elems);
            Value::Heap(heap.alloc(HeapObject::Data {
                tag: TAG_JARRAY,
                fields: smallvec![list],
            }))
        }
        serde_json::Value::Object(map) => {
            let pairs: Vec<Value> = map
                .iter()
                .map(|(k, v)| {
                    let key = Value::Heap(heap.alloc(HeapObject::String(k.clone())));
                    let val = json_to_hiko(v, heap);
                    Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![key, val])))
                })
                .collect();
            let list = alloc_list(heap, pairs);
            Value::Heap(heap.alloc(HeapObject::Data {
                tag: TAG_JOBJECT,
                fields: smallvec![list],
            }))
        }
    }
}

/// Convert a hiko json datatype value back into a JSON string.
/// Escape a string for JSON output (RFC 8259 section 7).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if c < '\u{20}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Collect elements from a hiko cons-list into a Vec<Value>.
fn collect_list(heap: &Heap, list_val: Value) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let mut cur = list_val;
    while let Value::Heap(lr) = cur {
        match heap.get(lr).map_err(|e| e.to_string())? {
            HeapObject::Data { tag, .. } if *tag == TAG_NIL => break,
            HeapObject::Data { tag, fields } if *tag == TAG_CONS && fields.len() == 2 => {
                out.push(fields[0]);
                cur = fields[1];
            }
            _ => break,
        }
    }
    Ok(out)
}

fn hiko_to_json_string(val: Value, heap: &Heap) -> Result<String, String> {
    match val {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Data { tag, fields } => match *tag {
                TAG_JNULL => Ok("null".into()),
                TAG_JBOOL => match fields.first() {
                    Some(Value::Bool(b)) => Ok(b.to_string()),
                    _ => Err("json_to_string: malformed JBool".into()),
                },
                TAG_JINT => match fields.first() {
                    Some(Value::Int(n)) => Ok(n.to_string()),
                    _ => Err("json_to_string: malformed JInt".into()),
                },
                TAG_JFLOAT => match fields.first() {
                    Some(Value::Float(f)) => Ok(f.to_string()),
                    _ => Err("json_to_string: malformed JFloat".into()),
                },
                TAG_JSTR => match fields.first() {
                    Some(Value::Heap(sr)) => match heap.get(*sr).map_err(|e| e.to_string())? {
                        HeapObject::String(s) => Ok(json_escape(s)),
                        _ => Err("json_to_string: malformed JStr".into()),
                    },
                    _ => Err("json_to_string: malformed JStr".into()),
                },
                TAG_JARRAY => {
                    let list_val = fields.first().copied().unwrap_or(Value::Unit);
                    let elems = collect_list(heap, list_val)?;
                    let items: Result<Vec<String>, String> = elems
                        .into_iter()
                        .map(|v| hiko_to_json_string(v, heap))
                        .collect();
                    Ok(format!("[{}]", items?.join(",")))
                }
                TAG_JOBJECT => {
                    let list_val = fields.first().copied().unwrap_or(Value::Unit);
                    let pairs = collect_list(heap, list_val)?;
                    let mut entries = Vec::new();
                    for pair_val in pairs {
                        match pair_val {
                            Value::Heap(tr) => match heap.get(tr).map_err(|e| e.to_string())? {
                                HeapObject::Tuple(pair) if pair.len() == 2 => {
                                    let key = match pair[0] {
                                        Value::Heap(kr) => {
                                            match heap.get(kr).map_err(|e| e.to_string())? {
                                                HeapObject::String(s) => json_escape(s),
                                                _ => return Err("json_to_string: bad key".into()),
                                            }
                                        }
                                        _ => return Err("json_to_string: bad key".into()),
                                    };
                                    let val_str = hiko_to_json_string(pair[1], heap)?;
                                    entries.push(format!("{key}:{val_str}"));
                                }
                                _ => return Err("json_to_string: bad object entry".into()),
                            },
                            _ => return Err("json_to_string: bad object entry".into()),
                        }
                    }
                    Ok(format!("{{{}}}", entries.join(",")))
                }
                _ => Err(format!("json_to_string: unknown tag {tag}")),
            },
            _ => Err("json_to_string: expected json value".into()),
        },
        _ => Err("json_to_string: expected json value".into()),
    }
}

/// Parse a JSON string into a hiko json datatype. Takes String -> json.
pub(crate) fn bi_json_parse(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let json_str = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.as_str(),
            _ => return Err("json_parse: expected String".into()),
        },
        _ => return Err("json_parse: expected String".into()),
    };
    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("json_parse: {e}"))?;
    Ok(json_to_hiko(&parsed, heap))
}

/// Serialize a hiko json value back to a JSON string. Takes json -> String.
pub(crate) fn bi_json_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let result = hiko_to_json_string(args[0], heap)?;
    Ok(Value::Heap(heap.alloc(HeapObject::String(result))))
}

/// Parse a JSON string and get a value by key or path.
/// Takes (json_string, key_or_path) -> String.
/// Path supports dot notation: "foo.bar.baz" or array indexing: "items.0.name"
pub(crate) fn bi_json_get(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

    Ok(Value::Heap(heap.alloc(HeapObject::String(result))))
}

/// Get all keys from a JSON object. Takes json_string -> String list.
pub(crate) fn bi_json_keys(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
        serde_json::Value::Object(map) => map
            .keys()
            .map(|k| Value::Heap(heap.alloc(HeapObject::String(k.clone()))))
            .collect(),
        _ => Vec::new(),
    };

    Ok(alloc_list(heap, keys))
}

/// Get the length of a JSON array. Takes json_string -> Int.
pub(crate) fn bi_json_length(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

/// Extract a string argument from args[0].
fn extract_string_arg(args: &[Value], heap: &Heap, name: &str) -> Result<String, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(s.clone()),
            _ => Err(format!("{name}: expected String")),
        },
        _ => Err(format!("{name}: expected String")),
    }
}

/// Collect headers from an http::HeaderMap into a hiko list of (String, String) tuples.
fn collect_headers(header_map: &ureq::http::HeaderMap, heap: &mut Heap) -> Value {
    let mut header_values: Vec<Value> = Vec::new();
    for (name, val) in header_map.iter() {
        let k = Value::Heap(heap.alloc(HeapObject::String(name.to_string())));
        let v = Value::Heap(heap.alloc(HeapObject::String(val.to_str().unwrap_or("").to_string())));
        let pair = Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![k, v])));
        header_values.push(pair);
    }
    alloc_list(heap, header_values)
}

/// Convenience: HTTP GET, returns (status, headers, body_string).
pub(crate) fn bi_http_get(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let url = extract_string_arg(args, heap, "http_get")?;
    let response = ureq::get(&url)
        .call()
        .map_err(|e| format!("http_get: {e}"))?;
    let status = Value::Int(response.status().as_u16() as i64);
    let headers = collect_headers(response.headers(), heap);
    let body_str = response
        .into_body()
        .read_to_string()
        .map_err(|e| format!("http_get: {e}"))?;
    let body = Value::Heap(heap.alloc(HeapObject::String(body_str)));
    Ok(Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![
        status, headers, body
    ]))))
}

/// Extract (method, url, request_headers, body) from a 4-tuple argument.
fn extract_http_args(
    args: &[Value],
    heap: &Heap,
    name: &str,
) -> Result<(String, String, Vec<(String, String)>, String), String> {
    let (v0, v1, v2, v3) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 4 => (t[0], t[1], t[2], t[3]),
            _ => {
                return Err(format!(
                    "{name}: expected (String, String, (String * String) list, String)"
                ));
            }
        },
        _ => {
            return Err(format!(
                "{name}: expected (String, String, (String * String) list, String)"
            ));
        }
    };
    let method = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err(format!("{name}: expected String for method")),
        },
        _ => return Err(format!("{name}: expected String for method")),
    };
    let url = match v1 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err(format!("{name}: expected String for url")),
        },
        _ => return Err(format!("{name}: expected String for url")),
    };
    // Walk the headers list: [(String, String)]
    let mut req_headers = Vec::new();
    let header_elems = collect_list(heap, v2)?;
    for elem in header_elems {
        match elem {
            Value::Heap(tr) => match heap.get(tr).map_err(|e| e.to_string())? {
                HeapObject::Tuple(pair) if pair.len() == 2 => {
                    let k = match pair[0] {
                        Value::Heap(kr) => match heap.get(kr).map_err(|e| e.to_string())? {
                            HeapObject::String(s) => s.clone(),
                            _ => return Err(format!("{name}: header key must be String")),
                        },
                        _ => return Err(format!("{name}: header key must be String")),
                    };
                    let v = match pair[1] {
                        Value::Heap(vr) => match heap.get(vr).map_err(|e| e.to_string())? {
                            HeapObject::String(s) => s.clone(),
                            _ => return Err(format!("{name}: header value must be String")),
                        },
                        _ => return Err(format!("{name}: header value must be String")),
                    };
                    req_headers.push((k, v));
                }
                _ => return Err(format!("{name}: headers must be (String, String) list")),
            },
            _ => return Err(format!("{name}: headers must be (String, String) list")),
        }
    }
    let body = match v3 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err(format!("{name}: expected String for body")),
        },
        _ => return Err(format!("{name}: expected String for body")),
    };
    Ok((method, url, req_headers, body))
}

/// Perform an HTTP request. Returns (status, response_headers as hiko value, body reader).
fn do_http_request(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &str,
    name: &str,
    heap: &mut Heap,
) -> Result<(Value, Value, Box<dyn std::io::Read + Send>), String> {
    macro_rules! call_no_body {
        ($req_fn:expr) => {{
            let mut req = $req_fn(url);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.call().map_err(|e| format!("{name}: {e}"))
        }};
    }
    macro_rules! call_with_body {
        ($req_fn:expr) => {{
            let mut req = $req_fn(url);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.send(body.as_bytes())
                .map_err(|e| format!("{name}: {e}"))
        }};
    }

    let response = match method.to_uppercase().as_str() {
        "GET" => call_no_body!(ureq::get),
        "HEAD" => call_no_body!(ureq::head),
        "DELETE" => call_no_body!(ureq::delete),
        "POST" => call_with_body!(ureq::post),
        "PUT" => call_with_body!(ureq::put),
        "PATCH" => call_with_body!(ureq::patch),
        other => return Err(format!("{name}: unsupported method '{other}'")),
    }?;

    let status = Value::Int(response.status().as_u16() as i64);
    let resp_headers = collect_headers(response.headers(), heap);
    let reader = Box::new(response.into_body().into_reader()) as Box<dyn std::io::Read + Send>;
    Ok((status, resp_headers, reader))
}

/// General HTTP request. Takes (method, url, headers, body) -> (status, response_headers, body_string).
pub(crate) fn bi_http(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (method, url, req_headers, body) = extract_http_args(args, heap, "http")?;
    let (status, resp_headers, mut reader) =
        do_http_request(&method, &url, &req_headers, &body, "http", heap)?;
    let mut body_str = String::new();
    std::io::Read::read_to_string(&mut reader, &mut body_str).map_err(|e| format!("http: {e}"))?;
    let resp_body = Value::Heap(heap.alloc(HeapObject::String(body_str)));
    Ok(Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![
        status,
        resp_headers,
        resp_body
    ]))))
}

/// General HTTP request with JSON response parsing. Returns (status, headers, json).
pub(crate) fn bi_http_json(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (method, url, req_headers, body) = extract_http_args(args, heap, "http_json")?;
    let (status, resp_headers, mut reader) =
        do_http_request(&method, &url, &req_headers, &body, "http_json", heap)?;
    let mut body_str = String::new();
    std::io::Read::read_to_string(&mut reader, &mut body_str)
        .map_err(|e| format!("http_json: {e}"))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body_str).map_err(|e| format!("http_json: {e}"))?;
    let resp_body = json_to_hiko(&parsed, heap);
    Ok(Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![
        status,
        resp_headers,
        resp_body
    ]))))
}

/// General HTTP request with msgpack response decoding. Returns (status, headers, json).
pub(crate) fn bi_http_msgpack(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (method, url, req_headers, body) = extract_http_args(args, heap, "http_msgpack")?;
    let (status, resp_headers, reader) =
        do_http_request(&method, &url, &req_headers, &body, "http_msgpack", heap)?;
    let parsed: serde_json::Value =
        rmp_serde::from_read(reader).map_err(|e| format!("http_msgpack: {e}"))?;
    let resp_body = json_to_hiko(&parsed, heap);
    Ok(Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![
        status,
        resp_headers,
        resp_body
    ]))))
}

// ── Cryptographic RNG (OS entropy via dryoc) ────────────────────────

/// random_bytes : Int -> Bytes — cryptographically secure random bytes from OS entropy.
pub(crate) fn bi_random_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) if *n >= 0 => {
            let buf = dryoc::rng::randombytes_buf(*n as usize);
            Ok(Value::Heap(heap.alloc(HeapObject::Bytes(buf))))
        }
        Value::Int(_) => Err("random_bytes: length must be non-negative".into()),
        _ => Err("random_bytes: expected Int".into()),
    }
}

// ── RNG builtins (PCG-XSH-RR-64/32) ─────────────────────────────────

fn pcg_next(state: u64, inc: u64) -> (u32, u64) {
    let old_state = state;
    let new_state = old_state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(inc);
    let xorshifted = (((old_state >> 18) ^ old_state) >> 27) as u32;
    let rot = (old_state >> 59) as u32;
    (xorshifted.rotate_right(rot), new_state)
}

/// rng_seed : Bytes -> rng
pub(crate) fn bi_rng_seed(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let seed_bytes = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => b.clone(),
            _ => return Err("rng_seed: expected Bytes".into()),
        },
        _ => return Err("rng_seed: expected Bytes".into()),
    };
    let mut state: u64 = 0;
    let mut inc: u64 = 1;
    for (i, &b) in seed_bytes.iter().enumerate() {
        if i % 2 == 0 {
            state = state.wrapping_mul(31).wrapping_add(b as u64);
        } else {
            inc = inc.wrapping_mul(37).wrapping_add(b as u64);
        }
    }
    inc |= 1; // PCG requires odd increment
    let (_, state) = pcg_next(state, inc);
    let (_, state) = pcg_next(state, inc);
    Ok(Value::Heap(heap.alloc(HeapObject::Rng { state, inc })))
}

/// rng_bytes : (rng, Int) -> (Bytes, rng)
pub(crate) fn bi_rng_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("rng_bytes: expected (rng, Int)".into()),
        },
        _ => return Err("rng_bytes: expected (rng, Int)".into()),
    };
    let (mut state, inc) = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Rng { state, inc } => (*state, *inc),
            _ => return Err("rng_bytes: expected rng".into()),
        },
        _ => return Err("rng_bytes: expected rng".into()),
    };
    let n = match v1 {
        Value::Int(n) if n >= 0 => n as usize,
        _ => return Err("rng_bytes: expected non-negative Int".into()),
    };
    let mut output = Vec::with_capacity(n);
    while output.len() < n {
        let (word, new_state) = pcg_next(state, inc);
        state = new_state;
        for &b in &word.to_le_bytes() {
            if output.len() >= n {
                break;
            }
            output.push(b);
        }
    }
    let bytes_val = Value::Heap(heap.alloc(HeapObject::Bytes(output)));
    let rng_val = Value::Heap(heap.alloc(HeapObject::Rng { state, inc }));
    Ok(Value::Heap(
        heap.alloc(HeapObject::Tuple(smallvec![bytes_val, rng_val])),
    ))
}

/// rng_int : (rng, Int) -> (Int, rng)
pub(crate) fn bi_rng_int(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("rng_int: expected (rng, Int)".into()),
        },
        _ => return Err("rng_int: expected (rng, Int)".into()),
    };
    let (state, inc) = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Rng { state, inc } => (*state, *inc),
            _ => return Err("rng_int: expected rng".into()),
        },
        _ => return Err("rng_int: expected rng".into()),
    };
    let bound = match v1 {
        Value::Int(n) if n > 0 => n,
        _ => return Err("rng_int: bound must be positive".into()),
    };
    let (word, new_state) = pcg_next(state, inc);
    let value = (word as i64).abs() % bound;
    let rng_val = Value::Heap(heap.alloc(HeapObject::Rng {
        state: new_state,
        inc,
    }));
    Ok(Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![
        Value::Int(value),
        rng_val
    ]))))
}

// ── Bytes builtins ───────────────────────────────────────────────────

pub(crate) fn bi_bytes_length(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => Ok(Value::Int(b.len() as i64)),
            _ => Err("bytes_length: expected Bytes".into()),
        },
        _ => Err("bytes_length: expected Bytes".into()),
    }
}

pub(crate) fn bi_bytes_to_string(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => Ok(Value::Heap(
                heap.alloc(HeapObject::String(String::from_utf8_lossy(b).into_owned())),
            )),
            _ => Err("bytes_to_string: expected Bytes".into()),
        },
        _ => Err("bytes_to_string: expected Bytes".into()),
    }
}

pub(crate) fn bi_string_to_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Heap(
                heap.alloc(HeapObject::Bytes(s.as_bytes().to_vec())),
            )),
            _ => Err("string_to_bytes: expected String".into()),
        },
        _ => Err("string_to_bytes: expected String".into()),
    }
}

pub(crate) fn bi_bytes_get(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
        Value::Int(n) => n as usize,
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

pub(crate) fn bi_bytes_slice(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
        Value::Int(n) => n as usize,
        _ => return Err("bytes_slice: expected Int for start".into()),
    };
    let len = match v2 {
        Value::Int(n) => n as usize,
        _ => return Err("bytes_slice: expected Int for length".into()),
    };
    let end = (start + len).min(bytes.len());
    let start = start.min(bytes.len());
    Ok(Value::Heap(
        heap.alloc(HeapObject::Bytes(bytes[start..end].to_vec())),
    ))
}

/// HTTP request returning raw bytes body.
pub(crate) fn bi_http_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (method, url, req_headers, body) = extract_http_args(args, heap, "http_bytes")?;
    let (status, resp_headers, mut reader) =
        do_http_request(&method, &url, &req_headers, &body, "http_bytes", heap)?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf).map_err(|e| format!("http_bytes: {e}"))?;
    let resp_body = Value::Heap(heap.alloc(HeapObject::Bytes(buf)));
    Ok(Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![
        status,
        resp_headers,
        resp_body
    ]))))
}

/// Execute a command directly (no shell). Takes (String, String list) -> (Int, String, String).
/// The allowed-commands check is done by the VM before calling this.
pub(crate) fn bi_exec(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("exec: expected (String, String list)".into()),
        },
        _ => return Err("exec: expected (String, String list)".into()),
    };

    let command = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("exec: expected String for command".into()),
        },
        _ => return Err("exec: expected String for command".into()),
    };

    // Walk the linked list of args
    let mut cmd_args: Vec<String> = Vec::new();
    let mut cur = v1;
    loop {
        match cur {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Data { tag, .. } if *tag == TAG_NIL => break,
                HeapObject::Data { tag, fields } if *tag == TAG_CONS && fields.len() == 2 => {
                    match fields[0] {
                        Value::Heap(sr) => match heap.get(sr).map_err(|e| e.to_string())? {
                            HeapObject::String(s) => cmd_args.push(s.clone()),
                            _ => return Err("exec: args must be strings".into()),
                        },
                        _ => return Err("exec: args must be strings".into()),
                    }
                    cur = fields[1];
                }
                _ => return Err("exec: expected String list for args".into()),
            },
            _ => return Err("exec: expected String list for args".into()),
        }
    }

    let output = std::process::Command::new(&command)
        .args(&cmd_args)
        .output()
        .map_err(|e| format!("exec: {e}"))?;

    let exit_code = Value::Int(output.status.code().unwrap_or(-1) as i64);
    let stdout = Value::Heap(heap.alloc(HeapObject::String(
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )));
    let stderr = Value::Heap(heap.alloc(HeapObject::String(
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )));

    Ok(Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![
        exit_code, stdout, stderr
    ]))))
}

pub(crate) fn bi_getenv(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

pub(crate) fn bi_starts_with(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

pub(crate) fn bi_ends_with(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

pub(crate) fn bi_to_upper(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Heap(
                heap.alloc(HeapObject::String(s.to_uppercase())),
            )),
            _ => Err("to_upper: expected String".into()),
        },
        _ => Err("to_upper: expected String".into()),
    }
}

pub(crate) fn bi_to_lower(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(Value::Heap(
                heap.alloc(HeapObject::String(s.to_lowercase())),
            )),
            _ => Err("to_lower: expected String".into()),
        },
        _ => Err("to_lower: expected String".into()),
    }
}

pub(crate) fn bi_epoch(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("epoch: {e}"))?
        .as_secs();
    Ok(Value::Int(secs as i64))
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
        ("string_replace", bi_string_replace),
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
        ("read_file_tagged", bi_read_file_tagged),
        ("edit_file_tagged", bi_edit_file_tagged),
        ("glob", bi_glob),
        ("walk_dir", bi_walk_dir),
        ("regex_match", bi_regex_match),
        ("regex_replace", bi_regex_replace),
        ("json_parse", bi_json_parse),
        ("json_to_string", bi_json_to_string),
        ("json_get", bi_json_get),
        ("json_keys", bi_json_keys),
        ("json_length", bi_json_length),
        ("http_get", bi_http_get),
        ("http", bi_http),
        ("http_json", bi_http_json),
        ("http_msgpack", bi_http_msgpack),
        ("http_bytes", bi_http_bytes),
        ("bytes_length", bi_bytes_length),
        ("bytes_to_string", bi_bytes_to_string),
        ("string_to_bytes", bi_string_to_bytes),
        ("bytes_get", bi_bytes_get),
        ("bytes_slice", bi_bytes_slice),
        ("random_bytes", bi_random_bytes),
        ("rng_seed", bi_rng_seed),
        ("rng_bytes", bi_rng_bytes),
        ("rng_int", bi_rng_int),
        ("getenv", bi_getenv),
        ("starts_with", bi_starts_with),
        ("ends_with", bi_ends_with),
        ("to_upper", bi_to_upper),
        ("to_lower", bi_to_lower),
        ("epoch", bi_epoch),
        ("exec", bi_exec),
        ("exit", bi_exit),
        ("panic", bi_panic),
        ("assert", bi_assert),
        ("assert_eq", bi_assert_eq),
    ]
}
