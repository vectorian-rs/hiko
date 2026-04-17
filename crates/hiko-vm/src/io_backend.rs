//! Abstract I/O backend trait for async operations.
//!
//! The runtime suspends a process when it requests I/O, registers the
//! request with the backend, and resumes the process when the backend
//! reports completion. No worker thread is blocked during I/O.

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::sendable::SendableValue;

/// Opaque token identifying an I/O operation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IoToken(pub u64);

/// How to decode the HTTP response body.
#[derive(Debug, Clone)]
pub enum HttpResponseFormat {
    Text,
    Json,
    Msgpack,
    Bytes,
}

#[derive(Debug, Clone)]
pub enum IoRequest {
    /// Delay for a duration (async sleep).
    Sleep(Duration),
    /// HTTP GET request. Returns (status, headers, body).
    HttpGet { url: String },
    /// Full HTTP request. Returns (status, headers, body).
    Http {
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: String,
        format: HttpResponseFormat,
    },
    /// Read a file. Returns file contents as a string.
    ReadFile { path: String },
}

/// Result of a completed I/O operation.
#[derive(Debug, Clone)]
pub enum IoResult {
    /// Operation completed successfully with a value.
    Ok(SendableValue),
    /// Operation failed with an error message.
    Err(String),
}

/// Abstract I/O backend. Implementations handle the actual I/O
/// (epoll, io_uring, mock, etc.) without the runtime knowing details.
pub trait IoBackend: Send + Sync {
    /// Register an I/O request. The backend will eventually produce a result.
    fn register(&self, token: IoToken, request: IoRequest);

    /// Poll for completed I/O operations. Returns immediately.
    /// Non-blocking — returns empty vec if nothing is ready.
    fn poll(&self) -> Vec<(IoToken, IoResult)>;
}

/// Mock I/O backend for deterministic testing.
/// Completes requests immediately or after a configurable delay.
pub struct MockIoBackend {
    completed: Mutex<Vec<(IoToken, IoResult)>>,
}

impl Default for MockIoBackend {
    fn default() -> Self {
        Self {
            completed: Mutex::new(Vec::new()),
        }
    }
}

impl MockIoBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IoBackend for MockIoBackend {
    fn register(&self, token: IoToken, request: IoRequest) {
        // Mock: complete immediately with canned responses
        let result = match request {
            IoRequest::Sleep(_) => IoResult::Ok(SendableValue::Unit),
            IoRequest::HttpGet { url } => IoResult::Ok(SendableValue::Tuple(vec![
                SendableValue::Int(200),
                SendableValue::List(vec![]),
                SendableValue::String(format!("mock response from {url}").into()),
            ])),
            IoRequest::Http { url, method, .. } => IoResult::Ok(SendableValue::Tuple(vec![
                SendableValue::Int(200),
                SendableValue::List(vec![]),
                SendableValue::String(format!("mock {method} {url}").into()),
            ])),
            IoRequest::ReadFile { path } => IoResult::Ok(SendableValue::String(
                format!("mock contents of {path}").into(),
            )),
        };
        self.completed.lock().unwrap().push((token, result));
    }

    fn poll(&self) -> Vec<(IoToken, IoResult)> {
        let mut completed = self.completed.lock().unwrap();
        std::mem::take(&mut *completed)
    }
}

/// I/O backend with a fixed pool of worker threads.
/// Each worker has its own channel; requests are round-robin dispatched.
/// Results are pushed to a shared completion queue.
pub struct ThreadPoolIoBackend {
    senders: Vec<std::sync::mpsc::Sender<(IoToken, IoRequest)>>,
    next_worker: std::sync::atomic::AtomicUsize,
    completed: Arc<Mutex<Vec<(IoToken, IoResult)>>>,
}

impl ThreadPoolIoBackend {
    pub fn new(num_threads: usize) -> Self {
        let completed = Arc::new(Mutex::new(Vec::new()));
        let mut senders = Vec::with_capacity(num_threads);

        for _ in 0..num_threads {
            let (tx, rx) = std::sync::mpsc::channel::<(IoToken, IoRequest)>();
            let done = Arc::clone(&completed);
            std::thread::spawn(move || {
                while let Ok((token, request)) = rx.recv() {
                    let result = execute_io_request(request);
                    done.lock().unwrap().push((token, result));
                }
            });
            senders.push(tx);
        }

        Self {
            senders,
            next_worker: std::sync::atomic::AtomicUsize::new(0),
            completed,
        }
    }
}

impl IoBackend for ThreadPoolIoBackend {
    fn register(&self, token: IoToken, request: IoRequest) {
        let idx = self
            .next_worker
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.senders.len();
        let _ = self.senders[idx].send((token, request));
    }

    fn poll(&self) -> Vec<(IoToken, IoResult)> {
        let mut completed = self.completed.lock().unwrap();
        std::mem::take(&mut *completed)
    }
}

fn execute_io_request(request: IoRequest) -> IoResult {
    match request {
        IoRequest::Sleep(duration) => {
            std::thread::sleep(duration);
            IoResult::Ok(SendableValue::Unit)
        }
        IoRequest::HttpGet { url } => match aio_http_get(&url) {
            Ok(sv) => IoResult::Ok(sv),
            Err(e) => IoResult::Err(e),
        },
        IoRequest::Http {
            method,
            url,
            headers,
            body,
            format,
        } => match aio_http(&method, &url, &headers, &body, format) {
            Ok(sv) => IoResult::Ok(sv),
            Err(e) => IoResult::Err(e),
        },
        IoRequest::ReadFile { path } => match std::fs::read_to_string(&path) {
            Ok(contents) => IoResult::Ok(SendableValue::String(contents.into())),
            Err(e) => IoResult::Err(format!("read_file: {e}")),
        },
    }
}

/// Dispatch a ureq HTTP request. Returns the raw response.
pub(crate) fn dispatch_ureq(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    hiko_common::dispatch_ureq(method, url, headers, body)
}

/// Extract headers from a ureq HeaderMap as plain string pairs.
pub(crate) fn extract_headers(headers: &ureq::http::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect()
}

/// Async HTTP GET — runs on I/O pool thread.
fn aio_http_get(url: &str) -> Result<SendableValue, String> {
    aio_http("GET", url, &[], "", HttpResponseFormat::Text)
}

/// Async full HTTP — runs on I/O pool thread.
fn aio_http(
    method: &str,
    url: &str,
    req_headers: &[(String, String)],
    body: &str,
    format: HttpResponseFormat,
) -> Result<SendableValue, String> {
    let response = dispatch_ureq(method, url, req_headers, body)?;

    let status = SendableValue::Int(response.status().as_u16() as i64);
    let headers: Vec<SendableValue> = extract_headers(response.headers())
        .into_iter()
        .map(|(k, v)| {
            SendableValue::Tuple(vec![
                SendableValue::String(k.into()),
                SendableValue::String(v.into()),
            ])
        })
        .collect();

    let resp_body = match format {
        HttpResponseFormat::Text => {
            let s = response
                .into_body()
                .read_to_string()
                .map_err(|e| format!("http: {e}"))?;
            SendableValue::String(s.into())
        }
        HttpResponseFormat::Json => {
            let s = response
                .into_body()
                .read_to_string()
                .map_err(|e| format!("http_json: {e}"))?;
            let parsed: serde_json::Value =
                serde_json::from_str(&s).map_err(|e| format!("http_json: {e}"))?;
            json_value_to_sendable(&parsed)
        }
        HttpResponseFormat::Msgpack => {
            let reader = response.into_body().into_reader();
            let parsed: serde_json::Value =
                rmp_serde::from_read(reader).map_err(|e| format!("http_msgpack: {e}"))?;
            json_value_to_sendable(&parsed)
        }
        HttpResponseFormat::Bytes => {
            let mut buf = Vec::new();
            response
                .into_body()
                .into_reader()
                .read_to_end(&mut buf)
                .map_err(|e| format!("http_bytes: {e}"))?;
            SendableValue::Bytes(buf.into())
        }
    };

    Ok(SendableValue::Tuple(vec![
        status,
        SendableValue::List(headers),
        resp_body,
    ]))
}

/// Convert a serde_json::Value to SendableValue (for async JSON/msgpack parsing).
fn json_value_to_sendable(v: &serde_json::Value) -> SendableValue {
    match v {
        serde_json::Value::Null => SendableValue::Unit,
        serde_json::Value::Bool(b) => SendableValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SendableValue::Int(i)
            } else {
                SendableValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => SendableValue::String(s.clone().into()),
        serde_json::Value::Array(arr) => {
            SendableValue::List(arr.iter().map(json_value_to_sendable).collect())
        }
        serde_json::Value::Object(obj) => SendableValue::List(
            obj.iter()
                .map(|(k, val)| {
                    SendableValue::Tuple(vec![
                        SendableValue::String(k.clone().into()),
                        json_value_to_sendable(val),
                    ])
                })
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_sleep_completes_immediately() {
        let backend = MockIoBackend::new();
        backend.register(IoToken(1), IoRequest::Sleep(Duration::from_millis(100)));
        let results = backend.poll();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, IoToken(1));
        assert!(matches!(results[0].1, IoResult::Ok(SendableValue::Unit)));
    }

    #[test]
    fn test_mock_poll_drains() {
        let backend = MockIoBackend::new();
        backend.register(IoToken(1), IoRequest::Sleep(Duration::from_millis(0)));
        let r1 = backend.poll();
        assert_eq!(r1.len(), 1);
        let r2 = backend.poll();
        assert_eq!(r2.len(), 0);
    }
}
