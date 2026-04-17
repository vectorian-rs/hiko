use crate::builder::{ExecPolicy as VmExecPolicy, VMBuilder};
use crate::vm::VM;
use hiko_builtin_meta::capability_path_for_builtin as meta_capability_path_for_builtin;
use hiko_compile::chunk::CompiledProgram;
use serde::Deserialize;
use std::collections::BTreeSet;
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
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_fuel: Option<u64>,
    pub max_heap: Option<usize>,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecLeaf {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    allowed_commands: Vec<String>,
    #[serde(default = "default_exec_timeout")]
    timeout: u64,
}

impl Default for ExecLeaf {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_commands: Vec::new(),
            timeout: default_exec_timeout(),
        }
    }
}

fn default_exec_timeout() -> u64 {
    30
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
            fn apply(&self, mut builder: VMBuilder) -> VMBuilder {
                $(
                    if let Some(leaf) = &self.$field
                        && leaf.enabled
                    {
                        builder = builder.allow_filesystem_builtin($builtin, leaf.folders.clone());
                    }
                )*
                builder
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
            fn apply(&self, mut builder: VMBuilder) -> VMBuilder {
                $(
                    if let Some(leaf) = &self.$field
                        && leaf.enabled
                    {
                        builder = builder.allow_http_builtin($builtin, leaf.allowed_hosts.clone());
                    }
                )*
                builder
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

enabled_family!(StdioCapabilities {
    print => "print",
    println => "println",
    read_line => "read_line",
    read_stdin => "read_stdin",
});

enabled_family!(ConvertCapabilities {
    int_to_string => "int_to_string",
    float_to_string => "float_to_string",
    string_to_int => "string_to_int",
    char_to_int => "char_to_int",
    int_to_char => "int_to_char",
    int_to_float => "int_to_float",
});

enabled_family!(StringCapabilities {
    string_length => "string_length",
    substring => "substring",
    string_contains => "string_contains",
    trim => "trim",
    split => "split",
    string_replace => "string_replace",
    starts_with => "starts_with",
    ends_with => "ends_with",
    to_upper => "to_upper",
    to_lower => "to_lower",
    string_join => "string_join",
});

enabled_family!(RegexCapabilities {
    regex_match => "regex_match",
    regex_replace => "regex_replace",
});

enabled_family!(JsonCapabilities {
    json_parse => "json_parse",
    json_to_string => "json_to_string",
    json_get => "json_get",
    json_keys => "json_keys",
    json_length => "json_length",
});

enabled_family!(MathCapabilities {
    sqrt => "sqrt",
    abs_int => "abs_int",
    abs_float => "abs_float",
    floor => "floor",
    ceil => "ceil",
});

enabled_family!(BytesCapabilities {
    bytes_length => "bytes_length",
    bytes_to_string => "bytes_to_string",
    string_to_bytes => "string_to_bytes",
    bytes_get => "bytes_get",
    bytes_slice => "bytes_slice",
});

enabled_family!(HashCapabilities {
    blake3 => "blake3",
});

enabled_family!(RandomCapabilities {
    random_bytes => "random_bytes",
    rng_seed => "rng_seed",
    rng_bytes => "rng_bytes",
    rng_int => "rng_int",
});

enabled_family!(EnvCapabilities {
    getenv => "getenv",
});

enabled_family!(TimeCapabilities {
    epoch => "epoch",
    epoch_ms => "epoch_ms",
    monotonic_ms => "monotonic_ms",
    sleep => "sleep",
});

enabled_family!(ProcessCapabilities {
    spawn => "spawn",
    await_process => "await_process",
});

enabled_family!(PathCapabilities {
    path_join => "path_join",
});

folder_family!(FilesystemCapabilities {
    read_file => "read_file",
    read_file_bytes => "read_file_bytes",
    write_file => "write_file",
    file_exists => "file_exists",
    list_dir => "list_dir",
    remove_file => "remove_file",
    create_dir => "create_dir",
    is_dir => "is_dir",
    is_file => "is_file",
    read_file_tagged => "read_file_tagged",
    edit_file_tagged => "edit_file_tagged",
    glob => "glob",
    walk_dir => "walk_dir",
});

http_family!(HttpCapabilities {
    http_get => "http_get",
    http => "http",
    http_json => "http_json",
    http_msgpack => "http_msgpack",
    http_bytes => "http_bytes",
});

enabled_family!(SystemCapabilities {
    exit => "exit",
});

enabled_family!(TestingCapabilities {
    panic => "panic",
    assert => "assert",
    assert_eq => "assert_eq",
});

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ExecCapabilities {
    exec: Option<ExecLeaf>,
}

impl ExecCapabilities {
    fn apply(&self, builder: VMBuilder) -> VMBuilder {
        if let Some(leaf) = &self.exec
            && leaf.enabled
        {
            return builder.with_exec(VmExecPolicy {
                allowed: leaf.allowed_commands.clone(),
                timeout: leaf.timeout,
            });
        }
        builder
    }

    fn emit(&self, out: &mut String) {
        if let Some(leaf) = &self.exec
            && leaf.enabled
        {
            out.push_str(&format!(
                "            .with_exec(hiko_vm::builder::ExecPolicy {{\n\
                 \x20               allowed: vec![{}],\n\
                 \x20               timeout: {},\n\
                 \x20           }})\n",
                rust_string_vec(&leaf.allowed_commands),
                leaf.timeout
            ));
        }
    }

    fn extend_enabled(&self, out: &mut BTreeSet<&'static str>) {
        if let Some(leaf) = &self.exec
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
    pub convert: ConvertCapabilities,
    #[serde(default)]
    pub string: StringCapabilities,
    #[serde(default)]
    pub regex: RegexCapabilities,
    #[serde(default)]
    pub json: JsonCapabilities,
    #[serde(default)]
    pub math: MathCapabilities,
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
    pub process: ProcessCapabilities,
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
        let builder = self.stdio.apply(builder);
        let builder = self.convert.apply(builder);
        let builder = self.string.apply(builder);
        let builder = self.regex.apply(builder);
        let builder = self.json.apply(builder);
        let builder = self.math.apply(builder);
        let builder = self.bytes.apply(builder);
        let builder = self.hash.apply(builder);
        let builder = self.random.apply(builder);
        let builder = self.env.apply(builder);
        let builder = self.time.apply(builder);
        let builder = self.process.apply(builder);
        let builder = self.path.apply(builder);
        let builder = self.filesystem.apply(builder);
        let builder = self.http.apply(builder);
        let builder = self.exec.apply(builder);
        let builder = self.system.apply(builder);
        self.testing.apply(builder)
    }

    fn emit(&self, out: &mut String) {
        self.stdio.emit(out);
        self.convert.emit(out);
        self.string.emit(out);
        self.regex.emit(out);
        self.json.emit(out);
        self.math.emit(out);
        self.bytes.emit(out);
        self.hash.emit(out);
        self.random.emit(out);
        self.env.emit(out);
        self.time.emit(out);
        self.process.emit(out);
        self.path.emit(out);
        self.filesystem.emit(out);
        self.http.emit(out);
        self.exec.emit(out);
        self.system.emit(out);
        self.testing.emit(out);
    }

    fn enabled_builtin_names(&self) -> BTreeSet<&'static str> {
        let mut out = BTreeSet::new();
        self.stdio.extend_enabled(&mut out);
        self.convert.extend_enabled(&mut out);
        self.string.extend_enabled(&mut out);
        self.regex.extend_enabled(&mut out);
        self.json.extend_enabled(&mut out);
        self.math.extend_enabled(&mut out);
        self.bytes.extend_enabled(&mut out);
        self.hash.extend_enabled(&mut out);
        self.random.extend_enabled(&mut out);
        self.env.extend_enabled(&mut out);
        self.time.extend_enabled(&mut out);
        self.process.extend_enabled(&mut out);
        self.path.extend_enabled(&mut out);
        self.filesystem.extend_enabled(&mut out);
        self.http.extend_enabled(&mut out);
        self.exec.extend_enabled(&mut out);
        self.system.extend_enabled(&mut out);
        self.testing.extend_enabled(&mut out);
        out
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
        config.capabilities.filesystem.read_file.as_ref(),
        config.capabilities.filesystem.read_file_bytes.as_ref(),
        config.capabilities.filesystem.write_file.as_ref(),
        config.capabilities.filesystem.file_exists.as_ref(),
        config.capabilities.filesystem.list_dir.as_ref(),
        config.capabilities.filesystem.remove_file.as_ref(),
        config.capabilities.filesystem.create_dir.as_ref(),
        config.capabilities.filesystem.is_dir.as_ref(),
        config.capabilities.filesystem.is_file.as_ref(),
        config.capabilities.filesystem.read_file_tagged.as_ref(),
        config.capabilities.filesystem.edit_file_tagged.as_ref(),
        config.capabilities.filesystem.glob.as_ref(),
        config.capabilities.filesystem.walk_dir.as_ref(),
    ];

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

impl Default for RunConfig {
    #[allow(clippy::derivable_impls)]
    fn default() -> Self {
        Self {
            entry: None,
            limits: Limits::default(),
            capabilities: Capabilities::default(),
        }
    }
}

impl RunConfig {
    /// Parse a run config from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        validate_config_paths(&config)?;
        Ok(config)
    }

    /// Configure a VM builder with this run config.
    pub fn apply_to_builder(&self, builder: VMBuilder) -> VMBuilder {
        let mut builder = self.capabilities.apply(builder);

        if let Some(fuel) = self.limits.max_fuel {
            builder = builder.max_fuel(fuel);
        }
        if let Some(heap) = self.limits.max_heap {
            builder = builder.max_heap(heap);
        }

        builder
    }

    /// Build a VM for this run config and compiled program.
    pub fn build_vm(&self, program: CompiledProgram) -> VM {
        self.apply_to_builder(VMBuilder::new(program)).build()
    }

    /// Return the public builtin names enabled by this run config.
    pub fn enabled_builtin_names(&self) -> BTreeSet<&'static str> {
        self.capabilities.enabled_builtin_names()
    }

    /// Return the config leaf path that enables a builtin, if it is modeled
    /// as a capability leaf in the run config surface.
    pub fn capability_path_for_builtin(name: &str) -> Option<&'static str> {
        meta_capability_path_for_builtin(name)
    }

    /// Generate Rust source code for a main.rs that bakes this config in.
    pub fn to_rust_source(&self) -> String {
        let mut s = String::new();
        s.push_str("use hiko_vm::builder::VMBuilder;\n");
        s.push_str("use hiko_compile::compiler::Compiler;\n");
        s.push_str("use hiko_syntax::lexer::Lexer;\n");
        s.push_str("use hiko_syntax::parser::Parser;\n\n");
        s.push_str("fn main() {\n");
        if let Some(entry) = &self.entry {
            s.push_str(&format!(
                "    let path = std::env::args().nth(1).unwrap_or_else(|| \"{}\".to_string());\n",
                entry.replace('\\', "\\\\").replace('"', "\\\"")
            ));
        } else {
            s.push_str("    let path = std::env::args().nth(1).expect(\"usage: <script.hml>\");\n");
        }
        s.push_str(
            "    let source = std::fs::read_to_string(&path).expect(\"cannot read file\");\n",
        );
        s.push_str("    let tokens = Lexer::new(&source, 0).tokenize().expect(\"lex error\");\n");
        s.push_str(
            "    let program = Parser::new(tokens).parse_program().expect(\"parse error\");\n",
        );
        s.push_str(
            "    let (compiled, _) = Compiler::compile_file(program, std::path::Path::new(&path)).expect(\"compile error\");\n",
        );
        s.push_str("    let mut vm = {\n");
        s.push_str("        let builder = VMBuilder::new(compiled)\n");
        self.capabilities.emit(&mut s);
        if let Some(fuel) = self.limits.max_fuel {
            s.push_str(&format!("            .max_fuel({fuel})\n"));
        }
        if let Some(heap) = self.limits.max_heap {
            s.push_str(&format!("            .max_heap({heap})\n"));
        }
        s.push_str("            ;\n");
        s.push_str("        builder.build()\n");
        s.push_str("    };\n");
        s.push_str(
            "    vm.set_output_sink(std::sync::Arc::new(hiko_vm::vm::StdoutOutputSink::default()));\n",
        );
        s.push_str("    match vm.run() {\n");
        s.push_str("        Ok(()) => {}\n");
        s.push_str("        Err(e) => {\n");
        s.push_str("            eprintln!(\"error: {}\", e.message);\n");
        s.push_str("            std::process::exit(1);\n");
        s.push_str("        }\n");
        s.push_str("    }\n");
        s.push_str("}\n");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::RunConfig;
    use std::path::Path;

    #[test]
    fn parse_run_config_with_entry_and_filesystem_leaf() {
        let config = RunConfig::from_toml(
            r#"
entry = "scripts/read.hml"

[capabilities.filesystem.read_file]
enabled = true
folders = ["."]
"#,
        )
        .expect("run config should parse");

        assert_eq!(config.entry.as_deref(), Some("scripts/read.hml"));
        let leaf = config
            .capabilities
            .filesystem
            .read_file
            .expect("filesystem read_file config missing");
        assert!(leaf.enabled);
        assert_eq!(leaf.folders, vec![".".to_string()]);
    }

    #[test]
    fn parse_full_builtin_example_config() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/full-builtin-run-config.example.toml");
        let text = std::fs::read_to_string(path).expect("example config should exist");
        RunConfig::from_toml(&text).expect("full builtin example config should parse");
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
[capabilities.filesystem.read_file]
enabled = true
folders = ["../content"]
"#,
        )
        .expect_err("parent dir folders should be rejected");
        assert!(err.contains("must not contain '..'"));
    }
}
