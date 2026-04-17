use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta {
    name: "exec",
    capability_path: Some("capabilities.exec.exec"),
    in_core_default: false,
    surface: PUBLIC,
}];
