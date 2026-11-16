use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::module_only_opt_in(
        "http_get",
        "capabilities.http.client.get",
        &["__Builtin.Http"],
        "Http.Client.get",
    ),
    BuiltinMeta::module_only_opt_in(
        "http",
        "capabilities.http.client.request",
        &["__Builtin.Http"],
        "Http.Client.request",
    ),
    BuiltinMeta::module_only_opt_in(
        "http_json",
        "capabilities.http.client.request_json",
        &["__Builtin.Http"],
        "Http.Client.request_json",
    ),
    BuiltinMeta::module_only_opt_in(
        "http_msgpack",
        "capabilities.http.client.request_msgpack",
        &["__Builtin.Http"],
        "Http.Client.request_msgpack",
    ),
    BuiltinMeta::module_only_opt_in(
        "http_bytes",
        "capabilities.http.client.request_bytes",
        &["__Builtin.Http"],
        "Http.Client.request_bytes",
    ),
];
