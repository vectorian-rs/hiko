use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("regex_match", "capabilities.regex.regex_match"),
    BuiltinMeta::core("regex_replace", "capabilities.regex.regex_replace"),
];
