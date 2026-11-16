use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta::module_only_opt_in(
    "exec",
    "capabilities.exec.run",
    &["__Builtin.Exec"],
    "Std.Exec.run",
)];
