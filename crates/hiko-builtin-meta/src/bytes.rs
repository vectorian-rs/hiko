use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "bytes_length",
        capability_path: Some("capabilities.bytes.bytes_length"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "bytes_to_string",
        capability_path: Some("capabilities.bytes.bytes_to_string"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "string_to_bytes",
        capability_path: Some("capabilities.bytes.string_to_bytes"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "bytes_get",
        capability_path: Some("capabilities.bytes.bytes_get"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "bytes_slice",
        capability_path: Some("capabilities.bytes.bytes_slice"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
