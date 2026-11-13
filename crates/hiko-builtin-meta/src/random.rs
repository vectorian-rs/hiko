use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("random_bytes", "capabilities.random.entropy"),
    BuiltinMeta::core("rng_seed", "capabilities.random.seed"),
    BuiltinMeta::core("rng_bytes", "capabilities.random.bytes"),
    BuiltinMeta::core("rng_int", "capabilities.random.int_between"),
];
