pub const INTERNAL_BUILTIN_PACKAGE: &str = "__Builtin";

/// Nominal process types whose constructor layout is owned by the runtime.
/// Reserve them even before the embedded `__Builtin.Fiber` module is imported.
pub const PROCESS_RUNTIME_TYPES: &[(&str, usize)] =
    &[("BuiltinFiber.error", 0), ("BuiltinFiber.await_result", 1)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InternalBuiltinModule {
    pub leaf_name: &'static str,
    pub import_name: &'static str,
    pub feature_name: &'static str,
    pub enabled: bool,
    pub source: &'static str,
}

impl InternalBuiltinModule {
    pub fn owned_types(&self) -> &'static [(&'static str, usize)] {
        match self.leaf_name {
            "Fiber" => PROCESS_RUNTIME_TYPES,
            "Json" => &[("BuiltinJson.json", 0)],
            _ => &[],
        }
    }
}

pub fn runtime_type_arities() -> impl Iterator<Item = (&'static str, usize)> {
    MODULES
        .iter()
        .flat_map(|module| module.owned_types().iter().copied())
}

pub fn runtime_type_owner(name: &str) -> Option<&'static InternalBuiltinModule> {
    MODULES
        .iter()
        .find(|module| module.owned_types().iter().any(|&(owned, _)| owned == name))
}

const FILESYSTEM_SOURCE: &str = r#"structure BuiltinFilesystem = struct
  val read_text = read_file
  val read_bytes = read_file_bytes
  val write_text = write_file
  val exists = file_exists
  val list_entries = list_dir
  val remove = remove_file
  val create_directory = create_dir
  val is_directory = is_dir
  val is_regular_file = is_file
  val glob = glob
  val walk = walk_dir
end
"#;

const PATH_SOURCE: &str = r#"structure BuiltinPath = struct
  val join = path_join
end
"#;

const JSON_SOURCE: &str = r#"structure BuiltinJson = struct
  datatype json =
      JNull
    | JBool of bool
    | JInt of int
    | JFloat of float
    | JStr of string
    | JArray of json list
    | JObject of (string * json) list

  val parse = json_parse
  val to_string = json_to_string
  val get = json_get
  val keys = json_keys
  val length = json_length
end
"#;

const STRING_SOURCE: &str = r#"structure BuiltinString = struct
  val length = string_length
  val substring = substring
  val contains = string_contains
  val starts_with = starts_with
  val ends_with = ends_with
  val trim = trim
  val split = split
  val replace = string_replace
  val join = string_join
  val upper = to_upper
  val lower = to_lower
end
"#;

const BYTES_SOURCE: &str = r#"structure BuiltinBytes = struct
  val length = bytes_length
  val to_string = bytes_to_string
  val of_string = string_to_bytes
  val get = bytes_get
  val slice = bytes_slice
end
"#;

const RANDOM_SOURCE: &str = r#"structure BuiltinRandom = struct
  val random_bytes = random_bytes
  val seed = rng_seed
  val bytes = rng_bytes
  val int = rng_int
end
"#;

const REGEX_SOURCE: &str = r#"structure BuiltinRegex = struct
  val match = regex_match
  val replace = regex_replace
end
"#;

const ENV_SOURCE: &str = r#"structure BuiltinEnv = struct
  val get = getenv
end
"#;

const TESTING_SOURCE: &str = r#"structure BuiltinTesting = struct
  val assert = assert
  val assert_eq = assert_eq
  val panic = panic
end
"#;

const HASH_SOURCE: &str = r#"structure BuiltinHash = struct
  val blake3 = blake3
end
"#;

const STDIO_SOURCE: &str = r#"structure BuiltinStdio = struct
  val print = print
  val println = println
  val eprint = eprint
  val eprintln = eprintln
  val read_line = read_line
  val read_stdin = read_stdin
end
"#;

const HTTP_SOURCE: &str = r#"structure BuiltinHttp = struct
  val get = http_get
  val request = http
  val request_json = http_json
  val request_msgpack = http_msgpack
  val request_bytes = http_bytes
end
"#;

const PROCESS_SOURCE: &str = r#"structure BuiltinFiber = struct
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

  fun spawn_raw f = spawn f
  fun await pid = await_process pid
  fun await_result pid = await_process_result pid
  fun await_all pids = fiber_await_all pids
  fun cancel_raw pid = cancel pid
  fun wait_any_raw pids = wait_any pids
end
"#;

const EXEC_SOURCE: &str = r#"structure BuiltinExec = struct
  val exec = exec
end
"#;

const SYSTEM_SOURCE: &str = r#"structure BuiltinSystem = struct
  val exit = exit
end
"#;

const TIME_SOURCE: &str = r#"structure BuiltinTime = struct
  val epoch = epoch
  val epoch_ms = epoch_ms
  val monotonic_ms = monotonic_ms
  val sleep = sleep
end
"#;

const NUMERIC_SOURCE: &str = r#"structure BuiltinNumeric = struct
  val int32_min_value = numeric_int32_min_value
  val int32_max_value = numeric_int32_max_value
  val int32_of_int = numeric_int32_of_int
  val int32_checked_of_int = numeric_int32_checked_of_int
  val int32_to_int = numeric_int32_to_int
  val int32_add = numeric_int32_add
  val int32_checked_add = numeric_int32_checked_add
  val int32_wrapping_add = numeric_int32_wrapping_add
  val int32_saturating_add = numeric_int32_saturating_add
  val int32_sub = numeric_int32_sub
  val int32_mul = numeric_int32_mul
  val int32_div = numeric_int32_div
  val int32_rem = numeric_int32_rem
  val int32_neg = numeric_int32_neg

  val word32_min_value = numeric_word32_min_value
  val word32_max_value = numeric_word32_max_value
  val word32_of_word = numeric_word32_of_word
  val word32_checked_of_word = numeric_word32_checked_of_word
  val word32_of_int = numeric_word32_of_int
  val word32_checked_of_int = numeric_word32_checked_of_int
  val word32_to_word = numeric_word32_to_word
  val word32_to_int = numeric_word32_to_int
  val word32_add = numeric_word32_add
  val word32_checked_add = numeric_word32_checked_add
  val word32_saturating_add = numeric_word32_saturating_add
  val word32_sub = numeric_word32_sub
  val word32_mul = numeric_word32_mul
  val word32_div = numeric_word32_div
  val word32_rem = numeric_word32_rem

  val float32_of_float = numeric_float32_of_float
  val float32_to_float = numeric_float32_to_float
  val float32_neg = numeric_float32_neg
  val float32_add = numeric_float32_add
  val float32_sub = numeric_float32_sub
  val float32_mul = numeric_float32_mul
  val float32_div = numeric_float32_div
end
"#;

const DATE_SOURCE: &str = r#"structure BuiltinDate = struct
  val utc_tz = date_utc_tz
  val local_tz = date_local_tz
  val timezone_of = date_timezone_of
  val fixed_offset = date_fixed_offset
  val utc_now = date_utc_now
  val now_in = date_now_in
  val from_instant = date_from_instant
  val to_epoch_ms = date_to_epoch_ms
  val to_timezone = date_to_timezone
  val in_timezone = date_in_timezone
  val year = date_year
  val month = date_month
  val day = date_day
  val hour = date_hour
  val minute = date_minute
  val second = date_second
  val millisecond = date_millisecond
  val weekday = date_weekday
  val to_rfc3339 = date_to_rfc3339
  val to_rfc2822 = date_to_rfc2822
  val format = date_format
  val parse_rfc3339 = date_parse_rfc3339
  val parse_rfc9557 = date_parse_rfc9557
end
"#;

// New forwarders are lazy: importing a noun must not resolve unused policy
// leaves. Keep the existing eager module sources above unchanged.
const INT_SOURCE: &str = r#"structure BuiltinInt = struct
  fun abs value = abs_int value
  fun of_string text = string_to_int text
  fun to_string value = int_to_string value
end
"#;

const FLOAT_SOURCE: &str = r#"structure BuiltinFloat = struct
  fun abs value = abs_float value
  fun ceil_raw value = ceil value
  fun floor_raw value = floor value
  fun of_int value = int_to_float value
  fun sqrt_raw value = sqrt value
  fun to_string value = float_to_string value
end
"#;

const CHAR_SOURCE: &str = r#"structure BuiltinChar = struct
  fun of_int value = int_to_char value
  fun to_int value = char_to_int value
end
"#;

const WORD_SOURCE: &str = r#"structure BuiltinWord = struct
  fun of_int value = int_to_word value
  fun to_int value = word_to_int value
  fun of_string text = string_to_word text
  fun to_string value = word_to_string value
end
"#;

const PATH_JOIN_SOURCE: &str = r#"structure BuiltinPathJoin = struct
  fun join parts = path_join parts
end
"#;

const SYSTEM_EXIT_SOURCE: &str = r#"structure BuiltinSystemExit = struct
  fun exit_raw code = exit code
end
"#;

const SYSTEM_PANIC_SOURCE: &str = r#"structure BuiltinSystemPanic = struct
  fun panic_raw text = panic text
end
"#;

/// Declares an internal module whose import path and enabled flag are derived
/// from its leaf name and Cargo feature, so they cannot drift apart.
macro_rules! module {
    ($leaf:literal, $feature:literal, $source:ident) => {
        InternalBuiltinModule {
            leaf_name: $leaf,
            import_name: concat!("__Builtin.", $leaf),
            feature_name: $feature,
            enabled: cfg!(feature = $feature),
            source: $source,
        }
    };
}

const MODULES: &[InternalBuiltinModule] = &[
    module!("Filesystem", "builtin-filesystem", FILESYSTEM_SOURCE),
    module!("Path", "builtin-path", PATH_SOURCE),
    module!("Json", "builtin-json", JSON_SOURCE),
    module!("String", "builtin-string", STRING_SOURCE),
    module!("Bytes", "builtin-bytes", BYTES_SOURCE),
    module!("Random", "builtin-random", RANDOM_SOURCE),
    module!("Regex", "builtin-regex", REGEX_SOURCE),
    module!("Env", "builtin-env", ENV_SOURCE),
    module!("Testing", "builtin-testing", TESTING_SOURCE),
    module!("Hash", "builtin-hash", HASH_SOURCE),
    module!("Stdio", "builtin-stdio", STDIO_SOURCE),
    module!("Http", "builtin-http", HTTP_SOURCE),
    module!("Fiber", "builtin-process", PROCESS_SOURCE),
    module!("Exec", "builtin-exec", EXEC_SOURCE),
    module!("System", "builtin-system", SYSTEM_SOURCE),
    module!("Time", "builtin-time", TIME_SOURCE),
    module!("Numeric", "builtin-convert", NUMERIC_SOURCE),
    module!("Date", "builtin-date", DATE_SOURCE),
    module!("Int", "builtin-convert", INT_SOURCE),
    module!("Float", "builtin-convert", FLOAT_SOURCE),
    module!("Char", "builtin-convert", CHAR_SOURCE),
    module!("Word", "builtin-convert", WORD_SOURCE),
    module!("PathJoin", "builtin-path", PATH_JOIN_SOURCE),
    module!("SystemExit", "builtin-system", SYSTEM_EXIT_SOURCE),
    module!("SystemPanic", "builtin-testing", SYSTEM_PANIC_SOURCE),
];

pub fn is_internal_builtin_package(package_name: &str) -> bool {
    package_name == INTERNAL_BUILTIN_PACKAGE
}

pub fn internal_builtin_module(leaf_name: &str) -> Option<&'static InternalBuiltinModule> {
    MODULES.iter().find(|module| module.leaf_name == leaf_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_names_live_in_the_internal_package() {
        for module in MODULES {
            assert_eq!(
                module.import_name,
                format!("{INTERNAL_BUILTIN_PACKAGE}.{}", module.leaf_name)
            );
        }
    }

    /// Both eager aliases and lazy forwarders in enabled modules must name a
    /// builtin declared under the same feature set.
    #[test]
    fn enabled_module_sources_alias_declared_builtins() {
        for module in MODULES.iter().filter(|module| module.enabled) {
            for line in module.source.lines() {
                let Some((_, rhs)) = line
                    .trim()
                    .strip_prefix("val ")
                    .or_else(|| line.trim().strip_prefix("fun "))
                    .and_then(|rest| rest.split_once(" = "))
                else {
                    continue;
                };
                let rhs = rhs.split_whitespace().next().unwrap();
                assert!(
                    crate::builtin_meta(rhs).is_some(),
                    "{} aliases undeclared builtin {rhs}",
                    module.import_name
                );
            }
        }
    }
}
