use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::opt_in("http_get", "capabilities.http.http_get"),
    BuiltinMeta::opt_in("http", "capabilities.http.http"),
    BuiltinMeta::opt_in("http_json", "capabilities.http.http_json"),
    BuiltinMeta::opt_in("http_msgpack", "capabilities.http.http_msgpack"),
    BuiltinMeta::opt_in("http_bytes", "capabilities.http.http_bytes"),
];
