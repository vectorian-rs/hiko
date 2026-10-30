//! Per-operation I/O timeouts and size limits.
//!
//! These bound a single builtin call. They sit beside the cumulative
//! `max_io_bytes` budget: each read is limited to whichever of the two is
//! smaller, and the error names the limit that was hit.

use std::time::Duration;

pub use hiko_common::HttpTimeouts;

pub const DEFAULT_HTTP_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_HTTP_CONNECT_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_EXEC_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_MAX_HTTP_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_FILE_READ_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_EXEC_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MAX_CONCURRENT_IO: u64 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoLimits {
    /// Whole-request HTTP timeout, including reading the response body.
    pub http_timeout: Duration,
    /// HTTP connect timeout.
    pub http_connect_timeout: Duration,
    /// Wall-clock timeout for one `exec` call.
    pub exec_timeout: Duration,
    /// Maximum bytes read from one HTTP response body.
    pub max_http_response_bytes: u64,
    /// Maximum bytes read from one file.
    pub max_file_read_bytes: u64,
    /// Maximum bytes read from each of `exec`'s stdout and stderr.
    pub max_exec_output_bytes: u64,
    /// Maximum async I/O requests one process tree (a root process and the
    /// processes it spawns) has running at once. Further requests wait in the
    /// queue, and their deadlines keep running.
    pub max_concurrent_io: u64,
}

impl Default for IoLimits {
    fn default() -> Self {
        Self {
            http_timeout: Duration::from_millis(DEFAULT_HTTP_TIMEOUT_MS),
            http_connect_timeout: Duration::from_millis(DEFAULT_HTTP_CONNECT_TIMEOUT_MS),
            exec_timeout: Duration::from_millis(DEFAULT_EXEC_TIMEOUT_MS),
            max_http_response_bytes: DEFAULT_MAX_HTTP_RESPONSE_BYTES,
            max_file_read_bytes: DEFAULT_MAX_FILE_READ_BYTES,
            max_exec_output_bytes: DEFAULT_MAX_EXEC_OUTPUT_BYTES,
            max_concurrent_io: DEFAULT_MAX_CONCURRENT_IO,
        }
    }
}

impl IoLimits {
    pub fn http_timeouts(&self) -> HttpTimeouts {
        HttpTimeouts {
            connect: self.http_connect_timeout,
            global: self.http_timeout,
        }
    }
}

/// Byte limit for one streaming read, and the policy key that sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadLimit {
    pub max_bytes: u64,
    pub limit_name: &'static str,
}

impl ReadLimit {
    /// The smaller of a per-operation limit and the remaining cumulative
    /// `max_io_bytes` budget.
    pub fn new(per_op_bytes: u64, per_op_name: &'static str, remaining_io: Option<u64>) -> Self {
        match remaining_io {
            Some(remaining) if remaining < per_op_bytes => Self {
                max_bytes: remaining,
                limit_name: "max_io_bytes",
            },
            _ => Self {
                max_bytes: per_op_bytes,
                limit_name: per_op_name,
            },
        }
    }

    pub fn exceeded_message(&self, context: &str) -> String {
        format!(
            "{context}: result exceeds {} ({} bytes)",
            self.limit_name, self.max_bytes
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_limit_uses_smaller_of_per_op_and_remaining_budget() {
        let limit = ReadLimit::new(100, "max_file_read_bytes", None);
        assert_eq!(
            (limit.max_bytes, limit.limit_name),
            (100, "max_file_read_bytes")
        );

        let limit = ReadLimit::new(100, "max_file_read_bytes", Some(500));
        assert_eq!(
            (limit.max_bytes, limit.limit_name),
            (100, "max_file_read_bytes")
        );

        let limit = ReadLimit::new(100, "max_file_read_bytes", Some(40));
        assert_eq!((limit.max_bytes, limit.limit_name), (40, "max_io_bytes"));
    }
}
