#[cfg(feature = "builtin-aws-config")]
mod config;
#[cfg(feature = "builtin-aws-s3")]
mod s3;

use super::*;

pub(crate) fn entries() -> Vec<(&'static str, BuiltinFn)> {
    let mut entries = Vec::new();
    #[cfg(feature = "builtin-aws-config")]
    entries.extend(config::entries());
    #[cfg(feature = "builtin-aws-s3")]
    entries.extend(s3::entries());
    entries
}
