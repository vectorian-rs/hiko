use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta::module_only(
    "getenv",
    "capabilities.env.get",
    &["__Builtin.Env"],
    "Std.Env.get",
)];
