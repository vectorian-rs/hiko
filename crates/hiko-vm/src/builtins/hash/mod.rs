mod blake3;

use super::*;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    blake3::entries()
}
