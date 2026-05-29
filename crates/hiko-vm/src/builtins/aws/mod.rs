#[cfg(feature = "builtin-aws-config")]
mod config;
#[cfg(feature = "builtin-aws-s3")]
mod s3;
#[cfg(feature = "builtin-aws-sqs")]
mod sqs;

use super::*;

pub(crate) fn entries() -> Vec<(&'static str, BuiltinFn)> {
    let mut entries = Vec::new();
    #[cfg(feature = "builtin-aws-config")]
    entries.extend(config::entries());
    #[cfg(feature = "builtin-aws-s3")]
    entries.extend(s3::entries());
    #[cfg(feature = "builtin-aws-sqs")]
    entries.extend(sqs::entries());
    entries
}
