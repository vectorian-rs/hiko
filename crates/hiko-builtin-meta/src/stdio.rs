use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "print",
        capability_path: Some("capabilities.stdio.print"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "println",
        capability_path: Some("capabilities.stdio.println"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "read_line",
        capability_path: Some("capabilities.stdio.read_line"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "read_stdin",
        capability_path: Some("capabilities.stdio.read_stdin"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
