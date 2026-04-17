use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "random_bytes",
        capability_path: Some("capabilities.random.random_bytes"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "rng_seed",
        capability_path: Some("capabilities.random.rng_seed"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "rng_bytes",
        capability_path: Some("capabilities.random.rng_bytes"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "rng_int",
        capability_path: Some("capabilities.random.rng_int"),
        in_core_default: true,
        surface: PUBLIC,
    },
];
