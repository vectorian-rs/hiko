use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta {
    name: "blake3",
    capability_path: Some("capabilities.hash.blake3"),
    in_core_default: true,
    surface: PUBLIC,
}];
