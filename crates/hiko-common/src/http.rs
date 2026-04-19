pub fn dispatch_ureq(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let send_no_body = |r: ureq::RequestBuilder<ureq::typestate::WithoutBody>| {
        let mut r = r;
        for (k, v) in headers {
            r = r.header(k.as_str(), v.as_str());
        }
        r.call().map_err(|e| format!("http: {e}"))
    };
    let send_with_body = |r: ureq::RequestBuilder<ureq::typestate::WithBody>| {
        let mut r = r;
        for (k, v) in headers {
            r = r.header(k.as_str(), v.as_str());
        }
        r.send(body.as_bytes()).map_err(|e| format!("http: {e}"))
    };

    match method.to_uppercase().as_str() {
        "GET" => send_no_body(ureq::get(url)),
        "HEAD" => send_no_body(ureq::head(url)),
        "DELETE" => send_no_body(ureq::delete(url)),
        "POST" => send_with_body(ureq::post(url)),
        "PUT" => send_with_body(ureq::put(url)),
        "PATCH" => send_with_body(ureq::patch(url)),
        other => Err(format!("http: unsupported method '{other}'")),
    }
}

pub fn http_get_text(url: &str) -> Result<String, String> {
    dispatch_ureq("GET", url, &[], "").and_then(|response| {
        response
            .into_body()
            .read_to_string()
            .map_err(|e| format!("http: {e}"))
    })
}
