use std::time::Duration;

/// Connect and whole-request timeouts for an HTTP call.
///
/// `global` bounds the entire call, including reading the response body, so a
/// peer that accepts the connection and then stalls cannot block the caller
/// for longer than this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpTimeouts {
    pub connect: Duration,
    pub global: Duration,
}
