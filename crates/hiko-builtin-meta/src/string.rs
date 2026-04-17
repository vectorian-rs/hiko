use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "string_length",
        capability_path: Some("capabilities.string.string_length"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "substring",
        capability_path: Some("capabilities.string.substring"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "string_contains",
        capability_path: Some("capabilities.string.string_contains"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "trim",
        capability_path: Some("capabilities.string.trim"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "split",
        capability_path: Some("capabilities.string.split"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "string_replace",
        capability_path: Some("capabilities.string.string_replace"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "starts_with",
        capability_path: Some("capabilities.string.starts_with"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "ends_with",
        capability_path: Some("capabilities.string.ends_with"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "to_upper",
        capability_path: Some("capabilities.string.to_upper"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "to_lower",
        capability_path: Some("capabilities.string.to_lower"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "string_join",
        capability_path: Some("capabilities.string.string_join"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
