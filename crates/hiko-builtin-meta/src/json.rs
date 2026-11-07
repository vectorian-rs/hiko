use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "json_parse",
        "capabilities.json.json_parse",
        "__Builtin.Json",
        "Json.parse",
    ),
    BuiltinMeta::module_only(
        "json_to_string",
        "capabilities.json.json_to_string",
        "__Builtin.Json",
        "Json.to_string",
    ),
    BuiltinMeta::core("json_get", "capabilities.json.json_get"),
    BuiltinMeta::core("json_keys", "capabilities.json.json_keys"),
    BuiltinMeta::core("json_length", "capabilities.json.json_length"),
];
