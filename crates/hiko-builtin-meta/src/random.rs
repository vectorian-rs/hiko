use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("random_bytes", "capabilities.random.random_bytes"),
    BuiltinMeta::core("rng_seed", "capabilities.random.rng_seed"),
    BuiltinMeta::core("rng_bytes", "capabilities.random.rng_bytes"),
    BuiltinMeta::core("rng_int", "capabilities.random.rng_int"),
];
