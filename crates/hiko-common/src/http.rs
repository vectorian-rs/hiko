use std::net::IpAddr;
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

/// Fetch a module source over HTTP.
///
/// Module fetches are verified by hash afterwards, but the fetch itself still
/// refuses anything a lockfile author would not expect: redirects are not
/// followed (a 3xx is an error), and plain `http://` is accepted only for
/// loopback hosts such as a local package server.
pub fn http_get_text_limited(
    url: &str,
    timeout: Duration,
    max_bytes: u64,
) -> Result<String, String> {
    check_module_url(url)?;
    let response = ureq::get(url)
        .config()
        .max_redirects(0)
        .timeout_global(Some(timeout))
        .build()
        .call()
        .map_err(|e| format!("http: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "http: unexpected status {} for {url}",
            status.as_u16()
        ));
    }
    response
        .into_body()
        .into_with_config()
        .limit(max_bytes)
        .read_to_string()
        .map_err(|e| format!("http: {e}"))
}

fn check_module_url(url: &str) -> Result<(), String> {
    let uri: ureq::http::Uri = url
        .parse()
        .map_err(|e| format!("http: invalid url '{url}': {e}"))?;
    match uri.scheme_str() {
        Some("https") => Ok(()),
        Some("http") if uri.host().is_some_and(is_loopback_host) => Ok(()),
        Some("http") => Err(format!(
            "http: refusing plain http for non-loopback host in '{url}'; use https"
        )),
        _ => Err(format!("http: unsupported url scheme in '{url}'")),
    }
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serve one connection with a fixed raw HTTP response.
    fn serve_once(response: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            stream.write_all(response).unwrap();
        });
        (format!("http://{addr}"), server)
    }

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

    #[test]
    fn module_fetch_rejects_redirect() {
        let (base, server) = serve_once(
            b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let err = http_get_text_limited(&format!("{base}/m.hml"), Duration::from_secs(5), 1024)
            .unwrap_err();
        assert!(err.contains("unexpected status 302"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn module_fetch_accepts_loopback_http() {
        let (base, server) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nval x");
        let text =
            http_get_text_limited(&format!("{base}/m.hml"), Duration::from_secs(5), 1024).unwrap();
        assert_eq!(text, "val x");
        server.join().unwrap();
    }

    #[test]
    fn module_fetch_rejects_plain_http_for_remote_host() {
        // Rejected before any connection is attempted.
        let err = http_get_text_limited(
            "http://example.com/Std/List.hml",
            Duration::from_secs(5),
            1024,
        )
        .unwrap_err();
        assert!(err.contains("refusing plain http"), "{err}");
    }

    #[test]
    fn module_url_scheme_rules() {
        assert!(check_module_url("https://example.com/a.hml").is_ok());
        assert!(check_module_url("http://127.0.0.1:8000/a.hml").is_ok());
        assert!(check_module_url("http://127.0.0.2/a.hml").is_ok());
        assert!(check_module_url("http://localhost:8000/a.hml").is_ok());
        assert!(check_module_url("http://[::1]:8000/a.hml").is_ok());
        assert!(check_module_url("http://10.0.0.1/a.hml").is_err());
        assert!(check_module_url("http://localhost.example.com/a.hml").is_err());
        assert!(check_module_url("ftp://127.0.0.1/a.hml").is_err());
    }
}
