pub const INTERNAL_BUILTIN_PACKAGE: &str = "__Builtin";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InternalBuiltinModule {
    pub leaf_name: &'static str,
    pub import_name: &'static str,
    pub feature_name: &'static str,
    pub enabled: bool,
    pub source: &'static str,
}

const FILESYSTEM_SOURCE: &str = r#"structure BuiltinFilesystem = struct
  val read_text_raw = read_file
  val read_bytes_raw = read_file_bytes
  val write_text_raw = write_file
  val exists_raw = file_exists
  val list_entries_raw = list_dir
  val remove_raw = remove_file
  val create_directory_raw = create_dir
  val is_directory_raw = is_dir
  val is_regular_file_raw = is_file
  val glob_raw = glob
  val walk_raw = walk_dir
end
"#;

const PATH_SOURCE: &str = r#"structure BuiltinPath = struct
  val join_raw = path_join
end
"#;

const JSON_SOURCE: &str = r#"structure BuiltinJson = struct
  val parse_raw = json_parse
  val to_string_raw = json_to_string
  val get_raw = json_get
  val keys_raw = json_keys
  val length_raw = json_length
end
"#;

const HASH_SOURCE: &str = r#"structure BuiltinHash = struct
  val blake3_raw = blake3
end
"#;

const STDIO_SOURCE: &str = r#"structure BuiltinStdio = struct
  val print_raw = print
  val println_raw = println
  val read_line_raw = read_line
  val read_stdin_raw = read_stdin
end
"#;

const HTTP_SOURCE: &str = r#"structure BuiltinHttp = struct
  val get_raw = http_get
  val request_raw = http
  val request_json_raw = http_json
  val request_msgpack_raw = http_msgpack
  val request_bytes_raw = http_bytes
end
"#;

const PROCESS_SOURCE: &str = r#"structure BuiltinProcess = struct
  datatype error =
      RuntimeError of string
    | Cancelled
    | FuelExhausted
    | AlreadyJoined
    (* HeapObjectLimitExceeded (live, limit) *)
    | HeapObjectLimitExceeded of int * int

  datatype 'a await_result =
      Ok of 'a
    | Err of error

  val spawn_raw = spawn
  val await_raw = await_process
  fun await_result_raw pid =
    await_process_result pid
  val cancel_raw = cancel
  val wait_any_raw = wait_any
end
"#;

const EXEC_SOURCE: &str = r#"structure BuiltinExec = struct
  val exec_raw = exec
end
"#;

const SYSTEM_SOURCE: &str = r#"structure BuiltinSystem = struct
  val exit_raw = exit
end
"#;

const TIME_SOURCE: &str = r#"structure BuiltinTime = struct
  val epoch_raw = epoch
  val epoch_ms_raw = epoch_ms
  val monotonic_ms_raw = monotonic_ms
  val sleep_raw = sleep
end
"#;

const NUMERIC_SOURCE: &str = r#"structure BuiltinNumeric = struct
  val int32_min_value_raw = numeric_int32_min_value
  val int32_max_value_raw = numeric_int32_max_value
  val int32_of_int_raw = numeric_int32_of_int
  val int32_checked_of_int_raw = numeric_int32_checked_of_int
  val int32_to_int_raw = numeric_int32_to_int
  val int32_add_raw = numeric_int32_add
  val int32_checked_add_raw = numeric_int32_checked_add
  val int32_wrapping_add_raw = numeric_int32_wrapping_add
  val int32_saturating_add_raw = numeric_int32_saturating_add
  val int32_sub_raw = numeric_int32_sub
  val int32_mul_raw = numeric_int32_mul
  val int32_div_raw = numeric_int32_div
  val int32_rem_raw = numeric_int32_rem
  val int32_neg_raw = numeric_int32_neg

  val word32_min_value_raw = numeric_word32_min_value
  val word32_max_value_raw = numeric_word32_max_value
  val word32_of_word_raw = numeric_word32_of_word
  val word32_checked_of_word_raw = numeric_word32_checked_of_word
  val word32_of_int_raw = numeric_word32_of_int
  val word32_checked_of_int_raw = numeric_word32_checked_of_int
  val word32_to_word_raw = numeric_word32_to_word
  val word32_to_int_raw = numeric_word32_to_int
  val word32_add_raw = numeric_word32_add
  val word32_checked_add_raw = numeric_word32_checked_add
  val word32_saturating_add_raw = numeric_word32_saturating_add
  val word32_sub_raw = numeric_word32_sub
  val word32_mul_raw = numeric_word32_mul
  val word32_div_raw = numeric_word32_div
  val word32_rem_raw = numeric_word32_rem

  val float32_of_float_raw = numeric_float32_of_float
  val float32_to_float_raw = numeric_float32_to_float
  val float32_neg_raw = numeric_float32_neg
  val float32_add_raw = numeric_float32_add
  val float32_sub_raw = numeric_float32_sub
  val float32_mul_raw = numeric_float32_mul
  val float32_div_raw = numeric_float32_div
end
"#;

const DATE_SOURCE: &str = r#"structure BuiltinDate = struct
  val utc_tz_raw = date_utc_tz
  val local_tz_raw = date_local_tz
  val timezone_of_raw = date_timezone_of
  val fixed_offset_raw = date_fixed_offset
  val utc_now_raw = date_utc_now
  val now_in_raw = date_now_in
  val from_instant_raw = date_from_instant
  val to_epoch_ms_raw = date_to_epoch_ms
  val to_timezone_raw = date_to_timezone
  val in_timezone_raw = date_in_timezone
  val year_raw = date_year
  val month_raw = date_month
  val day_raw = date_day
  val hour_raw = date_hour
  val minute_raw = date_minute
  val second_raw = date_second
  val millisecond_raw = date_millisecond
  val weekday_raw = date_weekday
  val to_rfc3339_raw = date_to_rfc3339
  val to_rfc2822_raw = date_to_rfc2822
  val format_raw = date_format
  val parse_rfc3339_raw = date_parse_rfc3339
  val parse_rfc9557_raw = date_parse_rfc9557
end
"#;

const HASHLINE_SOURCE: &str = r#"structure BuiltinHashline = struct
  val read_tagged_raw = read_file_tagged
  val edit_tagged_raw = edit_file_tagged
end
"#;

const MODULES: &[InternalBuiltinModule] = &[
    InternalBuiltinModule {
        leaf_name: "Filesystem",
        import_name: "__Builtin.Filesystem",
        feature_name: "builtin-filesystem",
        enabled: cfg!(feature = "builtin-filesystem"),
        source: FILESYSTEM_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Path",
        import_name: "__Builtin.Path",
        feature_name: "builtin-path",
        enabled: cfg!(feature = "builtin-path"),
        source: PATH_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Json",
        import_name: "__Builtin.Json",
        feature_name: "builtin-json",
        enabled: cfg!(feature = "builtin-json"),
        source: JSON_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Hash",
        import_name: "__Builtin.Hash",
        feature_name: "builtin-hash",
        enabled: cfg!(feature = "builtin-hash"),
        source: HASH_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Stdio",
        import_name: "__Builtin.Stdio",
        feature_name: "builtin-stdio",
        enabled: cfg!(feature = "builtin-stdio"),
        source: STDIO_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Http",
        import_name: "__Builtin.Http",
        feature_name: "builtin-http",
        enabled: cfg!(feature = "builtin-http"),
        source: HTTP_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Process",
        import_name: "__Builtin.Process",
        feature_name: "builtin-process",
        enabled: cfg!(feature = "builtin-process"),
        source: PROCESS_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Exec",
        import_name: "__Builtin.Exec",
        feature_name: "builtin-exec",
        enabled: cfg!(feature = "builtin-exec"),
        source: EXEC_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "System",
        import_name: "__Builtin.System",
        feature_name: "builtin-system",
        enabled: cfg!(feature = "builtin-system"),
        source: SYSTEM_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Time",
        import_name: "__Builtin.Time",
        feature_name: "builtin-time",
        enabled: cfg!(feature = "builtin-time"),
        source: TIME_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Numeric",
        import_name: "__Builtin.Numeric",
        feature_name: "builtin-convert",
        enabled: cfg!(feature = "builtin-convert"),
        source: NUMERIC_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Date",
        import_name: "__Builtin.Date",
        feature_name: "builtin-time",
        enabled: cfg!(feature = "builtin-time"),
        source: DATE_SOURCE,
    },
    InternalBuiltinModule {
        leaf_name: "Hashline",
        import_name: "__Builtin.Hashline",
        feature_name: "builtin-filesystem",
        enabled: cfg!(feature = "builtin-filesystem"),
        source: HASHLINE_SOURCE,
    },
];

pub fn is_internal_builtin_package(package_name: &str) -> bool {
    package_name == INTERNAL_BUILTIN_PACKAGE
}

pub fn internal_builtin_modules() -> impl Iterator<Item = &'static InternalBuiltinModule> {
    MODULES.iter()
}

pub fn internal_builtin_module(leaf_name: &str) -> Option<&'static InternalBuiltinModule> {
    MODULES.iter().find(|module| module.leaf_name == leaf_name)
}
