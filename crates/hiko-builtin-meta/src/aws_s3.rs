use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::opt_in("aws_s3_client", "capabilities.aws.s3.client"),
    BuiltinMeta::opt_in("aws_s3_list_buckets", "capabilities.aws.s3.list_buckets"),
];
