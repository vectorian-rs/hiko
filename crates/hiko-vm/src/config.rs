#[cfg(feature = "builtin-exec")]
use crate::builder::ExecPolicy as VmExecPolicy;
use crate::builder::VMBuilder;
use crate::extension::Extension;
use crate::vm::VM;
use hiko_builtin_meta::{
    capability_path_for_builtin as meta_capability_path_for_builtin,
    unrestricted_runtime_builtin_names,
};
use hiko_compile::chunk::CompiledProgram;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path};

/// Runtime/loadable run configuration.
/// This struct describes how a VM should be configured for a specific
/// invocation. It can also be embedded into generated artifacts by build tools.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    pub entry: Option<String>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub capabilities: Capabilities,
    /// `[extensions.<name>]` tables. Each turns on the extension of that
    /// name; the table is passed to its `configure`.
    #[serde(default)]
    pub extensions: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_work: Option<u64>,
    pub max_fuel: Option<u64>,
    pub max_heap: Option<usize>,
    pub max_memory_bytes: Option<usize>,
    pub max_io_bytes: Option<u64>,
    pub max_host_work: Option<u64>,
    pub max_host_resources: Option<usize>,
    /// Spawned processes alive at once in the process tree (root not
    /// counted). Defaults to `crate::budget::DEFAULT_MAX_LIVE_PROCESSES`.
    pub max_live_processes: Option<u64>,
    /// Processes the process tree may spawn in total. Unlimited when unset.
    pub max_spawns: Option<u64>,
    /// Largest value one spawn capture or child result may serialize to.
    /// Defaults to `crate::sendable::DEFAULT_MAX_MESSAGE_BYTES`.
    pub max_message_bytes: Option<u64>,
    /// Independent tree-shared logical bulk-join storage. Defaults to 64 MiB.
    pub max_join_group_bytes: Option<u64>,
    #[serde(default)]
    pub host_resources: HashMap<String, usize>,
    /// Per-operation I/O limits. Unset keys use the defaults in
    /// `crate::io_limits`.
    pub http_timeout_ms: Option<u64>,
    pub http_connect_timeout_ms: Option<u64>,
    pub exec_timeout_ms: Option<u64>,
    pub max_http_response_bytes: Option<u64>,
    pub max_file_read_bytes: Option<u64>,
    pub max_exec_output_bytes: Option<u64>,
    pub max_concurrent_io: Option<u64>,
}

type BuilderLimitSetter = fn(VMBuilder, u64) -> VMBuilder;

impl Limits {
    /// Per-operation I/O limits that are set, as (builder method name, value,
    /// setter). The name is also the policy key.
    fn io_limit_settings(&self) -> Vec<(&'static str, u64, BuilderLimitSetter)> {
        let all: [(&'static str, Option<u64>, BuilderLimitSetter); 7] = [
            (
                "http_timeout_ms",
                self.http_timeout_ms,
                VMBuilder::http_timeout_ms,
            ),
            (
                "http_connect_timeout_ms",
                self.http_connect_timeout_ms,
                VMBuilder::http_connect_timeout_ms,
            ),
            (
                "exec_timeout_ms",
                self.exec_timeout_ms,
                VMBuilder::exec_timeout_ms,
            ),
            (
                "max_http_response_bytes",
                self.max_http_response_bytes,
                VMBuilder::max_http_response_bytes,
            ),
            (
                "max_file_read_bytes",
                self.max_file_read_bytes,
                VMBuilder::max_file_read_bytes,
            ),
            (
                "max_exec_output_bytes",
                self.max_exec_output_bytes,
                VMBuilder::max_exec_output_bytes,
            ),
            (
                "max_concurrent_io",
                self.max_concurrent_io,
                VMBuilder::max_concurrent_io,
            ),
        ];
        all.into_iter()
            .filter_map(|(name, value, setter)| value.map(|value| (name, value, setter)))
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct EnabledLeaf {
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FolderLeaf {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    folders: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct HttpLeaf {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    allowed_hosts: Vec<String>,
}

/// The `exec` timeout is `[limits] exec_timeout_ms`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ExecLeaf {
    #[serde(default)]
    enabled: bool,
    /// Commands allowed with any arguments.
    #[serde(default)]
    allowed_commands: Vec<String>,
    /// `[[capabilities.exec.run.allow]]`: commands allowed only with these
    /// arguments.
    #[serde(default)]
    allow: Vec<ExecAllowLeaf>,
}

/// One `allow` rule: the exact argv, or with `rest = "any"` a fixed prefix.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecAllowLeaf {
    argv: Vec<String>,
    #[serde(default)]
    rest: ExecRest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ExecRest {
    #[default]
    None,
    Any,
}

impl ExecAllowLeaf {
    #[cfg(feature = "builtin-exec")]
    fn rule(&self) -> crate::builder::ExecArgvRule {
        crate::builder::ExecArgvRule {
            argv: self.argv.clone(),
            rest_any: self.rest == ExecRest::Any,
        }
    }
}

macro_rules! enabled_family {
    ($name:ident { $($field:ident => $builtin:literal),* $(,)? }) => {
        #[derive(Debug, Clone, Deserialize, Default)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $($field: Option<EnabledLeaf>,)*
        }

        impl $name {
            fn apply(&self, mut builder: VMBuilder) -> VMBuilder {
                $(
                    if self.$field.as_ref().is_some_and(|leaf| leaf.enabled) {
                        builder = builder.register_builtin_name($builtin);
                    }
                )*
                builder
            }

            fn extend_enabled(&self, out: &mut BTreeSet<&'static str>) {
                $(
                    if self.$field.as_ref().is_some_and(|leaf| leaf.enabled) {
                        out.insert($builtin);
                    }
                )*
            }

            fn emit(&self, out: &mut String) {
                $(
                    if self.$field.as_ref().is_some_and(|leaf| leaf.enabled) {
                        out.push_str(&format!(
                            "            .register_builtin_name({:?})\n",
                            $builtin
                        ));
                    }
                )*
            }
        }
    };
}

macro_rules! folder_family {
    ($name:ident { $($field:ident => $builtin:literal),* $(,)? }) => {
        #[derive(Debug, Clone, Deserialize, Default)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $($field: Option<FolderLeaf>,)*
        }

        impl $name {
            fn apply(&self, builder: VMBuilder) -> VMBuilder {
                #[cfg(feature = "builtin-filesystem")]
                {
                    let mut builder = builder;
                    $(
                        if let Some(leaf) = &self.$field
                            && leaf.enabled
                        {
                            builder = builder.allow_filesystem_builtin($builtin, leaf.folders.clone());
                        }
                    )*
                    builder
                }
                #[cfg(not(feature = "builtin-filesystem"))]
                {
                    builder
                }
            }

            fn extend_enabled(&self, out: &mut BTreeSet<&'static str>) {
                $(
                    if let Some(leaf) = &self.$field
                        && leaf.enabled
                    {
                        out.insert($builtin);
                    }
                )*
            }

            fn emit(&self, out: &mut String) {
                $(
                    if let Some(leaf) = &self.$field
                        && leaf.enabled
                    {
                        out.push_str(&format!(
                            "            .allow_filesystem_builtin({:?}, vec![{}])\n",
                            $builtin,
                            rust_string_vec(&leaf.folders)
                        ));
                    }
                )*
            }
        }
    };
}

macro_rules! http_family {
    ($name:ident { $($field:ident => $builtin:literal),* $(,)? }) => {
        #[derive(Debug, Clone, Deserialize, Default)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $($field: Option<HttpLeaf>,)*
        }

        impl $name {
            fn apply(&self, builder: VMBuilder) -> VMBuilder {
                #[cfg(feature = "builtin-http")]
                {
                    let mut builder = builder;
                    $(
                        if let Some(leaf) = &self.$field
                            && leaf.enabled
                        {
                            builder = builder.allow_http_builtin($builtin, leaf.allowed_hosts.clone());
                        }
                    )*
                    builder
                }
                #[cfg(not(feature = "builtin-http"))]
                {
                    builder
                }
            }

            fn extend_enabled(&self, out: &mut BTreeSet<&'static str>) {
                $(
                    if let Some(leaf) = &self.$field
                        && leaf.enabled
                    {
                        out.insert($builtin);
                    }
                )*
            }

            fn emit(&self, out: &mut String) {
                $(
                    if let Some(leaf) = &self.$field
                        && leaf.enabled
                    {
                        out.push_str(&format!(
                            "            .allow_http_builtin({:?}, vec![{}])\n",
                            $builtin,
                            rust_string_vec(&leaf.allowed_hosts)
                        ));
                    }
                )*
            }
        }
    };
}

enabled_family!(FiberCapabilities {
    spawn => "spawn",
    join => "await_process_result",
    join_or_fail => "await_process",
    join_all => "fiber_await_all",
    cancel => "cancel",
    wait_any => "wait_any",
});

impl FiberCapabilities {
    fn requires_runtime(&self) -> bool {
        [
            &self.spawn,
            &self.join,
            &self.join_or_fail,
            &self.join_all,
            &self.cancel,
            &self.wait_any,
        ]
        .into_iter()
        .any(|leaf| leaf.as_ref().is_some_and(|leaf| leaf.enabled))
    }
}

enabled_family!(StdioCapabilities {
    print => "print",
    println => "println",
    eprint => "eprint",
    eprintln => "eprintln",
    read_line => "read_line",
    read_stdin => "read_stdin",
});

enabled_family!(IntCapabilities {
    abs => "abs_int",
    of_string => "string_to_int",
    to_string => "int_to_string",
});

enabled_family!(FloatCapabilities {
    abs => "abs_float",
    ceil => "ceil",
    floor => "floor",
    of_int => "int_to_float",
    sqrt => "sqrt",
    to_string => "float_to_string",
});

enabled_family!(CharCapabilities {
    of_int => "int_to_char",
    to_int => "char_to_int",
});

enabled_family!(WordCapabilities {
    of_int => "int_to_word",
    of_string => "string_to_word",
    to_int => "word_to_int",
    to_string => "word_to_string",
});

enabled_family!(StringCapabilities {
    length => "string_length",
    substring => "substring",
    contains => "string_contains",
    trim => "trim",
    split => "split",
    replace => "string_replace",
    starts_with => "starts_with",
    ends_with => "ends_with",
    upper => "to_upper",
    lower => "to_lower",
    join => "string_join",
});

enabled_family!(RegexCapabilities {
    matches => "regex_match",
    replace => "regex_replace",
});

enabled_family!(JsonCapabilities {
    parse => "json_parse",
    to_string => "json_to_string",
    get => "json_get",
    keys => "json_keys",
    length => "json_length",
});

enabled_family!(BytesCapabilities {
    length => "bytes_length",
    to_string => "bytes_to_string",
    of_string => "string_to_bytes",
    get => "bytes_get",
    slice => "bytes_slice",
});

enabled_family!(HashBlake3Capabilities {
    hash => "blake3",
});

/// Hash keys nest one level: `capabilities.hash.blake3.hash.<leaf>`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HashCapabilities {
    #[serde(default)]
    blake3: HashBlake3Capabilities,
}

impl HashCapabilities {
    fn apply(&self, builder: VMBuilder) -> VMBuilder {
        self.blake3.apply(builder)
    }

    fn extend_enabled(&self, out: &mut BTreeSet<&'static str>) {
        self.blake3.extend_enabled(out);
    }

    fn emit(&self, out: &mut String) {
        self.blake3.emit(out);
    }
}

enabled_family!(RandomCapabilities {
    entropy => "random_bytes",
    seed => "rng_seed",
    bytes => "rng_bytes",
    int_between => "rng_int",
});

enabled_family!(EnvCapabilities {
    get => "getenv",
});

enabled_family!(TimeCapabilities {
    epoch => "epoch",
    now => "epoch_ms",
    monotonic_now => "monotonic_ms",
    sleep => "sleep",
});

enabled_family!(PathCapabilities {
    join => "path_join",
});

folder_family!(FilesystemCapabilities {
    read_text => "read_file",
    read_bytes => "read_file_bytes",
    write_text => "write_file",
    exists => "file_exists",
    list_entries => "list_dir",
    remove => "remove_file",
    create_directory => "create_dir",
    is_directory => "is_dir",
    is_regular_file => "is_file",
    glob_matches => "glob",
    walk => "walk_dir",
});

http_family!(HttpClientCapabilities {
    get => "http_get",
    request => "http",
    request_json => "http_json",
    request_msgpack => "http_msgpack",
    request_bytes => "http_bytes",
});

/// HTTP keys nest one level: `capabilities.http.client.<leaf>`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HttpCapabilities {
    #[serde(default)]
    client: HttpClientCapabilities,
}

impl HttpCapabilities {
    fn apply(&self, builder: VMBuilder) -> VMBuilder {
        self.client.apply(builder)
    }

    fn extend_enabled(&self, out: &mut BTreeSet<&'static str>) {
        self.client.extend_enabled(out);
    }

    fn emit(&self, out: &mut String) {
        self.client.emit(out);
    }
}

enabled_family!(SystemCapabilities {
    exit => "exit",
    panic => "panic",
});

enabled_family!(TestingCapabilities {
    check => "assert",
    equal => "assert_eq",
});

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ExecCapabilities {
    run: Option<ExecLeaf>,
}

impl ExecCapabilities {
    fn apply(&self, builder: VMBuilder) -> VMBuilder {
        if let Some(leaf) = &self.run
            && leaf.enabled
        {
            #[cfg(feature = "builtin-exec")]
            {
                return builder.with_exec(VmExecPolicy {
                    allowed: leaf.allowed_commands.clone(),
                    rules: leaf.allow.iter().map(ExecAllowLeaf::rule).collect(),
                });
            }
        }
        builder
    }

    fn emit(&self, out: &mut String) {
        if let Some(leaf) = &self.run
            && leaf.enabled
        {
            let rules: Vec<String> = leaf
                .allow
                .iter()
                .map(|rule| {
                    format!(
                        "hiko_vm::builder::ExecArgvRule {{ argv: vec![{}], rest_any: {} }}",
                        rust_string_vec(&rule.argv),
                        rule.rest == ExecRest::Any
                    )
                })
                .collect();
            out.push_str(&format!(
                "            .with_exec(hiko_vm::builder::ExecPolicy {{\n\
                 \x20               allowed: vec![{}],\n\
                 \x20               rules: vec![{}],\n\
                 \x20           }})\n",
                rust_string_vec(&leaf.allowed_commands),
                rules.join(", "),
            ));
        }
    }

    fn extend_enabled(&self, out: &mut BTreeSet<&'static str>) {
        if let Some(leaf) = &self.run
            && leaf.enabled
        {
            out.insert("exec");
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub stdio: StdioCapabilities,
    #[serde(default)]
    pub int: IntCapabilities,
    #[serde(default)]
    pub float: FloatCapabilities,
    #[serde(default)]
    pub char: CharCapabilities,
    #[serde(default)]
    pub word: WordCapabilities,
    #[serde(default)]
    pub string: StringCapabilities,
    #[serde(default)]
    pub regex: RegexCapabilities,
    #[serde(default)]
    pub json: JsonCapabilities,
    #[serde(default)]
    pub bytes: BytesCapabilities,
    #[serde(default)]
    pub hash: HashCapabilities,
    #[serde(default)]
    pub random: RandomCapabilities,
    #[serde(default)]
    pub env: EnvCapabilities,
    #[serde(default)]
    pub time: TimeCapabilities,
    #[serde(default)]
    pub fiber: FiberCapabilities,
    #[serde(default)]
    pub path: PathCapabilities,
    #[serde(default)]
    pub filesystem: FilesystemCapabilities,
    #[serde(default)]
    pub http: HttpCapabilities,
    #[serde(default)]
    pub exec: ExecCapabilities,
    #[serde(default)]
    pub system: SystemCapabilities,
    #[serde(default)]
    pub testing: TestingCapabilities,
}

impl Capabilities {
    fn apply(&self, builder: VMBuilder) -> VMBuilder {
        let mut builder = builder;
        for name in unrestricted_runtime_builtin_names() {
            builder = builder.register_builtin_name(name);
        }
        let builder = self.stdio.apply(builder);
        let builder = self.int.apply(builder);
        let builder = self.float.apply(builder);
        let builder = self.char.apply(builder);
        let builder = self.word.apply(builder);
        let builder = self.string.apply(builder);
        let builder = self.regex.apply(builder);
        let builder = self.json.apply(builder);
        let builder = self.bytes.apply(builder);
        let builder = self.hash.apply(builder);
        let builder = self.random.apply(builder);
        let builder = self.env.apply(builder);
        let builder = self.time.apply(builder);
        let builder = self.fiber.apply(builder);
        let builder = self.path.apply(builder);
        let builder = self.filesystem.apply(builder);
        let builder = self.http.apply(builder);
        let builder = self.exec.apply(builder);
        let builder = self.system.apply(builder);
        self.testing.apply(builder)
    }

    fn emit(&self, out: &mut String) {
        for name in unrestricted_runtime_builtin_names() {
            out.push_str(&format!("            .register_builtin_name({name:?})\n"));
        }
        self.stdio.emit(out);
        self.int.emit(out);
        self.float.emit(out);
        self.char.emit(out);
        self.word.emit(out);
        self.string.emit(out);
        self.regex.emit(out);
        self.json.emit(out);
        self.bytes.emit(out);
        self.hash.emit(out);
        self.random.emit(out);
        self.env.emit(out);
        self.time.emit(out);
        self.fiber.emit(out);
        self.path.emit(out);
        self.filesystem.emit(out);
        self.http.emit(out);
        self.exec.emit(out);
        self.system.emit(out);
        self.testing.emit(out);
    }

    fn enabled_builtin_names(&self) -> BTreeSet<&'static str> {
        let mut out: BTreeSet<_> = unrestricted_runtime_builtin_names().collect();
        self.stdio.extend_enabled(&mut out);
        self.int.extend_enabled(&mut out);
        self.float.extend_enabled(&mut out);
        self.char.extend_enabled(&mut out);
        self.word.extend_enabled(&mut out);
        self.string.extend_enabled(&mut out);
        self.regex.extend_enabled(&mut out);
        self.json.extend_enabled(&mut out);
        self.bytes.extend_enabled(&mut out);
        self.hash.extend_enabled(&mut out);
        self.random.extend_enabled(&mut out);
        self.env.extend_enabled(&mut out);
        self.time.extend_enabled(&mut out);
        self.fiber.extend_enabled(&mut out);
        self.path.extend_enabled(&mut out);
        self.filesystem.extend_enabled(&mut out);
        self.http.extend_enabled(&mut out);
        self.exec.extend_enabled(&mut out);
        self.system.extend_enabled(&mut out);
        self.testing.extend_enabled(&mut out);
        out
    }

    fn requires_runtime(&self) -> bool {
        self.fiber.requires_runtime()
    }
}

fn rust_string_vec(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("{value:?}.into()"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn path_uses_parent_dir(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn validate_config_paths(config: &RunConfig) -> Result<(), String> {
    if let Some(entry) = &config.entry
        && path_uses_parent_dir(entry)
    {
        return Err(format!(
            "entry path must not contain '..': {entry:?} (use a cwd-relative './...' path or an absolute path)"
        ));
    }

    let filesystem_families = [
        config.capabilities.filesystem.read_text.as_ref(),
        config.capabilities.filesystem.read_bytes.as_ref(),
        config.capabilities.filesystem.write_text.as_ref(),
        config.capabilities.filesystem.exists.as_ref(),
        config.capabilities.filesystem.list_entries.as_ref(),
        config.capabilities.filesystem.remove.as_ref(),
        config.capabilities.filesystem.create_directory.as_ref(),
        config.capabilities.filesystem.is_directory.as_ref(),
        config.capabilities.filesystem.is_regular_file.as_ref(),
        config.capabilities.filesystem.glob_matches.as_ref(),
        config.capabilities.filesystem.walk.as_ref(),
    ];

    if let Some(exec) = &config.capabilities.exec.run {
        for rule in &exec.allow {
            if rule.argv.is_empty() || rule.argv[0].is_empty() {
                return Err("exec allow rules need a command: argv must not be empty".into());
            }
            if rule.argv.iter().any(|arg| arg.contains('\0')) {
                return Err(format!("exec allow rule {:?} contains NUL", rule.argv));
            }
        }
    }

    for leaf in filesystem_families.into_iter().flatten() {
        for folder in &leaf.folders {
            if path_uses_parent_dir(folder) {
                return Err(format!(
                    "filesystem folder paths must not contain '..': {folder:?} (use a cwd-relative './...' path or an absolute path)"
                ));
            }
        }
    }

    Ok(())
}

/// Where a binary generated by `build-vm` gets its program and modules
/// ([`RunConfig::to_rust_source_for`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedProgram {
    /// The script named on the command line (or the config's `entry`), with
    /// named imports from the lockfile, cache or network at run time.
    ScriptArgument,
    /// Named imports resolve only from `embedded::SOURCES`, a module that
    /// `build-vm` writes next to `main.rs`. With `script`, the program is
    /// `embedded::SCRIPT` and the binary refuses any argument.
    Sealed { script: bool },
}

impl Default for RunConfig {
    #[allow(clippy::derivable_impls)]
    fn default() -> Self {
        Self {
            entry: None,
            limits: Limits::default(),
            capabilities: Capabilities::default(),
            extensions: BTreeMap::new(),
        }
    }
}

/// Former capability keys and their #186 replacements, used only to explain
/// a failed policy parse. There are no compatibility aliases: old keys fail.
const RENAMED_CAPABILITY_KEYS: &[(&str, &str)] = &[
    ("capabilities.bytes.bytes_get", "capabilities.bytes.get"),
    (
        "capabilities.bytes.bytes_length",
        "capabilities.bytes.length",
    ),
    ("capabilities.bytes.bytes_slice", "capabilities.bytes.slice"),
    (
        "capabilities.bytes.bytes_to_string",
        "capabilities.bytes.to_string",
    ),
    (
        "capabilities.bytes.string_to_bytes",
        "capabilities.bytes.of_string",
    ),
    (
        "capabilities.convert.char_to_int",
        "capabilities.char.to_int",
    ),
    (
        "capabilities.convert.float_to_string",
        "capabilities.float.to_string",
    ),
    (
        "capabilities.convert.int_to_char",
        "capabilities.char.of_int",
    ),
    (
        "capabilities.convert.int_to_float",
        "capabilities.float.of_int",
    ),
    (
        "capabilities.convert.int_to_string",
        "capabilities.int.to_string",
    ),
    (
        "capabilities.convert.int_to_word",
        "capabilities.word.of_int",
    ),
    (
        "capabilities.convert.string_to_int",
        "capabilities.int.of_string",
    ),
    (
        "capabilities.convert.string_to_word",
        "capabilities.word.of_string",
    ),
    (
        "capabilities.convert.word_to_int",
        "capabilities.word.to_int",
    ),
    (
        "capabilities.convert.word_to_string",
        "capabilities.word.to_string",
    ),
    ("capabilities.env.getenv", "capabilities.env.get"),
    ("capabilities.exec.exec", "capabilities.exec.run"),
    (
        "capabilities.fiber.fiber_await_all",
        "capabilities.fiber.join_all",
    ),
    (
        "capabilities.filesystem.create_dir",
        "capabilities.filesystem.create_directory",
    ),
    (
        "capabilities.filesystem.file_exists",
        "capabilities.filesystem.exists",
    ),
    (
        "capabilities.filesystem.glob",
        "capabilities.filesystem.glob_matches",
    ),
    (
        "capabilities.filesystem.is_dir",
        "capabilities.filesystem.is_directory",
    ),
    (
        "capabilities.filesystem.is_file",
        "capabilities.filesystem.is_regular_file",
    ),
    (
        "capabilities.filesystem.list_dir",
        "capabilities.filesystem.list_entries",
    ),
    (
        "capabilities.filesystem.read_file",
        "capabilities.filesystem.read_text",
    ),
    (
        "capabilities.filesystem.read_file_bytes",
        "capabilities.filesystem.read_bytes",
    ),
    (
        "capabilities.filesystem.remove_file",
        "capabilities.filesystem.remove",
    ),
    (
        "capabilities.filesystem.walk_dir",
        "capabilities.filesystem.walk",
    ),
    (
        "capabilities.filesystem.write_file",
        "capabilities.filesystem.write_text",
    ),
    ("capabilities.hash.blake3", "capabilities.hash.blake3.hash"),
    ("capabilities.http.http", "capabilities.http.client.request"),
    (
        "capabilities.http.http_bytes",
        "capabilities.http.client.request_bytes",
    ),
    ("capabilities.http.http_get", "capabilities.http.client.get"),
    (
        "capabilities.http.http_json",
        "capabilities.http.client.request_json",
    ),
    (
        "capabilities.http.http_msgpack",
        "capabilities.http.client.request_msgpack",
    ),
    ("capabilities.json.json_get", "capabilities.json.get"),
    ("capabilities.json.json_keys", "capabilities.json.keys"),
    ("capabilities.json.json_length", "capabilities.json.length"),
    ("capabilities.json.json_parse", "capabilities.json.parse"),
    (
        "capabilities.json.json_to_string",
        "capabilities.json.to_string",
    ),
    ("capabilities.math.abs_float", "capabilities.float.abs"),
    ("capabilities.math.abs_int", "capabilities.int.abs"),
    ("capabilities.math.ceil", "capabilities.float.ceil"),
    ("capabilities.math.floor", "capabilities.float.floor"),
    ("capabilities.math.sqrt", "capabilities.float.sqrt"),
    ("capabilities.path.path_join", "capabilities.path.join"),
    (
        "capabilities.process.await_process",
        "capabilities.fiber.join_or_fail",
    ),
    ("capabilities.process.cancel", "capabilities.fiber.cancel"),
    ("capabilities.process.spawn", "capabilities.fiber.spawn"),
    (
        "capabilities.process.wait_any",
        "capabilities.fiber.wait_any",
    ),
    (
        "capabilities.random.random_bytes",
        "capabilities.random.entropy",
    ),
    ("capabilities.random.rng_bytes", "capabilities.random.bytes"),
    (
        "capabilities.random.rng_int",
        "capabilities.random.int_between",
    ),
    ("capabilities.random.rng_seed", "capabilities.random.seed"),
    (
        "capabilities.regex.regex_match",
        "capabilities.regex.matches",
    ),
    (
        "capabilities.regex.regex_replace",
        "capabilities.regex.replace",
    ),
    (
        "capabilities.string.string_contains",
        "capabilities.string.contains",
    ),
    (
        "capabilities.string.string_join",
        "capabilities.string.join",
    ),
    (
        "capabilities.string.string_length",
        "capabilities.string.length",
    ),
    (
        "capabilities.string.string_replace",
        "capabilities.string.replace",
    ),
    ("capabilities.string.to_lower", "capabilities.string.lower"),
    ("capabilities.string.to_upper", "capabilities.string.upper"),
    ("capabilities.testing.assert", "capabilities.testing.check"),
    (
        "capabilities.testing.assert_eq",
        "capabilities.testing.equal",
    ),
    ("capabilities.testing.panic", "capabilities.system.panic"),
    ("capabilities.time.epoch_ms", "capabilities.time.now"),
    (
        "capabilities.time.monotonic_ms",
        "capabilities.time.monotonic_now",
    ),
];

/// Guidance for a policy that still names pre-#186 capability keys.
fn old_capability_key_guidance(text: &str) -> Option<String> {
    if text.contains("capabilities.date.") {
        return Some(
            "the capabilities.date family was removed; date operations need no capability".into(),
        );
    }
    let renames: Vec<String> = RENAMED_CAPABILITY_KEYS
        .iter()
        .filter(|(old, _)| text.contains(old))
        .take(3)
        .map(|(old, new)| format!("{old} is now {new}"))
        .collect();
    if renames.is_empty() {
        None
    } else {
        Some(renames.join("; "))
    }
}

impl RunConfig {
    /// Parse a run config from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| {
            let mut message = hiko_compile::compiler::toml_error_text(&e, text);
            if let Some(guidance) = old_capability_key_guidance(text) {
                message.push_str("; ");
                message.push_str(&guidance);
            }
            message
        })?;
        validate_config_paths(&config)?;
        Ok(config)
    }

    /// Configure a VM builder with this run config.
    pub fn apply_to_builder(&self, builder: VMBuilder) -> VMBuilder {
        let mut builder = self.capabilities.apply(builder);

        if let Some(work) = self.limits.max_work.or(self.limits.max_fuel) {
            builder = builder.max_work(work);
        }
        if let Some(heap) = self.limits.max_heap {
            builder = builder.max_heap(heap);
        }
        if let Some(max_memory_bytes) = self.limits.max_memory_bytes {
            builder = builder.max_memory_bytes(max_memory_bytes);
        }
        if let Some(max_io_bytes) = self.limits.max_io_bytes {
            builder = builder.max_io_bytes(max_io_bytes);
        }
        if let Some(max_host_work) = self.limits.max_host_work {
            builder = builder.max_host_work(max_host_work);
        }
        if let Some(max_host_resources) = self.limits.max_host_resources {
            builder = builder.max_host_resources(max_host_resources);
        }
        if let Some(count) = self.limits.max_live_processes {
            builder = builder.max_live_processes(count);
        }
        if let Some(count) = self.limits.max_spawns {
            builder = builder.max_spawns(count);
        }
        if let Some(bytes) = self.limits.max_message_bytes {
            builder = builder.max_message_bytes(bytes);
        }
        if let Some(bytes) = self.limits.max_join_group_bytes {
            builder = builder.max_join_group_bytes(bytes);
        }
        if !self.limits.host_resources.is_empty() {
            builder = builder.host_resource_limits(self.limits.host_resources.clone());
        }
        for (_, value, setter) in self.limits.io_limit_settings() {
            builder = setter(builder, value);
        }

        builder
    }

    /// Build a VM for this run config and compiled program.
    pub fn build_vm(&self, program: CompiledProgram) -> VM {
        self.apply_to_builder(VMBuilder::new(program)).build()
    }

    /// The extensions this config turns on, from the ones linked into the
    /// host. A table for an extension that is not linked is an error.
    pub fn configured_extensions(
        &self,
        linked: &[&'static Extension],
    ) -> Result<Vec<&'static Extension>, String> {
        self.extensions
            .keys()
            .map(|name| {
                linked
                    .iter()
                    .find(|extension| extension.name == name)
                    .copied()
                    .ok_or_else(|| format!("extension '{name}' is not in this build of hiko"))
            })
            .collect()
    }

    /// Like [`RunConfig::build_vm`], with the extensions this config turns
    /// on. Compile the program with
    /// `Extension::compile_surface(&config.configured_extensions(linked)?)`.
    pub fn build_vm_with_extensions(
        &self,
        program: CompiledProgram,
        linked: &[&'static Extension],
    ) -> Result<VM, String> {
        let mut builder = self.apply_to_builder(VMBuilder::new(program));
        for extension in self.configured_extensions(linked)? {
            builder = builder.with_extension(extension, &self.extensions[extension.name])?;
        }
        Ok(builder.build())
    }

    /// Return builtin names enabled by this run config, including internal
    /// runtime-support builtins that do not have a policy capability leaf.
    pub fn enabled_builtin_names(&self) -> BTreeSet<&'static str> {
        self.capabilities.enabled_builtin_names()
    }

    /// Builtin names this config enables, including those of the
    /// extensions it turns on from `linked`.
    pub fn enabled_builtin_names_with_extensions(
        &self,
        linked: &[&'static Extension],
    ) -> BTreeSet<&'static str> {
        let mut names = self.enabled_builtin_names();
        for extension in linked {
            if self.extensions.contains_key(extension.name) {
                names.extend(extension.builtins.iter().map(|builtin| builtin.name));
            }
        }
        names
    }

    /// Return whether this config enables process builtins that need the
    /// runtime scheduler instead of direct `VM::run()`.
    pub fn requires_runtime(&self) -> bool {
        self.capabilities.requires_runtime()
    }

    /// Return the config leaf path that enables a builtin, if it is modeled
    /// as a capability leaf in the run config surface.
    pub fn capability_path_for_builtin(name: &str) -> Option<&'static str> {
        meta_capability_path_for_builtin(name)
    }

    /// Generate Rust source code for a main.rs that bakes this config in.
    ///
    /// A config that turns on extensions is an error here: the generated
    /// crate must link them ([`RunConfig::to_rust_source_for`]).
    pub fn to_rust_source(&self) -> Result<String, String> {
        self.to_rust_source_for(GeneratedProgram::ScriptArgument, &[])
    }

    /// Like [`RunConfig::to_rust_source`], choosing where the generated
    /// binary gets its program and modules.
    ///
    /// `extensions` names the Rust path of each extension this config turns
    /// on, such as `("github", "hiko_ext_github::GITHUB")`; the generated
    /// crate must depend on those crates and on `toml` 0.8. A configured
    /// extension missing from `extensions` is an error.
    pub fn to_rust_source_for(
        &self,
        program: GeneratedProgram,
        extensions: &[(&str, &str)],
    ) -> Result<String, String> {
        let mut paths = Vec::new();
        for name in self.extensions.keys() {
            let path = extensions
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, path)| *path)
                .ok_or_else(|| format!("extension '{name}' has no crate to link"))?;
            paths.push((name.as_str(), path));
        }
        let mut s = String::new();
        s.push_str("use hiko_vm::builder::VMBuilder;\n");
        s.push_str("use hiko_compile::compiler::{Compiler, ModuleLoaderOptions};\n");
        s.push_str("use hiko_syntax::lexer::Lexer;\n");
        s.push_str("use hiko_syntax::parser::Parser;\n\n");
        if program != GeneratedProgram::ScriptArgument {
            s.push_str("mod embedded;\n\n");
        }
        if !paths.is_empty() {
            let tables = toml::to_string(&self.extensions)
                .map_err(|e| format!("cannot write the [extensions] tables: {e}"))?;
            s.push_str("/// The run config's `[extensions]` tables.\n");
            s.push_str(&format!("static EXTENSIONS: &str = {tables:?};\n\n"));
        }
        s.push_str("fn main() {\n");
        if program == (GeneratedProgram::Sealed { script: true }) {
            s.push_str("    if std::env::args().len() > 1 {\n");
            s.push_str("        eprintln!(\"error: this artifact runs only its embedded script and takes no arguments\");\n");
            s.push_str("        std::process::exit(2);\n");
            s.push_str("    }\n");
            s.push_str("    let source = embedded::SCRIPT;\n");
        } else {
            if let Some(entry) = &self.entry {
                s.push_str(&format!(
                    "    let path = std::env::args().nth(1).unwrap_or_else(|| \"{}\".to_string());\n",
                    entry.replace('\\', "\\\\").replace('"', "\\\"")
                ));
            } else {
                s.push_str(
                    "    let path = std::env::args().nth(1).expect(\"usage: <script.hml>\");\n",
                );
            }
            s.push_str(
                "    let source = std::fs::read_to_string(&path).expect(\"cannot read file\");\n",
            );
        }
        s.push_str("    let tokens = Lexer::new(&source, 0).tokenize().expect(\"lex error\");\n");
        s.push_str(
            "    let program = Parser::new(tokens).parse_program().expect(\"parse error\");\n",
        );
        match program {
            GeneratedProgram::ScriptArgument => s.push_str(
                "    let options = ModuleLoaderOptions::default();\n",
            ),
            GeneratedProgram::Sealed { .. } => s.push_str(
                "    let options = ModuleLoaderOptions { embedded: Some(&embedded::SOURCES), ..ModuleLoaderOptions::default() };\n",
            ),
        }
        if !paths.is_empty() {
            let list = paths
                .iter()
                .map(|(_, path)| format!("&{path}"))
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(&format!(
                "    let extensions: [&'static hiko_vm::extension::Extension; {}] = [{list}];\n",
                paths.len()
            ));
            s.push_str("    let extension_tables: toml::Table = toml::from_str(EXTENSIONS).expect(\"[extensions] tables\");\n");
            s.push_str("    let options = ModuleLoaderOptions { extensions: hiko_vm::extension::Extension::compile_surface(&extensions), ..options };\n");
        }
        if program == (GeneratedProgram::Sealed { script: true }) {
            s.push_str(
                "    let (compiled, _) = Compiler::compile_embedded(program, options).expect(\"compile error\");\n",
            );
        } else {
            s.push_str(
                "    let (compiled, _) = Compiler::compile_file_with_options(program, std::path::Path::new(&path), options).expect(\"compile error\");\n",
            );
        }
        s.push_str("    let mut vm = {\n");
        s.push_str("        let builder = VMBuilder::new(compiled)\n");
        self.capabilities.emit(&mut s);
        if let Some(work) = self.limits.max_work.or(self.limits.max_fuel) {
            s.push_str(&format!("            .max_work({work})\n"));
        }
        if let Some(heap) = self.limits.max_heap {
            s.push_str(&format!("            .max_heap({heap})\n"));
        }
        if let Some(max_memory_bytes) = self.limits.max_memory_bytes {
            s.push_str(&format!(
                "            .max_memory_bytes({max_memory_bytes})\n"
            ));
        }
        if let Some(max_io_bytes) = self.limits.max_io_bytes {
            s.push_str(&format!("            .max_io_bytes({max_io_bytes})\n"));
        }
        if let Some(max_host_work) = self.limits.max_host_work {
            s.push_str(&format!("            .max_host_work({max_host_work})\n"));
        }
        if let Some(max_host_resources) = self.limits.max_host_resources {
            s.push_str(&format!(
                "            .max_host_resources({max_host_resources})\n"
            ));
        }
        if let Some(count) = self.limits.max_live_processes {
            s.push_str(&format!("            .max_live_processes({count})\n"));
        }
        if let Some(count) = self.limits.max_spawns {
            s.push_str(&format!("            .max_spawns({count})\n"));
        }
        if let Some(bytes) = self.limits.max_message_bytes {
            s.push_str(&format!("            .max_message_bytes({bytes})\n"));
        }
        if let Some(bytes) = self.limits.max_join_group_bytes {
            s.push_str(&format!("            .max_join_group_bytes({bytes})\n"));
        }
        if !self.limits.host_resources.is_empty() {
            let mut limits: Vec<_> = self.limits.host_resources.iter().collect();
            limits.sort_by_key(|(left, _)| *left);
            s.push_str("            .host_resource_limits(std::collections::HashMap::from([\n");
            for (kind, limit) in limits {
                let kind = kind.replace('\\', "\\\\").replace('"', "\\\"");
                s.push_str(&format!(
                    "                (\"{kind}\".to_string(), {limit}usize),\n"
                ));
            }
            s.push_str("            ]))\n");
        }
        for (name, value, _) in self.limits.io_limit_settings() {
            s.push_str(&format!("            .{name}({value})\n"));
        }
        s.push_str("            ;\n");
        for (i, (name, _)) in paths.iter().enumerate() {
            s.push_str(&format!(
                "        let builder = builder.with_extension(extensions[{i}], &extension_tables[{name:?}]).unwrap_or_else(|e| {{ eprintln!(\"error: [extensions.{name}]: {{e}}\"); std::process::exit(1) }});\n"
            ));
        }
        s.push_str("        builder.build()\n");
        s.push_str("    };\n");
        s.push_str(
            "    vm.set_output_sink(std::sync::Arc::new(hiko_vm::vm::StdoutOutputSink::default()));\n",
        );
        s.push_str("    vm.set_error_sink(std::sync::Arc::new(hiko_vm::vm::StderrOutputSink));\n");
        s.push_str("    match vm.run() {\n");
        s.push_str("        Ok(()) => {}\n");
        s.push_str("        Err(e) => {\n");
        s.push_str("            eprintln!(\"error: {}\", e.message);\n");
        s.push_str("            std::process::exit(1);\n");
        s.push_str("        }\n");
        s.push_str("    }\n");
        s.push_str("}\n");
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::RunConfig;
    use hiko_compile::chunk::CompiledProgram;
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::lexer::Lexer;
    use hiko_syntax::parser::Parser;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn compile_program(source: &str) -> CompiledProgram {
        let tokens = Lexer::new(source, 0).tokenize().expect("lex");
        let program = Parser::new(tokens).parse_program().expect("parse");
        Compiler::compile(program).expect("compile").0
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "hiko-config-{}-{}-{}",
            name,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn toml_path_literal(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "\\\\")
    }

    fn load_policy_text(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../policies")
            .join(name);
        fs::read_to_string(path).expect("policy file should exist")
    }

    #[test]
    fn bulk_join_policy_is_independent_default_off_and_codegen_preserves_storage_limit() {
        let denied = RunConfig::from_toml("").unwrap();
        assert!(!denied.enabled_builtin_names().contains("fiber_await_all"));
        let config = RunConfig::from_toml("[limits]\nmax_join_group_bytes = 12345\n[capabilities.fiber.join_all]\nenabled = true\n").unwrap();
        assert!(config.requires_runtime());
        let enabled = config.enabled_builtin_names();
        assert!(enabled.contains("fiber_await_all"));
        for peer in [
            "spawn",
            "await_process",
            "await_process_result",
            "cancel",
            "wait_any",
        ] {
            assert!(!enabled.contains(peer), "bulk must not imply {peer}");
        }
        let source = config.to_rust_source().unwrap();
        assert!(source.contains(".max_join_group_bytes(12345)"));
        assert!(source.contains(".register_builtin_name(\"fiber_await_all\")"));
        let vm = config.build_vm(compile_program("val x = 0"));
        assert_eq!(vm.join_group_budget().limit(), 12345);
        assert_eq!(vm.join_group_budget().used(), 0);
    }

    #[test]
    fn parse_run_config_with_entry_and_filesystem_leaf() {
        let config = RunConfig::from_toml(
            r#"
entry = "scripts/read.hml"

[capabilities.filesystem.read_text]
enabled = true
folders = ["."]
"#,
        )
        .expect("run config should parse");

        assert_eq!(config.entry.as_deref(), Some("scripts/read.hml"));
        let leaf = config
            .capabilities
            .filesystem
            .read_text
            .expect("filesystem read_file config missing");
        assert!(leaf.enabled);
        assert_eq!(leaf.folders, vec![".".to_string()]);
    }

    #[test]
    fn parse_full_builtin_example_config() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/builtins/full-builtin-run-config.example.toml");
        let text = std::fs::read_to_string(path).expect("example config should exist");
        RunConfig::from_toml(&text).expect("full builtin example config should parse");
    }

    /// Diagnostics tell users to "enable [<capability_path>]", so every path in
    /// the builtin metadata must be a run-config leaf that enables that builtin.
    #[test]
    fn every_builtin_capability_path_enables_its_builtin() {
        for meta in hiko_builtin_meta::builtins() {
            let Some(path) = meta.capability_path else {
                continue;
            };
            let config = RunConfig::from_toml(&format!("[{path}]\nenabled = true\n"))
                .unwrap_or_else(|err| panic!("{}: [{path}] is rejected: {err}", meta.name));
            assert!(
                config.enabled_builtin_names().contains(meta.name),
                "{}: enabling [{path}] does not enable it",
                meta.name
            );
        }
    }

    #[test]
    fn policy_configs_include_unrestricted_runtime_numeric_builtins() {
        let config = RunConfig::from_toml("").expect("empty config should parse");
        let enabled = config.enabled_builtin_names();
        assert!(enabled.contains("numeric_int32_min_value"));
        assert!(enabled.contains("numeric_word32_add"));
        assert!(enabled.contains("numeric_float32_add"));
        assert!(!enabled.contains("println"));

        let rust_source = config.to_rust_source().unwrap();
        assert!(rust_source.contains(".register_builtin_name(\"numeric_int32_min_value\")"));
        assert!(rust_source.contains(".register_builtin_name(\"numeric_word32_add\")"));
        assert!(rust_source.contains(".register_builtin_name(\"numeric_float32_add\")"));
    }

    #[test]
    fn named_core_policy_preserves_the_core_builtin_surface() {
        let config = RunConfig::from_toml(include_str!("../../../policies/core.policy.toml"))
            .expect("core policy should parse");
        let expected = hiko_builtin_meta::core_builtin_names().collect();
        // Policy leaves can name families omitted from this Cargo build;
        // registration only includes builtins linked into the host.
        let available = config
            .enabled_builtin_names()
            .into_iter()
            .filter(|name| hiko_builtin_meta::builtin_meta(name).is_some())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(available, expected);
    }

    #[test]
    fn rust_source_includes_host_resource_limits() {
        let config = RunConfig::from_toml(
            r#"
[limits]
max_host_resources = 4

[limits.host_resources]
unsupported = 1
other = 2
"#,
        )
        .expect("host resource limits should parse");

        let rust_source = config.to_rust_source().unwrap();
        assert!(rust_source.contains(".max_host_resources(4)"));
        assert!(rust_source.contains(".host_resource_limits(std::collections::HashMap::from(["));
        assert!(rust_source.contains("(\"unsupported\".to_string(), 1usize)"));
        assert!(rust_source.contains("(\"other\".to_string(), 2usize)"));
    }

    #[test]
    fn parse_docs_writer_policy() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../policies/docs-writer.policy.toml");
        let text = std::fs::read_to_string(path).expect("docs writer policy should exist");
        RunConfig::from_toml(&text).expect("docs writer policy should parse");
    }

    #[test]
    fn parse_software_developer_role_policy() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../policies/software-developer-role.policy.toml");
        let text =
            std::fs::read_to_string(path).expect("software developer role policy should exist");
        let config =
            RunConfig::from_toml(&text).expect("software developer role policy should parse");
        assert!(config.requires_runtime());
    }

    #[test]
    fn parse_should_fail_io_policy() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../policies/should-fail-io.policy.toml");
        let text = std::fs::read_to_string(path).expect("should-fail-io policy should exist");
        RunConfig::from_toml(&text).expect("should-fail-io policy should parse");
    }

    #[test]
    fn parse_work_demo_policies() {
        for policy in [
            "work-demo-tight.policy.toml",
            "work-demo-enough.policy.toml",
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../policies")
                .join(policy);
            let text = std::fs::read_to_string(path).expect("work demo policy should exist");
            RunConfig::from_toml(&text).expect("work demo policy should parse");
        }
    }

    #[test]
    fn config_without_process_capabilities_does_not_require_runtime() {
        let config = RunConfig::from_toml(
            r#"
[capabilities.stdio.println]
enabled = true
"#,
        )
        .expect("config should parse");
        assert!(!config.requires_runtime());
    }

    #[test]
    fn docs_policy_allows_writes_inside_docs_and_rejects_outside() {
        let root = temp_dir("docs-policy");
        let docs_dir = root.join("docs");
        fs::create_dir_all(&docs_dir).unwrap();

        // Importing __Builtin.Filesystem binds every member eagerly, so the
        // policy must enable `remove` too; keep it scoped to the docs dir.
        let policy = load_policy_text("docs-writer.policy.toml")
            .replace(
                "folders = [\"./docs\"]",
                &format!("folders = [\"{}\"]", toml_path_literal(&docs_dir)),
            )
            .replace(
                "folders = [\".\"]",
                &format!("folders = [\"{}\"]", toml_path_literal(&root)),
            )
            + &format!(
                "\n[capabilities.filesystem.remove]\nenabled = true\nfolders = [\"{}\"]\n",
                toml_path_literal(&docs_dir)
            );
        let config = RunConfig::from_toml(&policy).expect("policy should parse");

        let allowed = docs_dir.join("allowed.md");
        let program = compile_program(&format!(
            "import __Builtin.Filesystem\nval _ = BuiltinFilesystem.write_text ({:?}, \"ok\")",
            allowed.to_string_lossy()
        ));
        let mut vm = config.build_vm(program);
        vm.run().expect("write under docs should succeed");
        assert_eq!(fs::read_to_string(&allowed).unwrap(), "ok");

        let blocked = root.join("README.md");
        let program = compile_program(&format!(
            "import __Builtin.Filesystem\nval _ = BuiltinFilesystem.write_text ({:?}, \"nope\")",
            blocked.to_string_lossy()
        ));
        let mut vm = config.build_vm(program);
        let err = vm.run().expect_err("write outside docs should fail");
        assert!(err.message.contains("outside allowed root"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn docs_policy_does_not_expose_exec() {
        let config = RunConfig::from_toml(&load_policy_text("docs-writer.policy.toml"))
            .expect("policy should parse");
        let program =
            compile_program("import __Builtin.Exec\nval _ = BuiltinExec.exec (\"echo\", [])");
        let mut vm = config.build_vm(program);
        let err = vm.run().expect_err("docs policy should not expose exec");
        assert!(err.message.contains("undefined global: exec"));
    }

    #[test]
    fn software_developer_role_policy_allows_localhost_http_and_rejects_remote_hosts() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind localhost test server");
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .expect("write response");
        });

        let config = RunConfig::from_toml(&load_policy_text("software-developer-role.policy.toml"))
            .expect("policy should parse");

        let program = compile_program(&format!(
            "import __Builtin.Http\nval response = BuiltinHttp.get \"http://127.0.0.1:{port}/\""
        ));
        let mut vm = config.build_vm(program);
        vm.run().expect("localhost http should be allowed");
        server.join().unwrap();

        let program = compile_program(
            "import __Builtin.Http\nval response = BuiltinHttp.get \"http://example.com/\"",
        );
        let mut vm = config.build_vm(program);
        let err = vm.run().expect_err("remote host should be rejected");
        assert!(err.message.contains("not in allowed hosts"));
    }

    #[test]
    fn reject_entry_with_parent_dir_component() {
        let err = RunConfig::from_toml(
            r#"
entry = "../scripts/read.hml"
"#,
        )
        .expect_err("parent dir entry should be rejected");
        assert!(err.contains("must not contain '..'"));
    }

    #[test]
    fn reject_filesystem_folder_with_parent_dir_component() {
        let err = RunConfig::from_toml(
            r#"
[capabilities.filesystem.read_text]
enabled = true
folders = ["../content"]
"#,
        )
        .expect_err("parent dir folders should be rejected");
        assert!(err.contains("must not contain '..'"));
    }
}
