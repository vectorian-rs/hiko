use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "regex_match",
        "capabilities.regex.matches",
        &["__Builtin.Regex"],
        "Std.Regex.matches",
    ),
    BuiltinMeta::module_only(
        "regex_replace",
        "capabilities.regex.replace",
        &["__Builtin.Regex"],
        "Std.Regex.replace",
    ),
];
