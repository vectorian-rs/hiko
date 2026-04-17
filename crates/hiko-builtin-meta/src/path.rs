use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta {
    name: "path_join",
    capability_path: Some("capabilities.path.path_join"),
    in_core_default: true,
    surface: PUBLIC,
}];
