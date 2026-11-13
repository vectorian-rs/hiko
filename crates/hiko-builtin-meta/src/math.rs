use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "sqrt",
        "capabilities.float.sqrt",
        &["__Builtin.Float"],
        "Std.Float.sqrt",
    ),
    BuiltinMeta::module_only(
        "abs_int",
        "capabilities.int.abs",
        &["__Builtin.Int"],
        "Std.Int.abs",
    ),
    BuiltinMeta::module_only(
        "abs_float",
        "capabilities.float.abs",
        &["__Builtin.Float"],
        "Std.Float.abs",
    ),
    BuiltinMeta::module_only(
        "floor",
        "capabilities.float.floor",
        &["__Builtin.Float"],
        "Std.Float.floor",
    ),
    BuiltinMeta::module_only(
        "ceil",
        "capabilities.float.ceil",
        &["__Builtin.Float"],
        "Std.Float.ceil",
    ),
];
