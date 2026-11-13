use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("sqrt", "capabilities.float.sqrt"),
    BuiltinMeta::core("abs_int", "capabilities.int.abs"),
    BuiltinMeta::core("abs_float", "capabilities.float.abs"),
    BuiltinMeta::core("floor", "capabilities.float.floor"),
    BuiltinMeta::core("ceil", "capabilities.float.ceil"),
];
