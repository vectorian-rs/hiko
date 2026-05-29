use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "aws_sqs_client",
        capability_path: Some("capabilities.aws.sqs.client"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "aws_sqs_list_queues",
        capability_path: Some("capabilities.aws.sqs.list_queues"),
        in_core_default: false,
        surface: PUBLIC,
    },
];
