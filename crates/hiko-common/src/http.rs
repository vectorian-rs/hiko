use std::time::Duration;

pub fn dispatch_ureq(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let send_no_body = |r: ureq::RequestBuilder<ureq::typestate::WithoutBody>| {
        let mut r = r;
        for (k, v) in headers {
            r = r.header(*k, *v);
        }
        r.call().map_err(|e| format!("http: {e}"))
    };
    let send_with_body = |r: ureq::RequestBuilder<ureq::typestate::WithBody>| {
        let mut r = r;
        for (k, v) in headers {
            r = r.header(*k, *v);
        }
        r.send(body.as_bytes()).map_err(|e| format!("http: {e}"))
    };

    if method.eq_ignore_ascii_case("GET") {
        send_no_body(ureq::get(url))
    } else if method.eq_ignore_ascii_case("HEAD") {
        send_no_body(ureq::head(url))
    } else if method.eq_ignore_ascii_case("DELETE") {
        send_no_body(ureq::delete(url))
    } else if method.eq_ignore_ascii_case("POST") {
        send_with_body(ureq::post(url))
    } else if method.eq_ignore_ascii_case("PUT") {
        send_with_body(ureq::put(url))
    } else if method.eq_ignore_ascii_case("PATCH") {
        send_with_body(ureq::patch(url))
    } else {
        Err(format!("http: unsupported method '{method}'"))
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

pub fn http_get_text_limited(
    url: &str,
    timeout: Duration,
    max_bytes: u64,
) -> Result<String, String> {
    ureq::get(url)
        .config()
        .timeout_global(Some(timeout))
        .build()
        .call()
        .map_err(|e| format!("http: {e}"))?
        .into_body()
        .into_with_config()
        .limit(max_bytes)
        .read_to_string()
        .map_err(|e| format!("http: {e}"))
}
