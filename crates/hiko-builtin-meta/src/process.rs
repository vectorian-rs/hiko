use crate::{BuiltinMeta, BuiltinSurface};

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::core("spawn", "capabilities.process.spawn"),
    BuiltinMeta::core("await_process", "capabilities.process.await_process"),
    // Runtime-only companion of `await_process`, enabled by the same policy leaf.
    BuiltinMeta {
        name: "await_process_result",
        capability_path: Some("capabilities.process.await_process"),
        in_core_default: true,
        surface: BuiltinSurface::RuntimeOnly,
    },
    BuiltinMeta::core("cancel", "capabilities.process.cancel"),
    BuiltinMeta::core("wait_any", "capabilities.process.wait_any"),
];
