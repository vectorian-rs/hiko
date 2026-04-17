use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta {
    name: "getenv",
    capability_path: Some("capabilities.env.getenv"),
    in_core_default: true,
    surface: PUBLIC,
}];
