use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::opt_in(
        "aws_config_sso_profile",
        "capabilities.aws.config.sso_profile",
    ),
    BuiltinMeta::opt_in(
        "aws_config_instance_profile",
        "capabilities.aws.config.instance_profile",
    ),
];
