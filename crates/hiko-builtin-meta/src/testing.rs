use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "panic",
        "capabilities.system.panic",
        &["__Builtin.Testing", "__Builtin.SystemPanic"],
        "Std.System.panic",
    ),
    BuiltinMeta::module_only(
        "assert",
        "capabilities.testing.check",
        &["__Builtin.Testing"],
        "Std.Testing.check",
    ),
    BuiltinMeta::module_only(
        "assert_eq",
        "capabilities.testing.equal",
        &["__Builtin.Testing"],
        "Std.Testing.equal",
    ),
];
