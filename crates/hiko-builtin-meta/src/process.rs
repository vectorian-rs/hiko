use crate::{BuiltinMeta, BuiltinSurface};

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only(
        "spawn",
        "capabilities.fiber.spawn",
        "__Builtin.Fiber",
        "Std.Fiber.spawn",
    ),
    BuiltinMeta::module_only(
        "await_process",
        "capabilities.fiber.join_or_fail",
        "__Builtin.Fiber",
        "Std.Fiber.join_or_fail",
    ),
    // Runtime companion with its own leaf since the #186 key split: `join`
    // returns the typed result, `join_or_fail` panics on failure.
    BuiltinMeta {
        name: "await_process_result",
        capability_path: Some("capabilities.fiber.join"),
        in_core_default: true,
        surface: BuiltinSurface::RuntimeOnly,
    },
    BuiltinMeta {
        name: "fiber_await_all",
        capability_path: Some("capabilities.fiber.join_all"),
        in_core_default: true,
        surface: BuiltinSurface::RuntimeOnly,
    },
    BuiltinMeta::module_only(
        "cancel",
        "capabilities.fiber.cancel",
        "__Builtin.Fiber",
        "Std.Fiber.cancel",
    ),
    BuiltinMeta::module_only(
        "wait_any",
        "capabilities.fiber.wait_any",
        "__Builtin.Fiber",
        "Std.Fiber.any",
    ),
];
