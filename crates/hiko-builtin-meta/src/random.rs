use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "random_bytes",
        "capabilities.random.entropy",
        "__Builtin.Random",
        "Std.Random.entropy",
    ),
    BuiltinMeta::module_only(
        "rng_seed",
        "capabilities.random.seed",
        "__Builtin.Random",
        "Std.Random.seed",
    ),
    BuiltinMeta::module_only(
        "rng_bytes",
        "capabilities.random.bytes",
        "__Builtin.Random",
        "Std.Random.bytes",
    ),
    BuiltinMeta::module_only(
        "rng_int",
        "capabilities.random.int_between",
        "__Builtin.Random",
        "Std.Random.int_between",
    ),
];
