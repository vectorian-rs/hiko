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
use std::sync::Arc;

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
) -> Result<Arc<str>, String> {
    match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(s.clone()),
            _ => Err(format!("{name}: expected String")),
        },
        _ => Err(format!("{name}: expected String")),
    }
}

/// The two string fields of a `(String, String)` argument.
pub(crate) fn string_pair(
    args: &[Value],
    heap: &Heap,
    builtin: &str,
) -> Result<(Arc<str>, Arc<str>), String> {
    let expected = || format!("{builtin}: expected (String, String)");
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err(expected()),
        },
        _ => return Err(expected()),
    };
    Ok((
        extract_string_arg(&[v0], heap, builtin)?,
        extract_string_arg(&[v1], heap, builtin)?,
    ))
}

/// Host work for one builtin call, in opcode-equivalents: `base`, plus
/// `per_kib` for every KiB of `bytes`, rounded up. See
/// docs/builtins/accounting-matrix.md#calibration for how the rates are
/// measured.
pub(crate) fn host_work(base: u64, per_kib: u64, bytes: usize) -> u64 {
    base.saturating_add((bytes as u64).saturating_mul(per_kib).div_ceil(1024))
}

/// Charge `work` units of host work for a call of `builtin`, with the
/// builtin's name in the error.
pub(crate) fn charge(heap: &mut Heap, builtin: &str, work: u64) -> Result<(), String> {
    heap.charge_host_work(work)
        .map_err(|e| format!("{builtin}: {e}"))
}
