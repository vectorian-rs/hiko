use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "epoch",
        "capabilities.time.epoch",
        &["__Builtin.Time"],
        "Std.Time.now",
    ),
    BuiltinMeta::module_only(
        "epoch_ms",
        "capabilities.time.now",
        &["__Builtin.Time"],
        "Std.Time.now",
    ),
    BuiltinMeta::module_only(
        "monotonic_ms",
        "capabilities.time.monotonic_now",
        &["__Builtin.Time"],
        "Std.Time.monotonic_now",
    ),
    BuiltinMeta::module_only(
        "sleep",
        "capabilities.time.sleep",
        &["__Builtin.Time"],
        "Std.Time.sleep",
    ),
];
