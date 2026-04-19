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
  val spawn_raw = spawn
  val await_raw = await_process
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
