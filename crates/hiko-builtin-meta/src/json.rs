use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "json_parse",
        "capabilities.json.parse",
        "__Builtin.Json",
        "Json.parse",
    ),
    BuiltinMeta::module_only(
        "json_to_string",
        "capabilities.json.to_string",
        "__Builtin.Json",
        "Json.to_string",
    ),
    BuiltinMeta::core("json_get", "capabilities.json.get"),
    BuiltinMeta::core("json_keys", "capabilities.json.keys"),
    BuiltinMeta::core("json_length", "capabilities.json.length"),
];
