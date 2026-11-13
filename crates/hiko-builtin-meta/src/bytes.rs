use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "bytes_length",
        "capabilities.bytes.length",
        &["__Builtin.Bytes"],
        "Std.Bytes.length",
    ),
    BuiltinMeta::module_only(
        "bytes_to_string",
        "capabilities.bytes.to_string",
        &["__Builtin.Bytes"],
        "Std.Bytes.to_string",
    ),
    BuiltinMeta::module_only(
        "string_to_bytes",
        "capabilities.bytes.of_string",
        &["__Builtin.Bytes"],
        "Std.Bytes.of_string",
    ),
    BuiltinMeta::module_only(
        "bytes_get",
        "capabilities.bytes.get",
        &["__Builtin.Bytes"],
        "Std.Bytes.get",
    ),
    BuiltinMeta::module_only(
        "bytes_slice",
        "capabilities.bytes.slice",
        &["__Builtin.Bytes"],
        "Std.Bytes.slice",
    ),
];
