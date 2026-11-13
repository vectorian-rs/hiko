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
  fun read_text value = read_file value
  fun read_bytes value = read_file_bytes value
  fun write_text value = write_file value
  fun exists value = file_exists value
  fun list_entries value = list_dir value
  fun remove value = remove_file value
  fun create_directory value = create_dir value
  fun is_directory value = is_dir value
  fun is_regular_file value = is_file value
  fun glob value = glob value
  fun walk value = walk_dir value
end
"#;

const PATH_SOURCE: &str = r#"structure BuiltinPath = struct
  fun join value = path_join value
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

  fun parse value = json_parse value
  fun to_string value = json_to_string value
  fun get value = json_get value
  fun keys value = json_keys value
  fun length value = json_length value
end
"#;

const STRING_SOURCE: &str = r#"structure BuiltinString = struct
  fun length value = string_length value
  fun substring value = substring value
  fun contains value = string_contains value
  fun starts_with value = starts_with value
  fun ends_with value = ends_with value
  fun trim value = trim value
  fun split value = split value
  fun replace value = string_replace value
  fun join value = string_join value
  fun upper value = to_upper value
  fun lower value = to_lower value
end
"#;

const BYTES_SOURCE: &str = r#"structure BuiltinBytes = struct
  fun length value = bytes_length value
  fun to_string value = bytes_to_string value
  fun of_string value = string_to_bytes value
  fun get value = bytes_get value
  fun slice value = bytes_slice value
end
"#;

const RANDOM_SOURCE: &str = r#"structure BuiltinRandom = struct
  fun random_bytes value = random_bytes value
  fun seed value = rng_seed value
  fun bytes value = rng_bytes value
  fun int value = rng_int value
end
"#;

const REGEX_SOURCE: &str = r#"structure BuiltinRegex = struct
  fun match value = regex_match value
  fun replace value = regex_replace value
end
"#;

const ENV_SOURCE: &str = r#"structure BuiltinEnv = struct
  fun get value = getenv value
end
"#;

const TESTING_SOURCE: &str = r#"structure BuiltinTesting = struct
  fun assert value = assert value
  fun assert_eq value = assert_eq value
  fun panic value = panic value
end
"#;

const HASH_SOURCE: &str = r#"structure BuiltinHash = struct
  fun blake3 value = blake3 value
end
"#;

const STDIO_SOURCE: &str = r#"structure BuiltinStdio = struct
  fun print value = print value
  fun println value = println value
  fun eprint value = eprint value
  fun eprintln value = eprintln value
  fun read_line value = read_line value
  fun read_stdin value = read_stdin value
end
"#;

const HTTP_SOURCE: &str = r#"structure BuiltinHttp = struct
  fun get value = http_get value
  fun request value = http value
  fun request_json value = http_json value
  fun request_msgpack value = http_msgpack value
  fun request_bytes value = http_bytes value
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

  fun spawn f = spawn f
  fun await pid = await_process pid
  fun await_result pid = await_process_result pid
  fun await_all pids = fiber_await_all pids
  fun cancel pid = cancel pid
  fun wait_any pids = wait_any pids
end
"#;

const EXEC_SOURCE: &str = r#"structure BuiltinExec = struct
  fun exec value = exec value
end
"#;

const SYSTEM_SOURCE: &str = r#"structure BuiltinSystem = struct
  fun exit value = exit value
end
"#;

const TIME_SOURCE: &str = r#"structure BuiltinTime = struct
  fun epoch value = epoch value
  fun epoch_ms value = epoch_ms value
  fun monotonic_ms value = monotonic_ms value
  fun sleep value = sleep value
end
"#;

const NUMERIC_SOURCE: &str = r#"structure BuiltinNumeric = struct
  fun int32_min_value value = numeric_int32_min_value value
  fun int32_max_value value = numeric_int32_max_value value
  fun int32_of_int value = numeric_int32_of_int value
  fun int32_checked_of_int value = numeric_int32_checked_of_int value
  fun int32_to_int value = numeric_int32_to_int value
  fun int32_add value = numeric_int32_add value
  fun int32_checked_add value = numeric_int32_checked_add value
  fun int32_wrapping_add value = numeric_int32_wrapping_add value
  fun int32_saturating_add value = numeric_int32_saturating_add value
  fun int32_sub value = numeric_int32_sub value
  fun int32_mul value = numeric_int32_mul value
  fun int32_div value = numeric_int32_div value
  fun int32_rem value = numeric_int32_rem value
  fun int32_neg value = numeric_int32_neg value

  fun word32_min_value value = numeric_word32_min_value value
  fun word32_max_value value = numeric_word32_max_value value
  fun word32_of_word value = numeric_word32_of_word value
  fun word32_checked_of_word value = numeric_word32_checked_of_word value
  fun word32_of_int value = numeric_word32_of_int value
  fun word32_checked_of_int value = numeric_word32_checked_of_int value
  fun word32_to_word value = numeric_word32_to_word value
  fun word32_to_int value = numeric_word32_to_int value
  fun word32_add value = numeric_word32_add value
  fun word32_checked_add value = numeric_word32_checked_add value
  fun word32_saturating_add value = numeric_word32_saturating_add value
  fun word32_sub value = numeric_word32_sub value
  fun word32_mul value = numeric_word32_mul value
  fun word32_div value = numeric_word32_div value
  fun word32_rem value = numeric_word32_rem value

  fun float32_of_float value = numeric_float32_of_float value
  fun float32_to_float value = numeric_float32_to_float value
  fun float32_neg value = numeric_float32_neg value
  fun float32_add value = numeric_float32_add value
  fun float32_sub value = numeric_float32_sub value
  fun float32_mul value = numeric_float32_mul value
  fun float32_div value = numeric_float32_div value
end
"#;

const DATE_SOURCE: &str = r#"structure BuiltinDate = struct
  fun utc_tz value = date_utc_tz value
  fun local_tz value = date_local_tz value
  fun timezone_of value = date_timezone_of value
  fun fixed_offset value = date_fixed_offset value
  fun utc_now value = date_utc_now value
  fun now_in value = date_now_in value
  fun from_instant value = date_from_instant value
  fun to_epoch_ms value = date_to_epoch_ms value
  fun to_timezone value = date_to_timezone value
  fun in_timezone value = date_in_timezone value
  fun year value = date_year value
  fun month value = date_month value
  fun day value = date_day value
  fun hour value = date_hour value
  fun minute value = date_minute value
  fun second value = date_second value
  fun millisecond value = date_millisecond value
  fun weekday value = date_weekday value
  fun to_rfc3339 value = date_to_rfc3339 value
  fun to_rfc2822 value = date_to_rfc2822 value
  fun format value = date_format value
  fun parse_rfc3339 value = date_parse_rfc3339 value
  fun parse_rfc9557 value = date_parse_rfc9557 value
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
  fun ceil value = ceil value
  fun floor value = floor value
  fun of_int value = int_to_float value
  fun sqrt value = sqrt value
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
  fun exit code = exit code
end
"#;

const SYSTEM_PANIC_SOURCE: &str = r#"structure BuiltinSystemPanic = struct
  fun panic text = panic text
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
