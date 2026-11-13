use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "json_parse",
        "capabilities.json.parse",
        "__Builtin.Json",
        "Std.Json.parse",
    ),
    BuiltinMeta::module_only(
        "json_to_string",
        "capabilities.json.to_string",
        "__Builtin.Json",
        "Std.Json.to_string",
    ),
    BuiltinMeta::module_only(
        "json_get",
        "capabilities.json.get",
        "__Builtin.Json",
        "Std.Json.get",
    ),
    BuiltinMeta::module_only(
        "json_keys",
        "capabilities.json.keys",
        "__Builtin.Json",
        "Std.Json.keys",
    ),
    BuiltinMeta::module_only(
        "json_length",
        "capabilities.json.length",
        "__Builtin.Json",
        "Std.Json.length",
    ),
];
