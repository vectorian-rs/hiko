use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta::module_only(
    "path_join",
    "capabilities.path.join",
    "__Builtin.Path",
    "Std.Path.join",
)];
