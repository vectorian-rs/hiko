use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::opt_in("aws_sqs_client", "capabilities.aws.sqs.client"),
    BuiltinMeta::opt_in("aws_sqs_list_queues", "capabilities.aws.sqs.list_queues"),
];
