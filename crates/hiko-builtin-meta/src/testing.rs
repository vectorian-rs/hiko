use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "panic",
        capability_path: Some("capabilities.testing.panic"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "assert",
        capability_path: Some("capabilities.testing.assert"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "assert_eq",
        capability_path: Some("capabilities.testing.assert_eq"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
