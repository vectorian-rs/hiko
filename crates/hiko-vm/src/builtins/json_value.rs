use super::*;
use smallvec::smallvec;

const TAG_JNULL: u16 = 0;
const TAG_JBOOL: u16 = 1;
const TAG_JINT: u16 = 2;
const TAG_JFLOAT: u16 = 3;
const TAG_JSTR: u16 = 4;
const TAG_JARRAY: u16 = 5;
const TAG_JOBJECT: u16 = 6;

pub(crate) fn json_to_hiko(val: &serde_json::Value, heap: &mut Heap) -> Result<Value, String> {
    match val {
        serde_json::Value::Null => heap_alloc(
            heap,
            HeapObject::Data {
                tag: TAG_JNULL,
                fields: smallvec![],
            },
        ),
        serde_json::Value::Bool(b) => heap_alloc(
            heap,
            HeapObject::Data {
                tag: TAG_JBOOL,
                fields: smallvec![Value::Bool(*b)],
            },
        ),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                heap_alloc(
                    heap,
                    HeapObject::Data {
                        tag: TAG_JINT,
                        fields: smallvec![Value::Int(i)],
                    },
                )
            } else if let Some(f) = n.as_f64() {
                heap_alloc(
                    heap,
                    HeapObject::Data {
                        tag: TAG_JFLOAT,
                        fields: smallvec![Value::Float(f)],
                    },
                )
            } else {
                heap_alloc(
                    heap,
                    HeapObject::Data {
                        tag: TAG_JNULL,
                        fields: smallvec![],
                    },
                )
            }
        }
        serde_json::Value::String(s) => {
            let sv = heap_alloc(heap, HeapObject::String(s.clone()))?;
            heap_alloc(
                heap,
                HeapObject::Data {
                    tag: TAG_JSTR,
                    fields: smallvec![sv],
                },
            )
        }
        serde_json::Value::Array(arr) => {
            let mut elems = Vec::with_capacity(arr.len());
            for v in arr {
                elems.push(json_to_hiko(v, heap)?);
            }
            let list = alloc_list(heap, elems)?;
            heap_alloc(
                heap,
                HeapObject::Data {
                    tag: TAG_JARRAY,
                    fields: smallvec![list],
                },
            )
        }
        serde_json::Value::Object(map) => {
            let mut pairs = Vec::with_capacity(map.len());
            for (k, v) in map {
                let key = heap_alloc(heap, HeapObject::String(k.clone()))?;
                let val = json_to_hiko(v, heap)?;
                pairs.push(heap_alloc(heap, HeapObject::Tuple(smallvec![key, val]))?);
            }
            let list = alloc_list(heap, pairs)?;
            heap_alloc(
                heap,
                HeapObject::Data {
                    tag: TAG_JOBJECT,
                    fields: smallvec![list],
                },
            )
        }
    }
}

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
            c if c < '\u{20}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn hiko_to_json_string(val: Value, heap: &Heap) -> Result<String, String> {
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
