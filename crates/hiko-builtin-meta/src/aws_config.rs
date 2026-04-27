use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta {
    name: "aws_config_sso_profile",
    capability_path: Some("capabilities.aws.config.sso_profile"),
    in_core_default: false,
    surface: PUBLIC,
}];
