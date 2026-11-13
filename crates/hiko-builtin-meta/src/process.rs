use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "spawn",
        "capabilities.fiber.spawn",
        &["__Builtin.Fiber"],
        "Std.Fiber.spawn",
    ),
    BuiltinMeta::module_only(
        "await_process",
        "capabilities.fiber.join_or_fail",
        &["__Builtin.Fiber"],
        "Std.Fiber.join_or_fail",
    ),
    // Runtime companion with its own leaf since the #186 key split: `join`
    // returns the typed result, `join_or_fail` panics on failure.
    BuiltinMeta::runtime_gated("await_process_result", "capabilities.fiber.join"),
    BuiltinMeta::runtime_gated("fiber_await_all", "capabilities.fiber.join_all"),
    BuiltinMeta::module_only(
        "cancel",
        "capabilities.fiber.cancel",
        &["__Builtin.Fiber"],
        "Std.Fiber.cancel",
    ),
    BuiltinMeta::module_only(
        "wait_any",
        "capabilities.fiber.wait_any",
        &["__Builtin.Fiber"],
        "Std.Fiber.any",
    ),
];
