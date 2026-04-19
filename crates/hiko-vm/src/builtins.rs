use crate::heap::Heap;
use crate::value::{BuiltinFn, HeapObject, Value};
use crate::vm::{TAG_CONS, TAG_NIL};
use smallvec::smallvec;

#[cfg(feature = "builtin-bytes")]
mod bytes;
#[cfg(feature = "builtin-convert")]
mod convert;
#[cfg(feature = "builtin-env")]
mod env;
#[cfg(feature = "builtin-exec")]
mod exec;
#[cfg(feature = "builtin-filesystem")]
mod filesystem;
#[cfg(feature = "builtin-hash")]
mod hash;
#[cfg(feature = "builtin-http")]
mod http;
#[cfg(feature = "builtin-json")]
mod json;
#[cfg(feature = "builtin-math")]
mod math;
#[cfg(feature = "builtin-path")]
mod path;
#[cfg(feature = "builtin-process")]
mod process;
#[cfg(feature = "builtin-random")]
mod random;
#[cfg(feature = "builtin-regex")]
mod regex;
#[cfg(feature = "builtin-stdio")]
mod stdio;
#[cfg(feature = "builtin-string")]
mod string;
#[cfg(feature = "builtin-system")]
mod system;
#[cfg(feature = "builtin-testing")]
mod testing;
#[cfg(feature = "builtin-time")]
mod time;

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
    let n = (h % 3844) as usize;
    [BASE62[n / 62], BASE62[n % 62]]
}

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

#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
const TAG_JNULL: u16 = 0;
#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
const TAG_JBOOL: u16 = 1;
#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
const TAG_JINT: u16 = 2;
#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
const TAG_JFLOAT: u16 = 3;
#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
const TAG_JSTR: u16 = 4;
#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
const TAG_JARRAY: u16 = 5;
#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
const TAG_JOBJECT: u16 = 6;

#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
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

#[cfg(feature = "builtin-json")]
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

#[cfg(feature = "builtin-json")]
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

type HttpArgs = (String, String, Vec<(String, String)>, String);

pub(crate) fn extract_http_args(
    args: &[Value],
    heap: &Heap,
    name: &str,
) -> Result<HttpArgs, String> {
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

pub(crate) fn builtin_entries() -> Vec<(&'static str, BuiltinFn)> {
    let mut entries = Vec::new();

    #[cfg(feature = "builtin-stdio")]
    entries.extend([
        ("print", stdio::print as BuiltinFn),
        ("println", stdio::println),
        ("read_line", stdio::read_line),
        ("read_stdin", stdio::read_stdin),
    ]);

    #[cfg(feature = "builtin-convert")]
    entries.extend([
        ("int_to_string", convert::int_to_string as BuiltinFn),
        ("float_to_string", convert::float_to_string),
        ("string_to_int", convert::string_to_int),
        ("char_to_int", convert::char_to_int),
        ("int_to_char", convert::int_to_char),
        ("int_to_float", convert::int_to_float),
    ]);

    #[cfg(feature = "builtin-string")]
    entries.extend([
        ("string_length", string::string_length as BuiltinFn),
        ("substring", string::substring),
        ("string_contains", string::string_contains),
        ("trim", string::trim),
        ("split", string::split),
        ("string_replace", string::string_replace),
        ("starts_with", string::starts_with),
        ("ends_with", string::ends_with),
        ("to_upper", string::to_upper),
        ("to_lower", string::to_lower),
        ("string_join", string::string_join),
    ]);

    #[cfg(feature = "builtin-regex")]
    entries.extend([
        ("regex_match", regex::regex_match as BuiltinFn),
        ("regex_replace", regex::regex_replace),
    ]);

    #[cfg(feature = "builtin-json")]
    entries.extend([
        ("json_parse", json::json_parse as BuiltinFn),
        ("json_to_string", json::json_to_string),
        ("json_get", json::json_get),
        ("json_keys", json::json_keys),
        ("json_length", json::json_length),
    ]);

    #[cfg(feature = "builtin-math")]
    entries.extend([
        ("sqrt", math::sqrt as BuiltinFn),
        ("abs_int", math::abs_int),
        ("abs_float", math::abs_float),
        ("floor", math::floor),
        ("ceil", math::ceil),
    ]);

    #[cfg(feature = "builtin-bytes")]
    entries.extend([
        ("bytes_length", bytes::bytes_length as BuiltinFn),
        ("bytes_to_string", bytes::bytes_to_string),
        ("string_to_bytes", bytes::string_to_bytes),
        ("bytes_get", bytes::bytes_get),
        ("bytes_slice", bytes::bytes_slice),
    ]);

    #[cfg(feature = "builtin-hash")]
    entries.push(("blake3", hash::blake3 as BuiltinFn));

    #[cfg(feature = "builtin-random")]
    entries.extend([
        ("random_bytes", random::random_bytes as BuiltinFn),
        ("rng_seed", random::rng_seed),
        ("rng_bytes", random::rng_bytes),
        ("rng_int", random::rng_int),
    ]);

    #[cfg(feature = "builtin-env")]
    entries.push(("getenv", env::getenv as BuiltinFn));

    #[cfg(feature = "builtin-time")]
    entries.extend([
        ("epoch", time::epoch as BuiltinFn),
        ("epoch_ms", time::epoch_ms),
        ("monotonic_ms", time::monotonic_ms),
        ("sleep", time::sleep),
    ]);

    #[cfg(feature = "builtin-process")]
    entries.extend([
        ("spawn", process::spawn_placeholder as BuiltinFn),
        ("await_process", process::await_placeholder),
    ]);

    #[cfg(feature = "builtin-path")]
    entries.push(("path_join", path::path_join as BuiltinFn));

    #[cfg(feature = "builtin-filesystem")]
    entries.extend([
        ("read_file", filesystem::read_file as BuiltinFn),
        ("read_file_bytes", filesystem::read_file_bytes),
        ("write_file", filesystem::write_file),
        ("file_exists", filesystem::file_exists),
        ("list_dir", filesystem::list_dir),
        ("remove_file", filesystem::remove_file),
        ("create_dir", filesystem::create_dir),
        ("is_dir", filesystem::is_dir),
        ("is_file", filesystem::is_file),
        ("read_file_tagged", filesystem::read_file_tagged),
        ("edit_file_tagged", filesystem::edit_file_tagged),
        ("glob", filesystem::glob),
        ("walk_dir", filesystem::walk_dir),
    ]);

    #[cfg(feature = "builtin-http")]
    entries.extend([
        ("http_get", http::http_get as BuiltinFn),
        ("http", http::http),
        ("http_json", http::http_json),
        ("http_msgpack", http::http_msgpack),
        ("http_bytes", http::http_bytes),
    ]);

    #[cfg(feature = "builtin-exec")]
    entries.push(("exec", exec::exec as BuiltinFn));

    #[cfg(feature = "builtin-system")]
    entries.push(("exit", system::exit as BuiltinFn));

    #[cfg(feature = "builtin-testing")]
    entries.extend([
        ("panic", testing::panic as BuiltinFn),
        ("assert", testing::assert),
        ("assert_eq", testing::assert_eq),
    ]);

    entries
}
