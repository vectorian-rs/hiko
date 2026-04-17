mod bytes;
mod convert;
mod env;
mod exec;
mod filesystem;
mod hash;
mod http;
mod json;
mod math;
mod path;
mod process;
mod random;
mod regex;
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

pub const BUILTIN_FAMILIES: &[&[BuiltinMeta]] = &[
    stdio::BUILTINS,
    convert::BUILTINS,
    string::BUILTINS,
    regex::BUILTINS,
    json::BUILTINS,
    math::BUILTINS,
    bytes::BUILTINS,
    hash::BUILTINS,
    random::BUILTINS,
    env::BUILTINS,
    time::BUILTINS,
    process::BUILTINS,
    path::BUILTINS,
    filesystem::BUILTINS,
    http::BUILTINS,
    exec::BUILTINS,
    system::BUILTINS,
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
