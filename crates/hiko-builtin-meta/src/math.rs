use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("sqrt", "capabilities.math.sqrt"),
    BuiltinMeta::core("abs_int", "capabilities.math.abs_int"),
    BuiltinMeta::core("abs_float", "capabilities.math.abs_float"),
    BuiltinMeta::core("floor", "capabilities.math.floor"),
    BuiltinMeta::core("ceil", "capabilities.math.ceil"),
];
