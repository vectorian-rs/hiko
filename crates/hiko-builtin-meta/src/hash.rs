use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta::module_only(
    "blake3",
    "capabilities.hash.blake3.hash",
    &["__Builtin.Hash"],
    "Hash.Blake3.hash",
)];
