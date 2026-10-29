use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("string_length", "capabilities.string.string_length"),
    BuiltinMeta::core("substring", "capabilities.string.substring"),
    BuiltinMeta::core("string_contains", "capabilities.string.string_contains"),
    BuiltinMeta::core("trim", "capabilities.string.trim"),
    BuiltinMeta::core("split", "capabilities.string.split"),
    BuiltinMeta::core("string_replace", "capabilities.string.string_replace"),
    BuiltinMeta::core("starts_with", "capabilities.string.starts_with"),
    BuiltinMeta::core("ends_with", "capabilities.string.ends_with"),
    BuiltinMeta::core("to_upper", "capabilities.string.to_upper"),
    BuiltinMeta::core("to_lower", "capabilities.string.to_lower"),
    BuiltinMeta::core("string_join", "capabilities.string.string_join"),
];
