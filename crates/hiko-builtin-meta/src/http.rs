use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "http_get",
        capability_path: Some("capabilities.http.http_get"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "http",
        capability_path: Some("capabilities.http.http"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "http_json",
        capability_path: Some("capabilities.http.http_json"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "http_msgpack",
        capability_path: Some("capabilities.http.http_msgpack"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "http_bytes",
        capability_path: Some("capabilities.http.http_bytes"),
        in_core_default: false,
        surface: PUBLIC,
    },
];
