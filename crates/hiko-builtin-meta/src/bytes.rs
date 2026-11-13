use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("bytes_length", "capabilities.bytes.length"),
    BuiltinMeta::core("bytes_to_string", "capabilities.bytes.to_string"),
    BuiltinMeta::core("string_to_bytes", "capabilities.bytes.of_string"),
    BuiltinMeta::core("bytes_get", "capabilities.bytes.get"),
    BuiltinMeta::core("bytes_slice", "capabilities.bytes.slice"),
];
