use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;
const RUNTIME: BuiltinSurface = BuiltinSurface::RuntimeOnly;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "epoch",
        capability_path: Some("capabilities.time.epoch"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "epoch_ms",
        capability_path: Some("capabilities.time.epoch_ms"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "monotonic_ms",
        capability_path: Some("capabilities.time.monotonic_ms"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "sleep",
        capability_path: Some("capabilities.time.sleep"),
        in_core_default: true,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "date_utc_tz",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_local_tz",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_timezone_of",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_fixed_offset",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_utc_now",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_now_in",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_from_instant",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_to_epoch_ms",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_to_timezone",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_in_timezone",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_year",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_month",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_day",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_hour",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_minute",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_second",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_millisecond",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_weekday",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_to_rfc3339",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_to_rfc2822",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_format",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_parse_rfc3339",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
    BuiltinMeta {
        name: "date_parse_rfc9557",
        capability_path: None,
        in_core_default: true,
        surface: RUNTIME,
    },
];
