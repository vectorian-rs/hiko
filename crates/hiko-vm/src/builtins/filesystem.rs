use super::*;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    &[
        ("read_file", read_file as BuiltinFn),
        ("read_file_bytes", read_file_bytes),
        ("write_file", write_file),
        ("file_exists", file_exists),
        ("list_dir", list_dir),
        ("remove_file", remove_file),
        ("create_dir", create_dir),
        ("is_dir", is_dir),
        ("is_file", is_file),
        ("read_file_tagged", read_file_tagged),
        ("edit_file_tagged", edit_file_tagged),
        ("glob", glob),
        ("walk_dir", walk_dir),
    ]
}

fn string_arg(args: &[Value], heap: &Heap, name: &str) -> Result<String, String> {
    match args.first() {
        Some(Value::Heap(r)) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(s.clone()),
            _ => Err(format!("{name}: expected String")),
        },
        _ => Err(format!("{name}: expected String")),
    }
}

fn cap_io<T>(
    builtin: &str,
    path: &str,
    heap: &Heap,
    f: impl Fn(&cap_std::fs::Dir, &std::path::Path) -> std::io::Result<T>,
) -> Result<T, String> {
    let candidates = heap
        .cap_candidates_for(builtin, path)
        .map_err(|e| format!("{builtin}: {e}"))?;
    let mut last_err = None;
    for candidate in candidates {
        match f(&candidate.dir, &candidate.relative_path) {
            Ok(value) => return Ok(value),
            Err(err) => last_err = Some(err),
        }
    }
    Err(format!(
        "{builtin}: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no allowed folder matched".into())
    ))
}

pub(super) fn read_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = string_arg(args, heap, "read_file")?;
    let contents = if heap.has_cap_fs_policy() {
        cap_io("read_file", &path, heap, |dir, path| {
            dir.read_to_string(path)
        })?
    } else {
        std::fs::read_to_string(&path).map_err(|e| format!("read_file: {e}"))?
    };
    heap.charge_io_bytes(contents.len() as u64)
        .map_err(|e| format!("read_file: {e}"))?;
    heap_alloc(heap, HeapObject::String(contents))
}

pub(super) fn read_file_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = string_arg(args, heap, "read_file_bytes")?;
    let contents = if heap.has_cap_fs_policy() {
        cap_io("read_file_bytes", &path, heap, |dir, path| dir.read(path))?
    } else {
        std::fs::read(&path).map_err(|e| format!("read_file_bytes: {e}"))?
    };
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
    heap.charge_io_bytes(contents.len() as u64)
        .map_err(|e| format!("write_file: {e}"))?;
    if heap.has_cap_fs_policy() {
        cap_io("write_file", &path, heap, |dir, path| {
            dir.write(path, &contents)
        })?;
    } else {
        std::fs::write(&path, &contents).map_err(|e| format!("write_file: {e}"))?;
    }
    Ok(Value::Unit)
}

pub(super) fn file_exists(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = string_arg(args, heap, "file_exists")?;
    if heap.has_cap_fs_policy() {
        let candidates = heap
            .cap_candidates_for("file_exists", &path)
            .map_err(|e| format!("file_exists: {e}"))?;
        Ok(Value::Bool(candidates.iter().any(|candidate| {
            candidate.dir.exists(&candidate.relative_path)
        })))
    } else {
        Ok(Value::Bool(std::path::Path::new(&path).exists()))
    }
}

pub(super) fn list_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = string_arg(args, heap, "list_dir")?;
    let mut read_dir = if heap.has_cap_fs_policy() {
        cap_io("list_dir", &path, heap, |dir, path| dir.read_dir(path))?
    } else {
        cap_std::fs::Dir::open_ambient_dir(".", cap_std::ambient_authority())
            .and_then(|dir| dir.read_dir(&path))
            .map_err(|e| format!("list_dir: {e}"))?
    };
    let mut entries = Vec::new();
    for entry in &mut read_dir {
        let entry = entry.map_err(|e| format!("list_dir: {e}"))?;
        let text = entry.file_name().to_string_lossy().to_string();
        heap.charge_io_bytes(text.len() as u64)
            .map_err(|e| format!("list_dir: {e}"))?;
        entries.push(heap_alloc(heap, HeapObject::String(text))?);
    }
    alloc_list(heap, entries)
}

pub(super) fn remove_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = string_arg(args, heap, "remove_file")?;
    if heap.has_cap_fs_policy() {
        cap_io("remove_file", &path, heap, |dir, path| {
            dir.remove_file(path)
        })?;
    } else {
        std::fs::remove_file(&path).map_err(|e| format!("remove_file: {e}"))?;
    }
    Ok(Value::Unit)
}

pub(super) fn create_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = string_arg(args, heap, "create_dir")?;
    if heap.has_cap_fs_policy() {
        cap_io("create_dir", &path, heap, |dir, path| {
            dir.create_dir_all(path)
        })?;
    } else {
        std::fs::create_dir_all(&path).map_err(|e| format!("create_dir: {e}"))?;
    }
    Ok(Value::Unit)
}

pub(super) fn is_dir(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = string_arg(args, heap, "is_dir")?;
    if heap.has_cap_fs_policy() {
        let candidates = heap
            .cap_candidates_for("is_dir", &path)
            .map_err(|e| format!("is_dir: {e}"))?;
        Ok(Value::Bool(candidates.iter().any(|candidate| {
            candidate.dir.is_dir(&candidate.relative_path)
        })))
    } else {
        Ok(Value::Bool(std::path::Path::new(&path).is_dir()))
    }
}

pub(super) fn is_file(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let path = string_arg(args, heap, "is_file")?;
    if heap.has_cap_fs_policy() {
        let candidates = heap
            .cap_candidates_for("is_file", &path)
            .map_err(|e| format!("is_file: {e}"))?;
        Ok(Value::Bool(candidates.iter().any(|candidate| {
            candidate.dir.is_file(&candidate.relative_path)
        })))
    } else {
        Ok(Value::Bool(std::path::Path::new(&path).is_file()))
    }
}

#[derive(Clone)]
struct TaggedFileLine {
    text: String,
    ending: String,
}

impl TaggedFileLine {
    fn tag(&self) -> String {
        fnv1a_tag_parts(&[self.text.as_str(), self.ending.as_str()])
    }
}

fn split_file_lines(content: &str) -> Vec<TaggedFileLine> {
    content
        .split_inclusive('\n')
        .map(|chunk| {
            if let Some(text) = chunk.strip_suffix("\r\n") {
                TaggedFileLine {
                    text: text.to_string(),
                    ending: "\r\n".to_string(),
                }
            } else if let Some(text) = chunk.strip_suffix('\n') {
                TaggedFileLine {
                    text: text.to_string(),
                    ending: "\n".to_string(),
                }
            } else {
                TaggedFileLine {
                    text: chunk.to_string(),
                    ending: String::new(),
                }
            }
        })
        .collect()
}

fn default_line_ending(lines: &[TaggedFileLine]) -> &str {
    lines
        .iter()
        .find(|line| !line.ending.is_empty())
        .map(|line| line.ending.as_str())
        .unwrap_or("\n")
}

fn render_file_lines(lines: &[TaggedFileLine]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&line.text);
        out.push_str(&line.ending);
    }
    out
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

    let content = if heap.has_cap_fs_policy() {
        cap_io("read_file_tagged", path, heap, |dir, path| {
            dir.read_to_string(path)
        })?
    } else {
        let checked_path = heap
            .check_fs_path_for("read_file_tagged", path)
            .map_err(|e| format!("read_file_tagged: {e}"))?;
        std::fs::read_to_string(&checked_path).map_err(|e| format!("read_file_tagged: {e}"))?
    };
    heap.charge_io_bytes(content.len() as u64)
        .map_err(|e| format!("read_file_tagged: {e}"))?;

    let lines = split_file_lines(&content);
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
        let tag = line.tag();
        out.push_str(&format!("{}:{}\t{}\n", i + 1, tag, line.text));
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

    let content = if heap.has_cap_fs_policy() {
        cap_io("edit_file_tagged", &path, heap, |dir, path| {
            dir.read_to_string(path)
        })?
    } else {
        let checked_path = heap
            .check_fs_path_for("edit_file_tagged", &path)
            .map_err(|e| format!("edit_file_tagged: {e}"))?;
        std::fs::read_to_string(&checked_path).map_err(|e| format!("edit_file_tagged: {e}"))?
    };
    heap.charge_io_bytes(content.len() as u64)
        .map_err(|e| format!("edit_file_tagged: {e}"))?;
    let lines = split_file_lines(&content);
    let hashes: Vec<String> = lines.iter().map(TaggedFileLine::tag).collect();

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

    let default_ending = default_line_ending(&lines).to_string();
    let mut result = lines.clone();
    let mut sorted_edits: Vec<&Edit> = edits.iter().collect();
    sorted_edits.sort_by(|a, b| b.line_num.cmp(&a.line_num));

    for edit in &sorted_edits {
        let idx = edit.line_num - 1;
        match edit.action {
            'R' => result[idx].text = edit.content.clone(),
            'I' => {
                let inserted_ending = if result[idx].ending.is_empty() {
                    result[idx].ending = default_ending.clone();
                    String::new()
                } else {
                    result[idx].ending.clone()
                };
                result.insert(
                    idx + 1,
                    TaggedFileLine {
                        text: edit.content.clone(),
                        ending: inserted_ending,
                    },
                );
            }
            'D' => {
                result.remove(idx);
            }
            _ => {}
        }
    }

    let final_output = render_file_lines(&result);
    heap.charge_io_bytes(final_output.len() as u64)
        .map_err(|e| format!("edit_file_tagged: {e}"))?;
    if heap.has_cap_fs_policy() {
        cap_io("edit_file_tagged", &path, heap, |dir, path| {
            dir.write(path, &final_output)
        })?;
    } else {
        let checked_path = heap
            .check_fs_path_for("edit_file_tagged", &path)
            .map_err(|e| format!("edit_file_tagged: {e}"))?;
        std::fs::write(&checked_path, &final_output)
            .map_err(|e| format!("edit_file_tagged: {e}"))?;
    }

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

    if heap.has_cap_fs_policy() {
        fn collect_cap_paths(
            root: &std::path::Path,
            cap_dir: &cap_std::fs::Dir,
            dir: &std::path::Path,
            visited: &mut std::collections::HashSet<std::path::PathBuf>,
            out: &mut Vec<std::path::PathBuf>,
        ) -> Result<(), String> {
            if !visited.insert(dir.to_path_buf()) {
                return Ok(());
            }
            let mut entries = cap_dir
                .read_dir(dir)
                .map_err(|e| format!("glob: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("glob: {e}"))?;
            entries.sort_by_key(|entry| entry.file_name());

            for entry in entries {
                let name = entry.file_name();
                let child = dir.join(&name);
                let file_type = entry.file_type().map_err(|e| format!("glob: {e}"))?;
                if file_type.is_symlink() {
                    let target = std::fs::canonicalize(root.join(&child))
                        .map_err(|e| format!("glob: {e}"))?;
                    let relative_target = target.strip_prefix(root).map_err(|_| {
                        "glob: path contains symlink outside allowed root".to_string()
                    })?;
                    out.push(target.clone());
                    if target.is_dir() {
                        let relative_target = if relative_target.as_os_str().is_empty() {
                            std::path::Path::new(".")
                        } else {
                            relative_target
                        };
                        collect_cap_paths(root, cap_dir, relative_target, visited, out)?;
                    }
                } else {
                    let display_child = child.strip_prefix(".").unwrap_or(&child);
                    let absolute = root.join(display_child);
                    out.push(absolute.clone());
                    if file_type.is_dir() {
                        collect_cap_paths(root, cap_dir, &child, visited, out)?;
                    }
                }
            }
            Ok(())
        }

        if !std::path::Path::new(&pattern).is_absolute() {
            crate::heap::validate_cap_relative_path(&pattern).map_err(|e| format!("glob: {e}"))?;
        }
        let candidates = heap
            .cap_candidates_for("glob", ".")
            .map_err(|e| format!("glob: {e}"))?;
        if std::path::Path::new(&pattern).is_absolute()
            && !candidates
                .iter()
                .any(|candidate| std::path::Path::new(&pattern).starts_with(&candidate.root))
        {
            return Err(format!(
                "glob: path '{pattern}' is outside allowed root/folders"
            ));
        }
        for candidate in candidates {
            let pattern_text = if std::path::Path::new(&pattern).is_absolute() {
                pattern.clone()
            } else {
                candidate.root.join(&pattern).to_string_lossy().to_string()
            };
            let pattern = glob::Pattern::new(&pattern_text).map_err(|e| format!("glob: {e}"))?;
            let mut all_paths = Vec::new();
            let mut visited = std::collections::HashSet::new();
            collect_cap_paths(
                &candidate.root,
                &candidate.dir,
                &candidate.relative_path,
                &mut visited,
                &mut all_paths,
            )?;
            for path in all_paths {
                if pattern.matches_path(&path) {
                    let text = path.to_string_lossy().to_string();
                    if seen.insert(text.clone()) {
                        heap.charge_io_bytes(text.len() as u64)
                            .map_err(|e| format!("glob: {e}"))?;
                        paths.push(heap_alloc(heap, HeapObject::String(text))?);
                    }
                }
            }
        }
        return alloc_list(heap, paths);
    }

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
    let mut files = Vec::new();
    if heap.has_cap_fs_policy() {
        fn walk_cap(
            root: &std::path::Path,
            cap_dir: &cap_std::fs::Dir,
            dir: &std::path::Path,
            visited: &mut std::collections::HashSet<std::path::PathBuf>,
            out: &mut Vec<String>,
        ) -> Result<(), String> {
            if !visited.insert(dir.to_path_buf()) {
                return Ok(());
            }

            let mut entries = cap_dir
                .read_dir(dir)
                .map_err(|e| format!("walk_dir: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("walk_dir: {e}"))?;
            entries.sort_by_key(|entry| entry.file_name());

            for entry in entries {
                let name = entry.file_name();
                let child = dir.join(&name);
                let file_type = entry.file_type().map_err(|e| format!("walk_dir: {e}"))?;
                if file_type.is_symlink() {
                    let target = std::fs::canonicalize(root.join(&child))
                        .map_err(|e| format!("walk_dir: {e}"))?;
                    let relative_target = target.strip_prefix(root).map_err(|_| {
                        "walk_dir: path contains symlink outside allowed root".to_string()
                    })?;
                    if target.is_dir() {
                        let relative_target = if relative_target.as_os_str().is_empty() {
                            std::path::Path::new(".")
                        } else {
                            relative_target
                        };
                        walk_cap(root, cap_dir, relative_target, visited, out)?;
                    } else if target.is_file() {
                        out.push(target.to_string_lossy().to_string());
                    }
                } else if file_type.is_dir() {
                    walk_cap(root, cap_dir, &child, visited, out)?;
                } else if file_type.is_file() {
                    let display_child = child.strip_prefix(".").unwrap_or(&child);
                    out.push(root.join(display_child).to_string_lossy().to_string());
                }
            }
            Ok(())
        }

        let candidates = heap
            .cap_candidates_for("walk_dir", &dir)
            .map_err(|e| format!("walk_dir: {e}"))?;
        let mut seen = std::collections::BTreeSet::new();
        for candidate in candidates {
            let mut candidate_files = Vec::new();
            let mut visited = std::collections::HashSet::new();
            walk_cap(
                &candidate.root,
                &candidate.dir,
                &candidate.relative_path,
                &mut visited,
                &mut candidate_files,
            )?;
            for file in candidate_files {
                if seen.insert(file.clone()) {
                    files.push(file);
                }
            }
        }
    } else {
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
        let mut visited = std::collections::HashSet::new();
        walk(&checked_dir, heap, &mut visited, &mut files)?;
    }
    let mut values = Vec::with_capacity(files.len());
    for f in files {
        heap.charge_io_bytes(f.len() as u64)
            .map_err(|e| format!("walk_dir: {e}"))?;
        values.push(heap_alloc(heap, HeapObject::String(f))?);
    }
    alloc_list(heap, values)
}
