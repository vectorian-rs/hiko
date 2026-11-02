use super::*;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    &[
        ("print", print as BuiltinFn),
        ("println", println),
        ("eprint", eprint),
        ("eprintln", eprintln),
        ("read_line", read_line),
        ("read_stdin", read_stdin),
    ]
}

pub(super) fn print(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Ok(args[0])
}

pub(super) fn println(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Ok(args[0])
}

pub(super) fn eprint(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    write_stderr(args, heap, "eprint", false)
}

pub(super) fn eprintln(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    write_stderr(args, heap, "eprintln", true)
}

/// Write the string in `args[0]` to the host's stderr, charged as I/O.
fn write_stderr(
    args: &[Value],
    heap: &mut Heap,
    builtin: &str,
    newline: bool,
) -> Result<Value, String> {
    use std::io::Write as _;

    let text = extract_string_arg(args, heap, builtin)?;
    let bytes = text.len() as u64 + u64::from(newline);
    heap.charge_io_bytes(bytes)
        .map_err(|e| format!("{builtin}: {e}"))?;
    if let Some(sink) = heap.error_sink() {
        let line = if newline {
            format!("{text}\n")
        } else {
            text.to_string()
        };
        sink.write(&line).map_err(|e| format!("{builtin}: {e}"))?;
        return Ok(Value::Unit);
    }
    let mut stderr = std::io::stderr().lock();
    stderr
        .write_all(text.as_bytes())
        .and_then(|()| {
            if newline {
                stderr.write_all(b"\n")
            } else {
                Ok(())
            }
        })
        .and_then(|()| stderr.flush())
        .map_err(|e| format!("{builtin}: {e}"))?;
    Ok(Value::Unit)
}

pub(super) fn read_line(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("read_line: {e}"))?;
    let io_bytes = line.len() as u64;
    if line.ends_with('\n') {
        line.pop();
    }
    heap.charge_io_bytes(io_bytes)
        .map_err(|e| format!("read_line: {e}"))?;
    heap_alloc(heap, HeapObject::String(line.into()))
}

pub(super) fn read_stdin(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let input = heap.read_stdin()?;
    heap_alloc(heap, HeapObject::String(input.into()))
}
