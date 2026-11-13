use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("regex_match", "capabilities.regex.matches"),
    BuiltinMeta::core("regex_replace", "capabilities.regex.replace"),
];
