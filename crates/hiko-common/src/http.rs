use std::time::Duration;

/// Applies caller headers and disables redirect following.
///
/// Callers (the VM HTTP builtins) enforce a host allowlist on the request URL
/// only, so following a 3xx would let a server bounce the request to any host
/// or downgrade https to http. The 3xx response is returned to the caller.
fn apply_headers<State>(
    request: ureq::RequestBuilder<State>,
    headers: &[(&str, &str)],
) -> ureq::RequestBuilder<State> {
    let mut request = request.config().max_redirects(0).build();
    for (key, value) in headers {
        request = request.header(*key, *value);
    }
    request
}

pub fn dispatch_ureq(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let send_no_body = |request: ureq::RequestBuilder<ureq::typestate::WithoutBody>| {
        apply_headers(request, headers)
            .call()
            .map_err(|e| format!("http: {e}"))
    };
    let send_with_body = |request: ureq::RequestBuilder<ureq::typestate::WithBody>| {
        apply_headers(request, headers)
            .send(body.as_bytes())
            .map_err(|e| format!("http: {e}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn dispatch_does_not_follow_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: http://evil.invalid/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let response = dispatch_ureq("GET", &format!("http://{addr}/"), &[], "").unwrap();
        assert_eq!(response.status().as_u16(), 302);
        server.join().unwrap();
    }
}
