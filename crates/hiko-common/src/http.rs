use std::net::IpAddr;
use std::time::Duration;

use crate::HttpTimeouts;

/// Applies caller headers and timeouts, and disables redirect following.
///
/// Callers (the VM HTTP builtins) enforce a host allowlist on the request URL
/// only, so following a 3xx would let a server bounce the request to any host
/// or downgrade https to http. The 3xx response is returned to the caller.
fn apply_headers<State>(
    request: ureq::RequestBuilder<State>,
    headers: &[(&str, &str)],
    timeouts: HttpTimeouts,
) -> ureq::RequestBuilder<State> {
    let mut request = request
        .config()
        .max_redirects(0)
        .timeout_connect(Some(timeouts.connect))
        .timeout_global(Some(timeouts.global))
        .build();
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
    timeouts: HttpTimeouts,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let send_no_body = |request: ureq::RequestBuilder<ureq::typestate::WithoutBody>| {
        apply_headers(request, headers, timeouts)
            .call()
            .map_err(|e| format!("http: {e}"))
    };
    let send_with_body = |request: ureq::RequestBuilder<ureq::typestate::WithBody>| {
        apply_headers(request, headers, timeouts)
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

/// The host a request to `url` goes to, for a host allowlist check.
///
/// The URL is parsed with the client's own parser (`ureq::http::Uri`), so the
/// host checked is the host `ureq` connects to. It fails closed: the scheme
/// must be http or https, and the host must be a DNS name (ASCII letters,
/// digits, `-`, `_` and `.`), which is lowercased, or a bracketed IPv6 address
/// without a zone, which is returned in canonical form. Anything else, such
/// as percent-encoded or non-ASCII hosts, is an error.
pub fn request_host(url: &str) -> Result<String, String> {
    let uri: ureq::http::Uri = url
        .parse()
        .map_err(|e| format!("invalid URL '{url}': {e}"))?;
    let scheme = uri.scheme_str().unwrap_or("");
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(format!(
            "URL '{url}' uses unsupported scheme '{scheme}'; expected http or https"
        ));
    }
    let host = match uri.host() {
        Some(host) if !host.is_empty() => host,
        _ => return Err(format!("URL '{url}' has no host")),
    };
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return inner
            .parse::<std::net::Ipv6Addr>()
            .map(|ip| format!("[{ip}]"))
            .map_err(|_| format!("URL '{url}' has an invalid IPv6 host '{host}'"));
    }
    if host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        Ok(host.to_ascii_lowercase())
    } else {
        Err(format!(
            "URL '{url}' has host '{host}', which is not a DNS name or IP address"
        ))
    }
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
    use std::time::Instant;

    const TEST_TIMEOUTS: HttpTimeouts = HttpTimeouts {
        connect: Duration::from_secs(5),
        global: Duration::from_secs(5),
    };

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

    /// URLs on which the `url` crate (the old allowlist parser) and the
    /// client's parser disagreed, or that are easy to get wrong.
    const TRICKY_URLS: &[&str] = &[
        "http://localhost:80@evil.example/path",
        "http://user:pw@allowed.example/",
        "http://evil.example\\@allowed.example/",
        "http://evil.example\\.allowed.example/",
        "http://ALLOWED.Example/",
        "HTTP://allowed.example/",
        "http://allowed.example./",
        "http://%61llowed.example/",
        "http://allowed%2eexample/",
        "http://b\u{fc}cher.example/",
        "http://xn--bcher-kva.example/",
        "http://[::1]/",
        "http://[0:0:0:0:0:0:0:1]/",
        "http://[fe80::1%25en0]/",
        "http://0x7f.1/",
        "http://2130706433/",
        "http://127.000.000.001/",
        "http:///path",
        "http://:80/",
        "http://allowed.example:80:80/",
        "http://allowed.example#@evil.example/",
        "http://allowed.example?@evil.example/",
        "http://allowed.example\t.x/",
        "http://evil.example%00.allowed.example/",
        "https:allowed.example/",
        "http:/allowed.example/",
        "//allowed.example/",
        "ftp://allowed.example/",
        "file:///etc/passwd",
        "http://allowed_example/",
    ];

    #[test]
    fn request_host_is_the_host_the_client_uses() {
        for url in TRICKY_URLS {
            let Ok(host) = request_host(url) else {
                continue;
            };
            let uri: ureq::http::Uri = url.parse().expect("accepted URLs parse");
            let client_host = uri.host().expect("accepted URLs have a host");
            let client_host = match client_host.strip_prefix('[') {
                Some(v6) => format!(
                    "[{}]",
                    v6.trim_end_matches(']')
                        .parse::<std::net::Ipv6Addr>()
                        .unwrap()
                ),
                None => client_host.to_ascii_lowercase(),
            };
            assert_eq!(host, client_host, "{url}");
        }
    }

    #[test]
    fn request_host_accepts_and_normalizes() {
        let cases = [
            ("http://localhost:80@evil.example/path", "evil.example"),
            ("http://user:pw@allowed.example/", "allowed.example"),
            ("http://ALLOWED.Example/", "allowed.example"),
            ("HTTP://allowed.example/", "allowed.example"),
            ("http://allowed.example./", "allowed.example."),
            ("http://xn--bcher-kva.example/", "xn--bcher-kva.example"),
            ("http://[::1]/", "[::1]"),
            ("http://[0:0:0:0:0:0:0:1]/", "[::1]"),
            ("http://allowed.example#@evil.example/", "allowed.example"),
            ("http://allowed.example?@evil.example/", "allowed.example"),
            ("http://allowed_example/", "allowed_example"),
            // Not normalized to 127.0.0.1, so an allowlist entry
            // "127.0.0.1" does not match it.
            ("http://0x7f.1/", "0x7f.1"),
            ("http://2130706433/", "2130706433"),
        ];
        for (url, expected) in cases {
            assert_eq!(request_host(url).as_deref(), Ok(expected), "{url}");
        }
    }

    #[test]
    fn request_host_rejects_what_it_cannot_classify() {
        for url in [
            "http://evil.example\\@allowed.example/",
            "http://evil.example\\.allowed.example/",
            "http://%61llowed.example/",
            "http://allowed%2eexample/",
            "http://b\u{fc}cher.example/",
            "http://[fe80::1%25en0]/",
            "http:///path",
            "http://:80/",
            "http://allowed.example:80:80/",
            "http://allowed.example\t.x/",
            "http://evil.example%00.allowed.example/",
            "https:allowed.example/",
            "http:/allowed.example/",
            "//allowed.example/",
            "ftp://allowed.example/",
            "file:///etc/passwd",
        ] {
            assert!(request_host(url).is_err(), "{url} should be rejected");
        }
    }

    #[test]
    fn dispatch_does_not_follow_redirects() {
        let (base, server) = serve_once(
            b"HTTP/1.1 302 Found\r\nLocation: http://evil.invalid/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let response = dispatch_ureq("GET", &format!("{base}/"), &[], "", TEST_TIMEOUTS).unwrap();
        assert_eq!(response.status().as_u16(), 302);
        server.join().unwrap();
    }

    #[test]
    fn dispatch_times_out_when_server_never_responds() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept the connection and hold it open without answering.
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(3));
            drop(stream);
        });
        let timeouts = HttpTimeouts {
            connect: Duration::from_secs(1),
            global: Duration::from_millis(500),
        };
        let started = Instant::now();
        let result = dispatch_ureq("GET", &format!("http://{addr}/"), &[], "", timeouts);
        let elapsed = started.elapsed();
        assert!(result.is_err(), "expected timeout error");
        assert!(
            elapsed < Duration::from_millis(1500),
            "timeout took {elapsed:?}"
        );
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
