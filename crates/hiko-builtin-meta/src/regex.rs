use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "regex_match",
        capability_path: Some("capabilities.regex.regex_match"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "regex_replace",
        capability_path: Some("capabilities.regex.regex_replace"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
