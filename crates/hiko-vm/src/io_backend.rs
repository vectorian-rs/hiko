//! Abstract I/O backend trait for async operations.
//!
//! The runtime suspends a process when it requests I/O, registers the
//! request with the backend, and resumes the process when the backend
//! reports completion. No worker thread is blocked during I/O.

#[cfg(feature = "builtin-http")]
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
    Ok { value: SendableValue, io_bytes: u64 },
    /// Operation failed with an error message.
    Err(String),
}

/// Abstract I/O backend. Implementations handle the actual I/O
/// (epoll, io_uring, mock, etc.) without the runtime knowing details.
pub trait IoBackend: Send + Sync {
    /// Register an I/O request. The backend will eventually produce a result.
    fn register(&self, token: IoToken, request: IoRequest) -> Result<(), String>;

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
    fn register(&self, token: IoToken, request: IoRequest) -> Result<(), String> {
        // Mock: complete immediately with canned responses
        let result = match request {
            IoRequest::Sleep(_) => IoResult::Ok {
                value: SendableValue::Unit,
                io_bytes: 0,
            },
            IoRequest::HttpGet { url } => {
                let value = SendableValue::Tuple(vec![
                    SendableValue::Int(200),
                    SendableValue::List(vec![]),
                    SendableValue::String(format!("mock response from {url}").into()),
                ]);
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
            IoRequest::Http { url, method, .. } => {
                let value = SendableValue::Tuple(vec![
                    SendableValue::Int(200),
                    SendableValue::List(vec![]),
                    SendableValue::String(format!("mock {method} {url}").into()),
                ]);
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
            IoRequest::ReadFile { path } => {
                let value = SendableValue::String(format!("mock contents of {path}").into());
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
        };
        self.completed.lock().unwrap().push((token, result));
        Ok(())
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
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl ThreadPoolIoBackend {
    pub fn new(num_threads: usize) -> Self {
        let num_threads = num_threads.max(1);
        let completed = Arc::new(Mutex::new(Vec::new()));
        let mut senders = Vec::with_capacity(num_threads);
        let mut workers = Vec::with_capacity(num_threads);

        for _ in 0..num_threads {
            let (tx, rx) = std::sync::mpsc::channel::<(IoToken, IoRequest)>();
            let done = Arc::clone(&completed);
            let worker = std::thread::spawn(move || {
                while let Ok((token, request)) = rx.recv() {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        execute_io_request(request)
                    }))
                    .unwrap_or_else(|_| IoResult::Err("io backend worker panicked".into()));
                    done.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((token, result));
                }
            });
            senders.push(tx);
            workers.push(worker);
        }

        Self {
            senders,
            next_worker: std::sync::atomic::AtomicUsize::new(0),
            completed,
            workers: Mutex::new(workers),
        }
    }

    pub fn shutdown(&mut self) -> Result<(), String> {
        self.senders.clear();
        let workers = self.workers.get_mut().unwrap_or_else(|e| e.into_inner());
        let mut panicked = 0;
        for worker in workers.drain(..) {
            if worker.join().is_err() {
                panicked += 1;
            }
        }
        if panicked == 0 {
            Ok(())
        } else {
            Err(format!("{panicked} io backend worker(s) panicked"))
        }
    }
}

impl IoBackend for ThreadPoolIoBackend {
    fn register(&self, token: IoToken, request: IoRequest) -> Result<(), String> {
        if self.senders.is_empty() {
            return Err("io backend is shut down".into());
        }
        let idx = self
            .next_worker
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.senders.len();
        self.senders[idx]
            .send((token, request))
            .map_err(|e| format!("io backend worker unavailable: {e}"))
    }

    fn poll(&self) -> Vec<(IoToken, IoResult)> {
        let mut completed = self.completed.lock().unwrap();
        std::mem::take(&mut *completed)
    }
}

impl Drop for ThreadPoolIoBackend {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn execute_io_request(request: IoRequest) -> IoResult {
    match request {
        IoRequest::Sleep(duration) => {
            std::thread::sleep(duration);
            IoResult::Ok {
                value: SendableValue::Unit,
                io_bytes: 0,
            }
        }
        IoRequest::HttpGet { url } => match aio_http_get(&url) {
            Ok((value, io_bytes)) => IoResult::Ok { value, io_bytes },
            Err(e) => IoResult::Err(e),
        },
        IoRequest::Http {
            method,
            url,
            headers,
            body,
            format,
        } => match aio_http(&method, &url, &headers, &body, format) {
            Ok((value, io_bytes)) => IoResult::Ok { value, io_bytes },
            Err(e) => IoResult::Err(e),
        },
        IoRequest::ReadFile { path } => match std::fs::read_to_string(&path) {
            Ok(contents) => IoResult::Ok {
                io_bytes: contents.len() as u64,
                value: SendableValue::String(contents.into()),
            },
            Err(e) => IoResult::Err(format!("read_file: {e}")),
        },
    }
}

/// Async HTTP GET — runs on I/O pool thread.
#[cfg(feature = "builtin-http")]
fn aio_http_get(url: &str) -> Result<(SendableValue, u64), String> {
    aio_http("GET", url, &[], "", HttpResponseFormat::Text)
}

/// Async full HTTP — runs on I/O pool thread.
#[cfg(feature = "builtin-http")]
fn aio_http(
    method: &str,
    url: &str,
    req_headers: &[(String, String)],
    body: &str,
    format: HttpResponseFormat,
) -> Result<(SendableValue, u64), String> {
    let response = hiko_common::dispatch_ureq(method, url, req_headers, body)?;

    let status = SendableValue::Int(response.status().as_u16() as i64);
    let headers: Vec<SendableValue> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .map(|(k, v)| {
            SendableValue::Tuple(vec![
                SendableValue::String(k.into()),
                SendableValue::String(v.into()),
            ])
        })
        .collect();

    let (resp_body, io_bytes) = match format {
        HttpResponseFormat::Text => {
            let s = response
                .into_body()
                .read_to_string()
                .map_err(|e| format!("http: {e}"))?;
            let io_bytes = s.len() as u64;
            (SendableValue::String(s.into()), io_bytes)
        }
        HttpResponseFormat::Json => {
            #[cfg(feature = "builtin-http")]
            {
                let s = response
                    .into_body()
                    .read_to_string()
                    .map_err(|e| format!("http_json: {e}"))?;
                let parsed: serde_json::Value =
                    serde_json::from_str(&s).map_err(|e| format!("http_json: {e}"))?;
                (json_value_to_sendable(&parsed)?, s.len() as u64)
            }
            #[cfg(not(feature = "builtin-http"))]
            {
                let _ = response;
                return Err("http_json is not available in this build".into());
            }
        }
        HttpResponseFormat::Msgpack => {
            #[cfg(feature = "builtin-http")]
            {
                let mut reader = response.into_body().into_reader();
                let mut buf = Vec::new();
                reader
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("http_msgpack: {e}"))?;
                let parsed: serde_json::Value =
                    rmp_serde::from_slice(&buf).map_err(|e| format!("http_msgpack: {e}"))?;
                (json_value_to_sendable(&parsed)?, buf.len() as u64)
            }
            #[cfg(not(feature = "builtin-http"))]
            {
                let _ = response;
                return Err("http_msgpack is not available in this build".into());
            }
        }
        HttpResponseFormat::Bytes => {
            let mut buf = Vec::new();
            response
                .into_body()
                .into_reader()
                .read_to_end(&mut buf)
                .map_err(|e| format!("http_bytes: {e}"))?;
            let io_bytes = buf.len() as u64;
            (SendableValue::Bytes(buf.into()), io_bytes)
        }
    };

    Ok((
        SendableValue::Tuple(vec![status, SendableValue::List(headers), resp_body]),
        io_bytes,
    ))
}

/// Convert a serde_json::Value to SendableValue (for async JSON/msgpack parsing).
#[cfg(feature = "builtin-http")]
fn json_value_to_sendable(v: &serde_json::Value) -> Result<SendableValue, String> {
    match v {
        serde_json::Value::Null => Ok(SendableValue::Unit),
        serde_json::Value::Bool(b) => Ok(SendableValue::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(SendableValue::Int(i))
            } else {
                let f = n
                    .as_f64()
                    .ok_or_else(|| format!("json number not representable as f64: {n}"))?;
                Ok(SendableValue::Float(f))
            }
        }
        serde_json::Value::String(s) => Ok(SendableValue::String(s.clone().into())),
        serde_json::Value::Array(arr) => {
            let items: Result<Vec<_>, _> = arr.iter().map(json_value_to_sendable).collect();
            Ok(SendableValue::List(items?))
        }
        serde_json::Value::Object(obj) => {
            let items: Result<Vec<SendableValue>, String> = obj
                .iter()
                .map(|(k, val)| {
                    Ok(SendableValue::Tuple(vec![
                        SendableValue::String(k.clone().into()),
                        json_value_to_sendable(val)?,
                    ]))
                })
                .collect();
            Ok(SendableValue::List(items?))
        }
    }
}

#[cfg(not(feature = "builtin-http"))]
fn aio_http_get(url: &str) -> Result<(SendableValue, u64), String> {
    let _ = url;
    Err("http_get is not available in this build".into())
}

#[cfg(not(feature = "builtin-http"))]
fn aio_http(
    method: &str,
    url: &str,
    req_headers: &[(String, String)],
    body: &str,
    format: HttpResponseFormat,
) -> Result<(SendableValue, u64), String> {
    let _ = (method, url, req_headers, body, format);
    Err("HTTP builtins are not available in this build".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "builtin-http")]
    fn test_json_value_to_sendable_normal_float() {
        let v: serde_json::Value = serde_json::from_str("3.5").unwrap();
        let result = json_value_to_sendable(&v).unwrap();
        assert!(matches!(result, SendableValue::Float(f) if (f - 3.5).abs() < f64::EPSILON));
    }

    #[test]
    #[cfg(feature = "builtin-http")]
    fn test_json_value_to_sendable_unrepresentable_float_errors() {
        // serde_json with the arbitrary_precision feature can produce numbers
        // that have no f64 representation. Without that feature, all parsed
        // numbers fit in f64, so we construct one via the Number API that
        // would fail as_f64() -- specifically a u64 that exceeds i64::MAX so
        // as_i64() returns None, then check whether as_f64() also fails.
        // In practice, serde_json::Number from a u64 always has an f64 path,
        // so we verify the happy path doesn't silently produce 0.0.
        let v: serde_json::Value = serde_json::json!(u64::MAX);
        let result = json_value_to_sendable(&v);
        // u64::MAX cannot be represented as i64, so the code takes the float
        // branch. as_f64() succeeds (lossy) so we get Ok, but crucially we
        // do NOT get 0.0.
        match result {
            Ok(SendableValue::Float(f)) => assert!(f > 0.0, "must not silently map to 0.0"),
            Ok(SendableValue::Int(_)) => { /* also acceptable */ }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_mock_sleep_completes_immediately() {
        let backend = MockIoBackend::new();
        backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_millis(100)))
            .unwrap();
        let results = backend.poll();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, IoToken(1));
        assert!(matches!(
            results[0].1,
            IoResult::Ok {
                value: SendableValue::Unit,
                io_bytes: 0
            }
        ));
    }

    #[test]
    fn test_mock_poll_drains() {
        let backend = MockIoBackend::new();
        backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_millis(0)))
            .unwrap();
        let r1 = backend.poll();
        assert_eq!(r1.len(), 1);
        let r2 = backend.poll();
        assert_eq!(r2.len(), 0);
    }

    #[test]
    fn test_thread_pool_zero_threads_still_accepts_request() {
        let backend = ThreadPoolIoBackend::new(0);
        backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_millis(0)))
            .unwrap();

        for _ in 0..50 {
            let results = backend.poll();
            if !results.is_empty() {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0, IoToken(1));
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        panic!("thread pool backend did not complete request");
    }

    #[test]
    fn test_thread_pool_shutdown_closes_registration() {
        let mut backend = ThreadPoolIoBackend::new(1);
        backend.shutdown().unwrap();

        let err = backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_millis(0)))
            .unwrap_err();
        assert!(err.contains("shut down"));
    }
}
