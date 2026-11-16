use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::runtime_alias("date_utc_tz", "__Builtin.Date", "Std.Date.utc"),
    BuiltinMeta::runtime_alias("date_local_tz", "__Builtin.Date", "Std.Date.local_timezone"),
    BuiltinMeta::runtime_alias("date_timezone_of", "__Builtin.Date", "Std.Date.timezone_of"),
    BuiltinMeta::runtime_alias(
        "date_fixed_offset",
        "__Builtin.Date",
        "Std.Date.fixed_offset",
    ),
    BuiltinMeta::runtime_alias("date_utc_now", "__Builtin.Date", "Std.Date.utc_now"),
    BuiltinMeta::runtime_alias("date_now_in", "__Builtin.Date", "Std.Date.now_in"),
    BuiltinMeta::runtime_alias(
        "date_from_instant",
        "__Builtin.Date",
        "Std.Date.from_instant",
    ),
    BuiltinMeta::runtime_alias("date_to_epoch_ms", "__Builtin.Date", "Std.Date.to_instant"),
    BuiltinMeta::runtime_alias("date_to_timezone", "__Builtin.Date", "Std.Date.to_timezone"),
    BuiltinMeta::runtime_alias("date_in_timezone", "__Builtin.Date", "Std.Date.in_timezone"),
    BuiltinMeta::runtime_alias("date_year", "__Builtin.Date", "Std.Date.year"),
    BuiltinMeta::runtime_alias("date_month", "__Builtin.Date", "Std.Date.month"),
    BuiltinMeta::runtime_alias("date_day", "__Builtin.Date", "Std.Date.day"),
    BuiltinMeta::runtime_alias("date_hour", "__Builtin.Date", "Std.Date.hour"),
    BuiltinMeta::runtime_alias("date_minute", "__Builtin.Date", "Std.Date.minute"),
    BuiltinMeta::runtime_alias("date_second", "__Builtin.Date", "Std.Date.second"),
    BuiltinMeta::runtime_alias("date_millisecond", "__Builtin.Date", "Std.Date.millisecond"),
    BuiltinMeta::runtime_alias("date_weekday", "__Builtin.Date", "Std.Date.weekday"),
    BuiltinMeta::runtime_alias("date_to_rfc3339", "__Builtin.Date", "Std.Date.to_rfc3339"),
    BuiltinMeta::runtime_alias("date_to_rfc2822", "__Builtin.Date", "Std.Date.to_rfc2822"),
    BuiltinMeta::runtime_alias("date_format", "__Builtin.Date", "Std.Date.format"),
    BuiltinMeta::runtime_alias(
        "date_parse_rfc3339",
        "__Builtin.Date",
        "Std.Date.parse_rfc3339",
    ),
    BuiltinMeta::runtime_alias(
        "date_parse_rfc9557",
        "__Builtin.Date",
        "Std.Date.parse_rfc9557",
    ),
];
