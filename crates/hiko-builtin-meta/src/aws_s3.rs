use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[BuiltinMeta {
    name: "aws_s3_list_buckets",
    capability_path: Some("capabilities.aws.s3.list_buckets"),
    in_core_default: false,
    surface: PUBLIC,
}];
