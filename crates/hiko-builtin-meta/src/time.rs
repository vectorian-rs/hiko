use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("epoch", "capabilities.time.epoch"),
    BuiltinMeta::core("epoch_ms", "capabilities.time.epoch_ms"),
    BuiltinMeta::core("monotonic_ms", "capabilities.time.monotonic_ms"),
    BuiltinMeta::core("sleep", "capabilities.time.sleep"),
];
