use crate::heap::Heap;
use crate::value::{HeapObject, Value};
use crate::vm::{TAG_CONS, TAG_NIL};
#[cfg(any(
    test,
    feature = "builtin-filesystem",
    feature = "builtin-http",
    feature = "builtin-json",
    feature = "builtin-string"
))]
use smallvec::smallvec;

/// Allocate a heap object and wrap it as a Value, converting HeapLimitExceeded to String.
#[cfg(any(
    test,
    feature = "builtin-bytes",
    feature = "builtin-convert",
    feature = "builtin-env",
    feature = "builtin-filesystem",
    feature = "builtin-hash",
    feature = "builtin-http",
    feature = "builtin-json",
    feature = "builtin-path",
    feature = "builtin-random",
    feature = "builtin-regex",
    feature = "builtin-stdio",
    feature = "builtin-string",
    feature = "builtin-time"
))]
pub(crate) fn heap_alloc(heap: &mut Heap, obj: HeapObject) -> Result<Value, String> {
    heap.alloc(obj).map(Value::Heap).map_err(|e| e.to_string())
}

#[cfg(any(
    test,
    feature = "builtin-filesystem",
    feature = "builtin-http",
    feature = "builtin-json",
    feature = "builtin-string"
))]
pub(crate) fn alloc_list(heap: &mut Heap, elems: Vec<Value>) -> Result<Value, String> {
    let mut list = heap_alloc(
        heap,
        HeapObject::Data {
            tag: TAG_NIL,
            fields: smallvec![],
        },
    )?;
    for elem in elems.into_iter().rev() {
        list = heap_alloc(
            heap,
            HeapObject::Data {
                tag: TAG_CONS,
                fields: smallvec![elem, list],
            },
        )?;
    }
    Ok(list)
}

/// Stable 64-bit FNV-1a hash, returned as 16-char lowercase hex.
#[cfg(feature = "builtin-filesystem")]
pub(crate) fn fnv1a_tag_parts(parts: &[&str]) -> String {
    const BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = BASIS;
    for part in parts {
        for &b in part.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(PRIME);
        }
    }
    format!("{h:016x}")
}

#[cfg(feature = "builtin-filesystem")]
pub(crate) fn is_valid_fnv1a_tag(tag: &str) -> bool {
    tag.len() == 16 && tag.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

pub(crate) fn collect_list(heap: &Heap, list_val: Value) -> Result<Vec<Value>, String> {
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

pub(crate) fn extract_pid_list_arg(
    args: &[Value],
    heap: &Heap,
    name: &str,
) -> Result<Vec<u64>, String> {
    let list_val = args
        .first()
        .copied()
        .ok_or_else(|| format!("{name}: expected pid list"))?;
    let values = collect_list(heap, list_val)?;
    let mut pids = Vec::with_capacity(values.len());
    for value in values {
        match value {
            Value::Pid(pid) => pids.push(pid),
            _ => return Err(format!("{name}: expected pid list")),
        }
    }
    Ok(pids)
}

pub(crate) fn extract_string_arg(
    args: &[Value],
    heap: &Heap,
    name: &str,
) -> Result<String, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(s.clone()),
            _ => Err(format!("{name}: expected String")),
        },
        _ => Err(format!("{name}: expected String")),
    }
}
