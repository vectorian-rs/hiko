mod bytes;
mod convert;
mod date;
mod env;
mod exec;
mod filesystem;
mod hash;
mod http;
mod internal;
mod json;
mod math;
mod path;
mod process;
mod random;
mod regex;
mod signatures;
mod stdio;
mod string;
mod system;
mod testing;
mod time;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinSurface {
    Public,
    RuntimeOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinMeta {
    pub name: &'static str,
    pub capability_path: Option<&'static str>,
    pub in_core_default: bool,
    pub surface: BuiltinSurface,
}

// Each constructor is unused in builds that disable every family using it.
#[allow(dead_code)]
impl BuiltinMeta {
    /// Public builtin that the core-only VM registers by default.
    const fn core(name: &'static str, capability_path: &'static str) -> Self {
        Self {
            name,
            capability_path: Some(capability_path),
            in_core_default: true,
            surface: BuiltinSurface::Public,
        }
    }

    /// Public builtin that is only registered when a policy enables it.
    const fn opt_in(name: &'static str, capability_path: &'static str) -> Self {
        Self {
            name,
            capability_path: Some(capability_path),
            in_core_default: false,
            surface: BuiltinSurface::Public,
        }
    }

    /// Internal alias used by `__Builtin` modules. It has no policy leaf and
    /// is always registered.
    const fn runtime_alias(name: &'static str) -> Self {
        Self {
            name,
            capability_path: None,
            in_core_default: true,
            surface: BuiltinSurface::RuntimeOnly,
        }
    }
}

pub use internal::{
    INTERNAL_BUILTIN_PACKAGE, InternalBuiltinModule, internal_builtin_module,
    is_internal_builtin_package,
};
pub use signatures::{
    BuiltinTypeSignature, PROCESS_AWAIT_RESULT_CONSTRUCTORS, PROCESS_AWAIT_RESULT_ERR_TAG,
    PROCESS_AWAIT_RESULT_OK_TAG, PROCESS_JOIN_ERROR_ALREADY_JOINED_TAG,
    PROCESS_JOIN_ERROR_CANCELLED_TAG, PROCESS_JOIN_ERROR_CONSTRUCTORS,
    PROCESS_JOIN_ERROR_FUEL_EXHAUSTED_TAG, PROCESS_JOIN_ERROR_HEAP_LIMIT_TAG,
    PROCESS_JOIN_ERROR_RUNTIME_ERROR_TAG, builtin_type_signature, builtin_type_signatures,
};

const BUILTIN_FAMILIES: &[&[BuiltinMeta]] = &[
    #[cfg(feature = "builtin-stdio")]
    stdio::BUILTINS,
    #[cfg(feature = "builtin-convert")]
    convert::BUILTINS,
    #[cfg(feature = "builtin-string")]
    string::BUILTINS,
    #[cfg(feature = "builtin-regex")]
    regex::BUILTINS,
    #[cfg(feature = "builtin-json")]
    json::BUILTINS,
    #[cfg(feature = "builtin-math")]
    math::BUILTINS,
    #[cfg(feature = "builtin-bytes")]
    bytes::BUILTINS,
    #[cfg(feature = "builtin-hash")]
    hash::BUILTINS,
    #[cfg(feature = "builtin-random")]
    random::BUILTINS,
    #[cfg(feature = "builtin-env")]
    env::BUILTINS,
    #[cfg(feature = "builtin-time")]
    time::BUILTINS,
    #[cfg(feature = "builtin-date")]
    date::BUILTINS,
    #[cfg(feature = "builtin-process")]
    process::BUILTINS,
    #[cfg(feature = "builtin-path")]
    path::BUILTINS,
    #[cfg(feature = "builtin-filesystem")]
    filesystem::BUILTINS,
    #[cfg(feature = "builtin-http")]
    http::BUILTINS,
    #[cfg(feature = "builtin-exec")]
    exec::BUILTINS,
    #[cfg(feature = "builtin-system")]
    system::BUILTINS,
    #[cfg(feature = "builtin-testing")]
    testing::BUILTINS,
];

/// Every builtin family with the Cargo feature that compiles it in, whether
/// or not this build enables that feature.
const ALL_FAMILIES: &[(&str, &[BuiltinMeta])] = &[
    ("builtin-stdio", stdio::BUILTINS),
    ("builtin-convert", convert::BUILTINS),
    ("builtin-string", string::BUILTINS),
    ("builtin-regex", regex::BUILTINS),
    ("builtin-json", json::BUILTINS),
    ("builtin-math", math::BUILTINS),
    ("builtin-bytes", bytes::BUILTINS),
    ("builtin-hash", hash::BUILTINS),
    ("builtin-random", random::BUILTINS),
    ("builtin-env", env::BUILTINS),
    ("builtin-time", time::BUILTINS),
    ("builtin-date", date::BUILTINS),
    ("builtin-process", process::BUILTINS),
    ("builtin-path", path::BUILTINS),
    ("builtin-filesystem", filesystem::BUILTINS),
    ("builtin-http", http::BUILTINS),
    ("builtin-exec", exec::BUILTINS),
    ("builtin-system", system::BUILTINS),
    ("builtin-testing", testing::BUILTINS),
];

pub fn builtins() -> impl Iterator<Item = &'static BuiltinMeta> {
    BUILTIN_FAMILIES.iter().flat_map(|family| family.iter())
}

pub fn builtin_meta(name: &str) -> Option<&'static BuiltinMeta> {
    builtins().find(|meta| meta.name == name)
}

/// The Cargo feature that provides `name` when this build leaves it out, so
/// errors can say why a known builtin is unbound.
pub fn missing_builtin_feature(name: &str) -> Option<&'static str> {
    if builtin_meta(name).is_some() {
        return None;
    }
    ALL_FAMILIES
        .iter()
        .find(|(_, family)| family.iter().any(|meta| meta.name == name))
        .map(|(feature, _)| *feature)
}

pub fn capability_path_for_builtin(name: &str) -> Option<&'static str> {
    builtin_meta(name).and_then(|meta| meta.capability_path)
}

pub fn core_builtin_names() -> impl Iterator<Item = &'static str> {
    builtins()
        .filter(|meta| meta.in_core_default)
        .map(|meta| meta.name)
}

pub fn unrestricted_runtime_builtin_names() -> impl Iterator<Item = &'static str> {
    builtins()
        .filter(|meta| {
            meta.in_core_default
                && meta.surface == BuiltinSurface::RuntimeOnly
                && meta.capability_path.is_none()
        })
        .map(|meta| meta.name)
}
