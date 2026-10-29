use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("print", "capabilities.stdio.print"),
    BuiltinMeta::core("println", "capabilities.stdio.println"),
    BuiltinMeta::core("read_line", "capabilities.stdio.read_line"),
    BuiltinMeta::core("read_stdin", "capabilities.stdio.read_stdin"),
];
