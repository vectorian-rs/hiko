mod client;

use super::*;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    client::entries()
}
