use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta {
    name: "exit",
    capability_path: Some("capabilities.system.exit"),
    in_core_default: false,
    surface: PUBLIC,
}];
