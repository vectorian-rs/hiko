use serde::Deserialize;

/// Compile-time policy specification.
/// This struct describes what a VM can do. It is used by build tools
/// to generate a VMBuilder configuration that gets compiled into the binary.
/// At runtime, the policy is already baked in -- there is no config file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub core: CorePolicy,
    pub filesystem: Option<FsPolicy>,
    pub http: Option<HttpPolicy>,
    pub exec: Option<ExecPolicy>,
    #[serde(default)]
    pub system: SystemPolicy,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_fuel: Option<u64>,
    pub max_heap: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorePolicy {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for CorePolicy {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsPolicy {
    #[serde(default = "default_dot")]
    pub root: String,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub delete: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpPolicy {
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecPolicy {
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default = "default_exec_timeout")]
    pub timeout: u64,
}

fn default_exec_timeout() -> u64 {
    30
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SystemPolicy {
    #[serde(default)]
    pub allow_exit: bool,
}

fn default_true() -> bool {
    true
}

fn default_dot() -> String {
    ".".to_string()
}

impl Default for Policy {
    #[allow(clippy::derivable_impls)]
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            core: CorePolicy::default(),
            filesystem: None,
            http: None,
            exec: None,
            system: SystemPolicy::default(),
        }
    }
}

impl Policy {
    /// Parse a policy from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Generate Rust source code for a main.rs that bakes this policy in.
    pub fn to_rust_source(&self) -> String {
        let mut s = String::new();
        s.push_str("use hiko_vm::builder::VMBuilder;\n");
        s.push_str("use hiko_compile::compiler::Compiler;\n");
        s.push_str("use hiko_syntax::lexer::Lexer;\n");
        s.push_str("use hiko_syntax::parser::Parser;\n\n");
        s.push_str("fn main() {\n");
        s.push_str("    let path = std::env::args().nth(1).expect(\"usage: <script.hml>\");\n");
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
        s.push_str("    let mut vm = VMBuilder::new(compiled)\n");

        if self.core.enabled {
            s.push_str("        .with_core()\n");
        }

        if let Some(fs) = &self.filesystem {
            s.push_str(&format!(
                "        .with_filesystem(hiko_vm::builder::FilesystemPolicy {{\n\
                 \x20           root: \"{}\".into(),\n\
                 \x20           allow_read: {},\n\
                 \x20           allow_write: {},\n\
                 \x20           allow_delete: {},\n\
                 \x20       }})\n",
                fs.root, fs.read, fs.write, fs.delete
            ));
        }

        if let Some(http) = &self.http {
            let hosts: Vec<String> = http
                .allowed_hosts
                .iter()
                .map(|h| format!("\"{h}\".into()"))
                .collect();
            s.push_str(&format!(
                "        .with_http(hiko_vm::builder::HttpPolicy {{\n\
                 \x20           allowed_hosts: vec![{}],\n\
                 \x20       }})\n",
                hosts.join(", ")
            ));
        }

        if let Some(exec) = &self.exec {
            let cmds: Vec<String> = exec
                .allowed
                .iter()
                .map(|c| format!("\"{c}\".into()"))
                .collect();
            s.push_str(&format!(
                "        .with_exec(hiko_vm::builder::ExecPolicy {{\n\
                 \x20           allowed: vec![{}],\n\
                 \x20           timeout: {},\n\
                 \x20       }})\n",
                cmds.join(", "),
                exec.timeout
            ));
        }

        if self.system.allow_exit {
            s.push_str("        .with_exit()\n");
        }

        if let Some(fuel) = self.limits.max_fuel {
            s.push_str(&format!("        .max_fuel({fuel})\n"));
        }
        if let Some(heap) = self.limits.max_heap {
            s.push_str(&format!("        .max_heap({heap})\n"));
        }

        s.push_str("        .build();\n");
        s.push_str("    match vm.run() {\n");
        s.push_str("        Ok(()) => {\n");
        s.push_str("            for line in vm.get_output() {\n");
        s.push_str("                print!(\"{line}\");\n");
        s.push_str("            }\n");
        s.push_str("        }\n");
        s.push_str("        Err(e) => {\n");
        s.push_str("            for line in vm.get_output() {\n");
        s.push_str("                print!(\"{line}\");\n");
        s.push_str("            }\n");
        s.push_str("            eprintln!(\"error: {}\", e.message);\n");
        s.push_str("            std::process::exit(1);\n");
        s.push_str("        }\n");
        s.push_str("    }\n");
        s.push_str("}\n");
        s
    }
}
