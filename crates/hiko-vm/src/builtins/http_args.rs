use crate::heap::Heap;
use crate::value::{GcRef, HeapObject, Value};

use super::collect_list;

type HttpArgs = (String, String, Vec<(String, String)>, String);

pub(crate) struct HttpArgRefs {
    method: GcRef,
    url: GcRef,
    headers: Vec<(GcRef, GcRef)>,
    body: GcRef,
    #[cfg(feature = "builtin-http")]
    body_len: usize,
}

impl HttpArgRefs {
    pub(crate) fn method<'a>(&self, heap: &'a Heap) -> Result<&'a str, String> {
        heap_string_ref(heap, self.method)
    }

    pub(crate) fn url<'a>(&self, heap: &'a Heap) -> Result<&'a str, String> {
        heap_string_ref(heap, self.url)
    }

    pub(crate) fn body<'a>(&self, heap: &'a Heap) -> Result<&'a str, String> {
        heap_string_ref(heap, self.body)
    }

    #[cfg(feature = "builtin-http")]
    pub(crate) fn body_len(&self) -> usize {
        self.body_len
    }

    pub(crate) fn headers<'a>(&self, heap: &'a Heap) -> Result<Vec<(&'a str, &'a str)>, String> {
        self.headers
            .iter()
            .map(|(key, value)| Ok((heap_string_ref(heap, *key)?, heap_string_ref(heap, *value)?)))
            .collect()
    }
}

fn expect_string_ref(value: Value, heap: &Heap, message: String) -> Result<GcRef, String> {
    match value {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(_) => Ok(r),
            _ => Err(message),
        },
        _ => Err(message),
    }
}

fn heap_string_ref(heap: &Heap, r: GcRef) -> Result<&str, String> {
    match heap.get(r).map_err(|e| e.to_string())? {
        HeapObject::String(s) => Ok(s.as_str()),
        _ => Err("expected String".into()),
    }
}

pub(crate) fn extract_http_arg_refs(
    args: &[Value],
    heap: &Heap,
    name: &str,
) -> Result<HttpArgRefs, String> {
    let expected = || format!("{name}: expected (String, String, (String * String) list, String)");
    let (v0, v1, v2, v3) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 4 => (t[0], t[1], t[2], t[3]),
            _ => return Err(expected()),
        },
        _ => return Err(expected()),
    };

    let method = expect_string_ref(v0, heap, format!("{name}: expected String for method"))?;
    let url = expect_string_ref(v1, heap, format!("{name}: expected String for url"))?;
    let body = expect_string_ref(v3, heap, format!("{name}: expected String for body"))?;
    #[cfg(feature = "builtin-http")]
    let body_len = heap_string_ref(heap, body)?.len();

    let mut headers = Vec::new();
    for elem in collect_list(heap, v2)? {
        match elem {
            Value::Heap(tr) => match heap.get(tr).map_err(|e| e.to_string())? {
                HeapObject::Tuple(pair) if pair.len() == 2 => {
                    let key = expect_string_ref(
                        pair[0],
                        heap,
                        format!("{name}: header key must be String"),
                    )?;
                    let value = expect_string_ref(
                        pair[1],
                        heap,
                        format!("{name}: header value must be String"),
                    )?;
                    headers.push((key, value));
                }
                _ => return Err(format!("{name}: headers must be (String, String) list")),
            },
            _ => return Err(format!("{name}: headers must be (String, String) list")),
        }
    }

    Ok(HttpArgRefs {
        method,
        url,
        headers,
        body,
        #[cfg(feature = "builtin-http")]
        body_len,
    })
}

pub(crate) fn extract_http_args(
    args: &[Value],
    heap: &Heap,
    name: &str,
) -> Result<HttpArgs, String> {
    let refs = extract_http_arg_refs(args, heap, name)?;
    let method = refs.method(heap)?.to_string();
    let url = refs.url(heap)?.to_string();
    let headers = refs
        .headers(heap)?
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let body = refs.body(heap)?.to_string();
    Ok((method, url, headers, body))
}
