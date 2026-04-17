use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "int_to_string",
        capability_path: Some("capabilities.convert.int_to_string"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "float_to_string",
        capability_path: Some("capabilities.convert.float_to_string"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "string_to_int",
        capability_path: Some("capabilities.convert.string_to_int"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "char_to_int",
        capability_path: Some("capabilities.convert.char_to_int"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "int_to_char",
        capability_path: Some("capabilities.convert.int_to_char"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "int_to_float",
        capability_path: Some("capabilities.convert.int_to_float"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
