use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta::module_only_opt_in(
    "exit",
    "capabilities.system.exit",
    &["__Builtin.System", "__Builtin.SystemExit"],
    "Std.System.exit",
)];
