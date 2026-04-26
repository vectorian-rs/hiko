#[cfg(feature = "builtin-bytes")]
mod bytes;
#[cfg(feature = "builtin-convert")]
mod convert;
#[cfg(feature = "builtin-env")]
mod env;
#[cfg(feature = "builtin-exec")]
mod exec;
#[cfg(feature = "builtin-filesystem")]
mod filesystem;
#[cfg(feature = "builtin-hash")]
mod hash;
#[cfg(feature = "builtin-http")]
mod http;
mod internal;
#[cfg(feature = "builtin-json")]
mod json;
#[cfg(feature = "builtin-math")]
mod math;
#[cfg(feature = "builtin-path")]
mod path;
#[cfg(feature = "builtin-process")]
mod process;
#[cfg(feature = "builtin-random")]
mod random;
#[cfg(feature = "builtin-regex")]
mod regex;
#[cfg(feature = "builtin-stdio")]
mod stdio;
#[cfg(feature = "builtin-string")]
mod string;
#[cfg(feature = "builtin-system")]
mod system;
#[cfg(feature = "builtin-testing")]
mod testing;
#[cfg(feature = "builtin-time")]
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

pub use internal::{
    INTERNAL_BUILTIN_PACKAGE, InternalBuiltinModule, internal_builtin_module,
    internal_builtin_modules, is_internal_builtin_package,
};

pub const BUILTIN_FAMILIES: &[&[BuiltinMeta]] = &[
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

pub fn builtins() -> impl Iterator<Item = &'static BuiltinMeta> {
    BUILTIN_FAMILIES.iter().flat_map(|family| family.iter())
}

pub fn builtin_meta(name: &str) -> Option<&'static BuiltinMeta> {
    builtins().find(|meta| meta.name == name)
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

pub fn public_builtin_names() -> impl Iterator<Item = &'static str> {
    builtins()
        .filter(|meta| meta.surface == BuiltinSurface::Public)
        .map(|meta| meta.name)
}

pub fn is_public_builtin(name: &str) -> bool {
    builtin_meta(name)
        .map(|meta| meta.surface == BuiltinSurface::Public)
        .unwrap_or(false)
}
