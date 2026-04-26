#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinTypeSignature {
    /// Type variables quantified by this signature, written without the leading quote.
    pub vars: &'static [&'static str],
    /// Hiko type expression for the builtin value.
    pub ty: &'static str,
}

pub fn builtin_type_signature(name: &str) -> Option<BuiltinTypeSignature> {
    let ty = match name {
        // I/O
        "print" | "println" => "string -> unit",
        "read_line" | "read_stdin" => "unit -> string",

        // Conversions
        "int_to_string" => "int -> string",
        "float_to_string" => "float -> string",
        "string_to_int" => "string -> int",
        "char_to_int" => "char -> int",
        "int_to_char" => "int -> char",
        "int_to_float" => "int -> float",
        "word_to_int" => "word -> int",
        "int_to_word" => "int -> word",
        "word_to_string" => "word -> string",
        "string_to_word" => "string -> word",

        // Width-specific numeric builtins use the current Option A ABI:
        // packed values are represented by the host primitive type at runtime.
        "numeric_int32_min_value" | "numeric_int32_max_value" => "unit -> int",
        "numeric_int32_of_int" | "numeric_int32_to_int" | "numeric_int32_neg" => "int -> int",
        "numeric_int32_checked_of_int" => "int -> bool * int",
        "numeric_int32_add"
        | "numeric_int32_wrapping_add"
        | "numeric_int32_saturating_add"
        | "numeric_int32_sub"
        | "numeric_int32_mul"
        | "numeric_int32_div"
        | "numeric_int32_rem" => "int * int -> int",
        "numeric_int32_checked_add" => "int * int -> bool * int",
        "numeric_word32_min_value" | "numeric_word32_max_value" => "unit -> word",
        "numeric_word32_of_word" | "numeric_word32_to_word" => "word -> word",
        "numeric_word32_checked_of_word" => "word -> bool * word",
        "numeric_word32_of_int" => "int -> word",
        "numeric_word32_checked_of_int" => "int -> bool * word",
        "numeric_word32_to_int" => "word -> int",
        "numeric_word32_add"
        | "numeric_word32_saturating_add"
        | "numeric_word32_sub"
        | "numeric_word32_mul"
        | "numeric_word32_div"
        | "numeric_word32_rem" => "word * word -> word",
        "numeric_word32_checked_add" => "word * word -> bool * word",
        "numeric_float32_of_float" | "numeric_float32_to_float" | "numeric_float32_neg" => {
            "float -> float"
        }
        "numeric_float32_add"
        | "numeric_float32_sub"
        | "numeric_float32_mul"
        | "numeric_float32_div" => "float * float -> float",

        // String
        "string_length" => "string -> int",
        "substring" => "string * int * int -> string",
        "string_contains" | "starts_with" | "ends_with" => "string * string -> bool",
        "trim" | "to_upper" | "to_lower" => "string -> string",
        "split" => "string * string -> string list",
        "string_replace" => "string * string * string -> string",
        "string_join" => "string list * string -> string",

        // Math
        "sqrt" | "abs_float" => "float -> float",
        "abs_int" => "int -> int",
        "floor" | "ceil" => "float -> int",

        // Filesystem and path
        "read_file" => "string -> string",
        "read_file_bytes" => "string -> bytes",
        "write_file" => "string * string -> unit",
        "file_exists" | "is_dir" | "is_file" => "string -> bool",
        "list_dir" | "glob" | "walk_dir" => "string -> string list",
        "remove_file" | "create_dir" => "string -> unit",
        "read_file_tagged" => "string * int * int -> string",
        "edit_file_tagged" => "string * string -> string",
        "path_join" => "string * string -> string",

        // HTTP
        "http_get" => "string -> int * (string * string) list * string",
        "http" => {
            "string * string * (string * string) list * string -> int * (string * string) list * string"
        }
        "http_bytes" => {
            "string * string * (string * string) list * string -> int * (string * string) list * bytes"
        }
        "http_json" | "http_msgpack" => {
            return Some(BuiltinTypeSignature {
                vars: &["a"],
                ty: "string * string * (string * string) list * string -> int * (string * string) list * 'a",
            });
        }

        // Bytes, hash, random
        "bytes_length" => "bytes -> int",
        "bytes_to_string" => "bytes -> string",
        "string_to_bytes" => "string -> bytes",
        "bytes_get" => "bytes * int -> int",
        "bytes_slice" => "bytes * int * int -> bytes",
        "blake3" => "bytes -> string",
        "random_bytes" => "int -> bytes",
        "rng_seed" => "bytes -> rng",
        "rng_bytes" => "rng * int -> bytes * rng",
        "rng_int" => "rng * int -> int * rng",

        // Regex and JSON
        "regex_match" => "string * string -> bool",
        "regex_replace" => "string * string * string -> string",
        "json_get" => "string * string -> string",
        "json_keys" => "string -> string list",
        "json_length" => "string -> int",
        "json_parse" => {
            return Some(BuiltinTypeSignature {
                vars: &["a"],
                ty: "string -> 'a",
            });
        }
        "json_to_string" => {
            return Some(BuiltinTypeSignature {
                vars: &["a"],
                ty: "'a -> string",
            });
        }

        // Environment, time, date
        "getenv" => "string -> string",
        "epoch" | "epoch_ms" | "monotonic_ms" => "unit -> int",
        "sleep" => "int -> unit",
        "date_utc_tz" | "date_utc_now" => "unit -> string",
        "date_local_tz" => "unit -> bool * string",
        "date_timezone_of" => "string -> bool * string",
        "date_fixed_offset" => "int -> string",
        "date_now_in" => "string -> string",
        "date_from_instant" => "int * string -> string",
        "date_to_epoch_ms" => "string -> int",
        "date_to_timezone" | "date_in_timezone" => "string * string -> string",
        "date_year" | "date_month" | "date_day" | "date_hour" | "date_minute" | "date_second"
        | "date_millisecond" | "date_weekday" => "string -> int",
        "date_to_rfc3339" | "date_to_rfc2822" => "string -> string",
        "date_format" => "string * string -> string",
        "date_parse_rfc3339" | "date_parse_rfc9557" => "string -> bool * string",

        // Process
        "spawn" => {
            return Some(BuiltinTypeSignature {
                vars: &["a"],
                ty: "(unit -> 'a) -> pid",
            });
        }
        "await_process" | "await_process_result" => {
            return Some(BuiltinTypeSignature {
                vars: &["a"],
                ty: "pid -> 'a",
            });
        }
        "cancel" => "pid -> unit",
        "wait_any" => "pid list -> pid",

        // Exec, system, testing
        "exec" => "string * string list -> int * string * string",
        "exit" => "int -> unit",
        "assert" => "bool * string -> unit",
        "panic" => {
            return Some(BuiltinTypeSignature {
                vars: &["a"],
                ty: "string -> 'a",
            });
        }
        "assert_eq" => {
            return Some(BuiltinTypeSignature {
                vars: &["a"],
                ty: "'a * 'a * string -> unit",
            });
        }
        _ => return None,
    };
    Some(BuiltinTypeSignature { vars: &[], ty })
}

pub fn builtin_type_signatures() -> impl Iterator<Item = (&'static str, BuiltinTypeSignature)> {
    crate::builtins()
        .filter_map(|meta| builtin_type_signature(meta.name).map(|sig| (meta.name, sig)))
}

pub const PROCESS_AWAIT_RESULT_CONSTRUCTORS: &[(&str, u16, usize)] = &[("Ok", 0, 1), ("Err", 1, 1)];
pub const PROCESS_JOIN_ERROR_CONSTRUCTORS: &[(&str, u16, usize)] = &[
    ("RuntimeError", 0, 1),
    ("Cancelled", 1, 0),
    ("FuelExhausted", 2, 0),
    ("AlreadyJoined", 3, 0),
    ("HeapObjectLimitExceeded", 4, 1),
];

pub const PROCESS_AWAIT_RESULT_OK_TAG: u16 = PROCESS_AWAIT_RESULT_CONSTRUCTORS[0].1;
pub const PROCESS_AWAIT_RESULT_ERR_TAG: u16 = PROCESS_AWAIT_RESULT_CONSTRUCTORS[1].1;
pub const PROCESS_JOIN_ERROR_RUNTIME_ERROR_TAG: u16 = PROCESS_JOIN_ERROR_CONSTRUCTORS[0].1;
pub const PROCESS_JOIN_ERROR_CANCELLED_TAG: u16 = PROCESS_JOIN_ERROR_CONSTRUCTORS[1].1;
pub const PROCESS_JOIN_ERROR_FUEL_EXHAUSTED_TAG: u16 = PROCESS_JOIN_ERROR_CONSTRUCTORS[2].1;
pub const PROCESS_JOIN_ERROR_ALREADY_JOINED_TAG: u16 = PROCESS_JOIN_ERROR_CONSTRUCTORS[3].1;
pub const PROCESS_JOIN_ERROR_HEAP_LIMIT_TAG: u16 = PROCESS_JOIN_ERROR_CONSTRUCTORS[4].1;

#[cfg(test)]
mod tests {
    #[test]
    fn every_enabled_builtin_has_a_type_signature() {
        let missing: Vec<_> = crate::builtins()
            .filter(|meta| super::builtin_type_signature(meta.name).is_none())
            .map(|meta| meta.name)
            .collect();
        assert!(
            missing.is_empty(),
            "missing builtin signatures: {missing:?}"
        );
    }

    #[test]
    fn process_constructor_tags_match_declared_abi() {
        assert_eq!(
            super::PROCESS_AWAIT_RESULT_CONSTRUCTORS,
            &[("Ok", 0, 1), ("Err", 1, 1)]
        );
        assert_eq!(
            super::PROCESS_JOIN_ERROR_CONSTRUCTORS,
            &[
                ("RuntimeError", 0, 1),
                ("Cancelled", 1, 0),
                ("FuelExhausted", 2, 0),
                ("AlreadyJoined", 3, 0),
                ("HeapObjectLimitExceeded", 4, 1),
            ]
        );
    }
}
