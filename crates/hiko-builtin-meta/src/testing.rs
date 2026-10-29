use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("panic", "capabilities.testing.panic"),
    BuiltinMeta::core("assert", "capabilities.testing.assert"),
    BuiltinMeta::core("assert_eq", "capabilities.testing.assert_eq"),
];
