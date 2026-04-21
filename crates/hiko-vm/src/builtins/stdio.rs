use super::*;

pub(super) fn print(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Ok(args[0])
}

pub(super) fn println(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Ok(args[0])
}

pub(super) fn read_line(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("read_line: {e}"))?;
    if line.ends_with('\n') {
        line.pop();
    }
    heap_alloc(heap, HeapObject::String(line))
}

pub(super) fn read_stdin(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let input = heap.read_stdin()?;
    heap_alloc(heap, HeapObject::String(input))
}
