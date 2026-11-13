use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::opt_in("http_get", "capabilities.http.client.get"),
    BuiltinMeta::opt_in("http", "capabilities.http.client.request"),
    BuiltinMeta::opt_in("http_json", "capabilities.http.client.request_json"),
    BuiltinMeta::opt_in("http_msgpack", "capabilities.http.client.request_msgpack"),
    BuiltinMeta::opt_in("http_bytes", "capabilities.http.client.request_bytes"),
];
