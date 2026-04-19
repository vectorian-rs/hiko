#[cfg(feature = "hash")]
mod hash;
#[cfg(feature = "http")]
mod http;

#[cfg(feature = "hash")]
pub use hash::blake3_hex;
#[cfg(feature = "http")]
pub use http::{dispatch_ureq, http_get_text};
