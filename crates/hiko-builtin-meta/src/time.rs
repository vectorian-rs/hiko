use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "epoch",
        capability_path: Some("capabilities.time.epoch"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "epoch_ms",
        capability_path: Some("capabilities.time.epoch_ms"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "monotonic_ms",
        capability_path: Some("capabilities.time.monotonic_ms"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "sleep",
        capability_path: Some("capabilities.time.sleep"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
