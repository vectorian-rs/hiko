use super::*;
use smallvec::smallvec;

fn collect_headers(
    header_pairs: impl IntoIterator<Item = (String, String)>,
    heap: &mut Heap,
) -> Value {
    let mut header_values: Vec<Value> = Vec::new();
    for (k, v) in header_pairs {
        let k = Value::Heap(heap.alloc(HeapObject::String(k)));
        let v = Value::Heap(heap.alloc(HeapObject::String(v)));
        let pair = Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![k, v])));
        header_values.push(pair);
    }
    alloc_list(heap, header_values)
}

fn do_http_request(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &str,
    name: &str,
    heap: &mut Heap,
) -> Result<(Value, Value, Box<dyn std::io::Read + Send>), String> {
    heap.check_http_host_for(name, url)
        .map_err(|e| format!("{name}: {e}"))?;
    let response = hiko_common::dispatch_ureq(method, url, headers, body)
        .map_err(|e| format!("{name}: {e}"))?;
    let status = Value::Int(response.status().as_u16() as i64);
    let resp_headers = collect_headers(
        response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())),
        heap,
    );
    let reader = Box::new(response.into_body().into_reader()) as Box<dyn std::io::Read + Send>;
    Ok((status, resp_headers, reader))
}

pub(super) fn http_get(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let url = extract_string_arg(args, heap, "http_get")?;
    let (status, headers, mut reader) = do_http_request("GET", &url, &[], "", "http_get", heap)?;
    let mut body_str = String::new();
    std::io::Read::read_to_string(&mut reader, &mut body_str)
        .map_err(|e| format!("http_get: {e}"))?;
    let body = Value::Heap(heap.alloc(HeapObject::String(body_str)));
    Ok(Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![
        status, headers, body
    ]))))
}

pub(super) fn http(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

pub(super) fn http_json(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

pub(super) fn http_msgpack(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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

pub(super) fn http_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
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
