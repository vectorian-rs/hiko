use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("epoch", "capabilities.time.epoch"),
    BuiltinMeta::core("epoch_ms", "capabilities.time.now"),
    BuiltinMeta::core("monotonic_ms", "capabilities.time.monotonic_now"),
    BuiltinMeta::core("sleep", "capabilities.time.sleep"),
];
