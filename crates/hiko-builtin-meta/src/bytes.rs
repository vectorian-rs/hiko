use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("bytes_length", "capabilities.bytes.bytes_length"),
    BuiltinMeta::core("bytes_to_string", "capabilities.bytes.bytes_to_string"),
    BuiltinMeta::core("string_to_bytes", "capabilities.bytes.string_to_bytes"),
    BuiltinMeta::core("bytes_get", "capabilities.bytes.bytes_get"),
    BuiltinMeta::core("bytes_slice", "capabilities.bytes.bytes_slice"),
];
