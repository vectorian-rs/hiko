use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta::module_only(
    "path_join",
    "capabilities.path.join",
    &[
        "__Builtin.Filesystem",
        "__Builtin.Path",
        "__Builtin.PathJoin",
    ],
    "Std.Path.join",
)];
