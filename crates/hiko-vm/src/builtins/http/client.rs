use super::*;
use smallvec::smallvec;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    &[
        ("http_get", http_get as BuiltinFn),
        ("http", http),
        ("http_json", http_json),
        ("http_msgpack", http_msgpack),
        ("http_bytes", http_bytes),
    ]
}

fn collect_headers<'a>(
    header_pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    heap: &mut Heap,
) -> Result<Value, String> {
    let mut header_values: Vec<Value> = Vec::new();
    for (k, v) in header_pairs {
        let k = heap_alloc(heap, HeapObject::String(k.to_string()))?;
        let v = heap_alloc(heap, HeapObject::String(v.to_string()))?;
        let pair = heap_alloc(heap, HeapObject::Tuple(smallvec![k, v]))?;
        header_values.push(pair);
    }
    alloc_list(heap, header_values)
}

fn do_http_request(
    args: &HttpArgRefs,
    name: &str,
    heap: &mut Heap,
) -> Result<(Value, Value, Box<dyn std::io::Read + Send>), String> {
    {
        let url = args.url(heap)?;
        heap.check_http_host_for(name, url)
            .map_err(|e| format!("{name}: {e}"))?;
    }

    heap.charge_io_bytes(args.body_len() as u64)
        .map_err(|e| format!("{name}: {e}"))?;

    let response = {
        let method = args.method(heap)?;
        let url = args.url(heap)?;
        let body = args.body(heap)?;
        let headers = args.headers(heap)?;
        hiko_common::dispatch_ureq(method, url, &headers, body)
            .map_err(|e| format!("{name}: {e}"))?
    };

    let status = Value::Int(response.status().as_u16() as i64);
    let resp_headers = collect_headers(
        response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str(), v.to_str().unwrap_or(""))),
        heap,
    )?;
    let reader = Box::new(response.into_body().into_reader()) as Box<dyn std::io::Read + Send>;
    Ok((status, resp_headers, reader))
}

pub(super) fn http_get(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let url = extract_string_arg(args, heap, "http_get")?;
    heap.check_http_host_for("http_get", &url)
        .map_err(|e| format!("http_get: {e}"))?;
    let response =
        hiko_common::dispatch_ureq("GET", &url, &[], "").map_err(|e| format!("http_get: {e}"))?;
    let status = Value::Int(response.status().as_u16() as i64);
    let headers = collect_headers(
        response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str(), v.to_str().unwrap_or(""))),
        heap,
    )?;
    let mut reader = Box::new(response.into_body().into_reader()) as Box<dyn std::io::Read + Send>;
    let mut body_str = String::new();
    std::io::Read::read_to_string(&mut reader, &mut body_str)
        .map_err(|e| format!("http_get: {e}"))?;
    heap.charge_io_bytes(body_str.len() as u64)
        .map_err(|e| format!("http_get: {e}"))?;
    let body = heap_alloc(heap, HeapObject::String(body_str))?;
    heap_alloc(heap, HeapObject::Tuple(smallvec![status, headers, body]))
}

pub(super) fn http(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let request = extract_http_arg_refs(args, heap, "http")?;
    let (status, resp_headers, mut reader) = do_http_request(&request, "http", heap)?;
    let mut body_str = String::new();
    std::io::Read::read_to_string(&mut reader, &mut body_str).map_err(|e| format!("http: {e}"))?;
    heap.charge_io_bytes(body_str.len() as u64)
        .map_err(|e| format!("http: {e}"))?;
    let resp_body = heap_alloc(heap, HeapObject::String(body_str))?;
    heap_alloc(
        heap,
        HeapObject::Tuple(smallvec![status, resp_headers, resp_body]),
    )
}

pub(super) fn http_json(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let request = extract_http_arg_refs(args, heap, "http_json")?;
    let (status, resp_headers, mut reader) = do_http_request(&request, "http_json", heap)?;
    let mut body_str = String::new();
    std::io::Read::read_to_string(&mut reader, &mut body_str)
        .map_err(|e| format!("http_json: {e}"))?;
    heap.charge_io_bytes(body_str.len() as u64)
        .map_err(|e| format!("http_json: {e}"))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body_str).map_err(|e| format!("http_json: {e}"))?;
    let resp_body = json_to_hiko(&parsed, heap)?;
    heap_alloc(
        heap,
        HeapObject::Tuple(smallvec![status, resp_headers, resp_body]),
    )
}

pub(super) fn http_msgpack(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let request = extract_http_arg_refs(args, heap, "http_msgpack")?;
    let (status, resp_headers, mut reader) = do_http_request(&request, "http_msgpack", heap)?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf).map_err(|e| format!("http_msgpack: {e}"))?;
    heap.charge_io_bytes(buf.len() as u64)
        .map_err(|e| format!("http_msgpack: {e}"))?;
    let parsed: serde_json::Value =
        rmp_serde::from_slice(&buf).map_err(|e| format!("http_msgpack: {e}"))?;
    let resp_body = json_to_hiko(&parsed, heap)?;
    heap_alloc(
        heap,
        HeapObject::Tuple(smallvec![status, resp_headers, resp_body]),
    )
}

pub(super) fn http_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let request = extract_http_arg_refs(args, heap, "http_bytes")?;
    let (status, resp_headers, mut reader) = do_http_request(&request, "http_bytes", heap)?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf).map_err(|e| format!("http_bytes: {e}"))?;
    heap.charge_io_bytes(buf.len() as u64)
        .map_err(|e| format!("http_bytes: {e}"))?;
    let resp_body = heap_alloc(heap, HeapObject::Bytes(buf))?;
    heap_alloc(
        heap,
        HeapObject::Tuple(smallvec![status, resp_headers, resp_body]),
    )
}
