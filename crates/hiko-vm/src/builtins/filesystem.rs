use super::*;

pub(super) fn read_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("read_file: expected String".into()),
        },
        _ => return Err("read_file: expected String".into()),
    };
    let checked_path = heap
        .check_fs_path_for("read_file", &path)
        .map_err(|e| format!("read_file: {e}"))?;
    let contents = std::fs::read_to_string(&checked_path).map_err(|e| format!("read_file: {e}"))?;
    heap.charge_io_bytes(contents.len() as u64)
        .map_err(|e| format!("read_file: {e}"))?;
    heap_alloc(heap, HeapObject::String(contents))
}

pub(super) fn read_file_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("read_file_bytes: expected String".into()),
        },
        _ => return Err("read_file_bytes: expected String".into()),
    };
    let checked_path = heap
        .check_fs_path_for("read_file_bytes", &path)
        .map_err(|e| format!("read_file_bytes: {e}"))?;
    let contents = std::fs::read(&checked_path).map_err(|e| format!("read_file_bytes: {e}"))?;
    heap.charge_io_bytes(contents.len() as u64)
        .map_err(|e| format!("read_file_bytes: {e}"))?;
    heap_alloc(heap, HeapObject::Bytes(contents))
}

pub(super) fn write_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
    let checked_path = heap
        .check_fs_path_for("write_file", &path)
        .map_err(|e| format!("write_file: {e}"))?;
    heap.charge_io_bytes(contents.len() as u64)
        .map_err(|e| format!("write_file: {e}"))?;
    std::fs::write(&checked_path, &contents).map_err(|e| format!("write_file: {e}"))?;
    Ok(Value::Unit)
}

pub(super) fn file_exists(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => {
                let checked_path = heap
                    .check_fs_path_for("file_exists", s)
                    .map_err(|e| format!("file_exists: {e}"))?;
                Ok(Value::Bool(checked_path.exists()))
            }
            _ => Err("file_exists: expected String".into()),
        },
        _ => Err("file_exists: expected String".into()),
    }
}

pub(super) fn list_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("list_dir: expected String".into()),
        },
        _ => return Err("list_dir: expected String".into()),
    };
    let checked_path = heap
        .check_fs_path_for("list_dir", &path)
        .map_err(|e| format!("list_dir: {e}"))?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&checked_path).map_err(|e| format!("list_dir: {e}"))? {
        let entry = entry.map_err(|e| format!("list_dir: {e}"))?;
        let text = entry.file_name().to_string_lossy().to_string();
        heap.charge_io_bytes(text.len() as u64)
            .map_err(|e| format!("list_dir: {e}"))?;
        entries.push(heap_alloc(heap, HeapObject::String(text))?);
    }
    alloc_list(heap, entries)
}

pub(super) fn remove_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => {
                let checked_path = heap
                    .check_fs_path_for("remove_file", s)
                    .map_err(|e| format!("remove_file: {e}"))?;
                std::fs::remove_file(&checked_path).map_err(|e| format!("remove_file: {e}"))?;
                Ok(Value::Unit)
            }
            _ => Err("remove_file: expected String".into()),
        },
        _ => Err("remove_file: expected String".into()),
    }
}

pub(super) fn create_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => {
                let checked_path = heap
                    .check_fs_path_for("create_dir", s)
                    .map_err(|e| format!("create_dir: {e}"))?;
                std::fs::create_dir_all(&checked_path).map_err(|e| format!("create_dir: {e}"))?;
                Ok(Value::Unit)
            }
            _ => Err("create_dir: expected String".into()),
        },
        _ => Err("create_dir: expected String".into()),
    }
}

pub(super) fn is_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => {
                let checked_path = heap
                    .check_fs_path_for("is_dir", s)
                    .map_err(|e| format!("is_dir: {e}"))?;
                Ok(Value::Bool(checked_path.is_dir()))
            }
            _ => Err("is_dir: expected String".into()),
        },
        _ => Err("is_dir: expected String".into()),
    }
}

pub(super) fn is_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => {
                let checked_path = heap
                    .check_fs_path_for("is_file", s)
                    .map_err(|e| format!("is_file: {e}"))?;
                Ok(Value::Bool(checked_path.is_file()))
            }
            _ => Err("is_file: expected String".into()),
        },
        _ => Err("is_file: expected String".into()),
    }
}

pub(super) fn read_file_tagged(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
        Value::Int(n) if n >= 0 => n as usize,
        Value::Int(n) => {
            return Err(format!(
                "read_file_tagged: offset must be non-negative, got {n}"
            ));
        }
        _ => return Err("read_file_tagged: expected Int for offset".into()),
    };
    let limit = match v_limit {
        Value::Int(n) if n >= 0 => n as usize,
        Value::Int(n) => {
            return Err(format!(
                "read_file_tagged: limit must be non-negative, got {n}"
            ));
        }
        _ => return Err("read_file_tagged: expected Int for limit".into()),
    };

    let checked_path = heap
        .check_fs_path_for("read_file_tagged", path)
        .map_err(|e| format!("read_file_tagged: {e}"))?;
    let content =
        std::fs::read_to_string(&checked_path).map_err(|e| format!("read_file_tagged: {e}"))?;
    heap.charge_io_bytes(content.len() as u64)
        .map_err(|e| format!("read_file_tagged: {e}"))?;

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
        out.push_str(&format!("{}:{}\t{}\n", i + 1, tag, line));
    }

    heap_alloc(heap, HeapObject::String(out))
}

pub(super) fn edit_file_tagged(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

    let checked_path = heap
        .check_fs_path_for("edit_file_tagged", &path)
        .map_err(|e| format!("edit_file_tagged: {e}"))?;
    let content =
        std::fs::read_to_string(&checked_path).map_err(|e| format!("edit_file_tagged: {e}"))?;
    heap.charge_io_bytes(content.len() as u64)
        .map_err(|e| format!("edit_file_tagged: {e}"))?;
    let lines: Vec<&str> = content.lines().collect();
    let hashes: Vec<String> = lines.iter().map(|line| fnv1a_tag(line)).collect();

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

    let mut errors = Vec::new();
    let mut edited_lines = std::collections::HashSet::new();
    for edit in &edits {
        if edit.line_num == 0 || edit.line_num > lines.len() {
            errors.push(format!(
                "line {} out of range (file has {} lines)",
                edit.line_num,
                lines.len()
            ));
            continue;
        }
        if !is_valid_fnv1a_tag(&edit.hash) {
            errors.push(format!(
                "invalid hash at line {}: expected 16 hex chars, got {}",
                edit.line_num, edit.hash
            ));
            continue;
        }
        if !edited_lines.insert(edit.line_num) {
            errors.push(format!("duplicate edit for line {}", edit.line_num));
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
        return heap_alloc(heap, HeapObject::String(msg));
    }

    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    let mut sorted_edits: Vec<&Edit> = edits.iter().collect();
    sorted_edits.sort_by(|a, b| b.line_num.cmp(&a.line_num));

    for edit in &sorted_edits {
        let idx = edit.line_num - 1;
        match edit.action {
            'R' => result[idx] = edit.content.clone(),
            'I' => result.insert(idx + 1, edit.content.clone()),
            'D' => {
                result.remove(idx);
            }
            _ => {}
        }
    }

    let output = result.join("\n");
    let final_output = if content.ends_with('\n') {
        format!("{output}\n")
    } else {
        output
    };
    heap.charge_io_bytes(final_output.len() as u64)
        .map_err(|e| format!("edit_file_tagged: {e}"))?;
    std::fs::write(&checked_path, &final_output).map_err(|e| format!("edit_file_tagged: {e}"))?;

    let msg = format!("Applied {} edit(s) to {}", edits.len(), path);
    heap_alloc(heap, HeapObject::String(msg))
}

pub(super) fn glob(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let pattern = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("glob: expected String".into()),
        },
        _ => return Err("glob: expected String".into()),
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut paths = Vec::new();
    let roots = heap
        .allowed_fs_folders_for("glob")
        .map_err(|e| format!("glob: {e}"))?;

    if roots.is_empty() {
        for entry in glob::glob(&pattern).map_err(|e| format!("glob: {e}"))? {
            let path = entry.map_err(|e| format!("glob: {e}"))?;
            let text = path.to_string_lossy().to_string();
            if seen.insert(text.clone()) {
                heap.charge_io_bytes(text.len() as u64)
                    .map_err(|e| format!("glob: {e}"))?;
                paths.push(heap_alloc(heap, HeapObject::String(text))?);
            }
        }
    } else if std::path::Path::new(&pattern).is_absolute() {
        for entry in glob::glob(&pattern).map_err(|e| format!("glob: {e}"))? {
            let path = entry.map_err(|e| format!("glob: {e}"))?;
            let checked = heap
                .check_fs_path_for("glob", path.to_string_lossy().as_ref())
                .map_err(|e| format!("glob: {e}"))?;
            let text = checked.to_string_lossy().to_string();
            if seen.insert(text.clone()) {
                heap.charge_io_bytes(text.len() as u64)
                    .map_err(|e| format!("glob: {e}"))?;
                paths.push(heap_alloc(heap, HeapObject::String(text))?);
            }
        }
    } else {
        for root in roots {
            let rooted_pattern = root.join(&pattern).to_string_lossy().to_string();
            for entry in glob::glob(&rooted_pattern).map_err(|e| format!("glob: {e}"))? {
                let path = entry.map_err(|e| format!("glob: {e}"))?;
                let checked = heap
                    .check_fs_path_for("glob", path.to_string_lossy().as_ref())
                    .map_err(|e| format!("glob: {e}"))?;
                let text = checked.to_string_lossy().to_string();
                if seen.insert(text.clone()) {
                    heap.charge_io_bytes(text.len() as u64)
                        .map_err(|e| format!("glob: {e}"))?;
                    paths.push(heap_alloc(heap, HeapObject::String(text))?);
                }
            }
        }
    }
    alloc_list(heap, paths)
}

pub(super) fn walk_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let dir = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.clone(),
            _ => return Err("walk_dir: expected String".into()),
        },
        _ => return Err("walk_dir: expected String".into()),
    };
    let checked_dir = heap
        .check_fs_path_for("walk_dir", &dir)
        .map_err(|e| format!("walk_dir: {e}"))?;
    fn walk(
        dir: &std::path::Path,
        heap: &Heap,
        visited: &mut std::collections::HashSet<std::path::PathBuf>,
        out: &mut Vec<String>,
    ) -> Result<(), String> {
        let canonical_dir = std::fs::canonicalize(dir).map_err(|e| format!("walk_dir: {e}"))?;
        if !visited.insert(canonical_dir) {
            return Ok(());
        }

        let mut entries = std::fs::read_dir(dir)
            .map_err(|e| format!("walk_dir: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("walk_dir: {e}"))?;
        entries.sort_by_key(|entry| entry.path());

        for entry in entries {
            let checked_path = heap
                .check_fs_path_for("walk_dir", entry.path().to_string_lossy().as_ref())
                .map_err(|e| format!("walk_dir: {e}"))?;
            if checked_path.is_dir() {
                walk(&checked_path, heap, visited, out)?;
            } else if checked_path.is_file() {
                out.push(checked_path.to_string_lossy().to_string());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    let mut visited = std::collections::HashSet::new();
    walk(&checked_dir, heap, &mut visited, &mut files)?;
    let mut values = Vec::with_capacity(files.len());
    for f in files {
        heap.charge_io_bytes(f.len() as u64)
            .map_err(|e| format!("walk_dir: {e}"))?;
        values.push(heap_alloc(heap, HeapObject::String(f))?);
    }
    alloc_list(heap, values)
}
