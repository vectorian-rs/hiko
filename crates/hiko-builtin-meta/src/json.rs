use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "json_parse",
        capability_path: Some("capabilities.json.json_parse"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "json_to_string",
        capability_path: Some("capabilities.json.json_to_string"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "json_get",
        capability_path: Some("capabilities.json.json_get"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "json_keys",
        capability_path: Some("capabilities.json.json_keys"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "json_length",
        capability_path: Some("capabilities.json.json_length"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
