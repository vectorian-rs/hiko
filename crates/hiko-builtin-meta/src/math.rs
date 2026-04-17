use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "sqrt",
        capability_path: Some("capabilities.math.sqrt"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "abs_int",
        capability_path: Some("capabilities.math.abs_int"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "abs_float",
        capability_path: Some("capabilities.math.abs_float"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "floor",
        capability_path: Some("capabilities.math.floor"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "ceil",
        capability_path: Some("capabilities.math.ceil"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
