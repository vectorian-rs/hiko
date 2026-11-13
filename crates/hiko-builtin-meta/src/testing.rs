use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("panic", "capabilities.system.panic"),
    BuiltinMeta::core("assert", "capabilities.testing.check"),
    BuiltinMeta::core("assert_eq", "capabilities.testing.equal"),
];
