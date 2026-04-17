use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "spawn",
        capability_path: Some("capabilities.process.spawn"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "await_process",
        capability_path: Some("capabilities.process.await_process"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
