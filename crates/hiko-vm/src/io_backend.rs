//! Abstract I/O backend trait for async operations.
//!
//! The runtime suspends a process when it requests I/O, registers the
//! request with the backend, and resumes the process when the backend
//! reports completion. No worker thread is blocked during I/O.

use std::sync::Mutex;
use std::time::Duration;

use crate::sendable::SendableValue;

/// Opaque token identifying an I/O operation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IoToken(pub u64);

/// The kind of I/O operation requested.
#[derive(Debug, Clone)]
pub enum IoRequest {
    /// Delay for a duration (async sleep).
    Sleep(Duration),
    /// Custom operation with a name and payload.
    Custom {
        operation: String,
        payload: SendableValue,
    },
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

impl MockIoBackend {
    pub fn new() -> Self {
        Self {
            completed: Mutex::new(Vec::new()),
        }
    }
}

impl IoBackend for MockIoBackend {
    fn register(&self, token: IoToken, request: IoRequest) {
        // Mock: complete immediately
        let result = match request {
            IoRequest::Sleep(_) => IoResult::Ok(SendableValue::Unit),
            IoRequest::Custom { payload, .. } => {
                // Echo the payload back
                IoResult::Ok(payload)
            }
        };
        self.completed.lock().unwrap().push((token, result));
    }

    fn poll(&self) -> Vec<(IoToken, IoResult)> {
        let mut completed = self.completed.lock().unwrap();
        std::mem::take(&mut *completed)
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
    fn test_mock_custom_echoes_payload() {
        let backend = MockIoBackend::new();
        backend.register(
            IoToken(2),
            IoRequest::Custom {
                operation: "test".into(),
                payload: SendableValue::Int(42),
            },
        );
        let results = backend.poll();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, IoResult::Ok(SendableValue::Int(42))));
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
