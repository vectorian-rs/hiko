use crate::builder::{ExecPolicy as VmExecPolicy, FilesystemPolicy, HttpPolicy as VmHttpPolicy, VMBuilder};
use crate::vm::VM;
use hiko_compile::chunk::CompiledProgram;
use serde::Deserialize;

/// Runtime/loadable policy specification.
/// This struct describes what a VM can do for a specific invocation.
/// It can also be embedded into generated artifacts by build tools.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub entry: Option<String>,
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
            entry: None,
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

    /// Configure a VM builder with this policy.
    pub fn apply_to_builder(&self, mut builder: VMBuilder) -> VMBuilder {
        if self.core.enabled {
            builder = builder.with_core();
        }

        if let Some(fs) = &self.filesystem {
            builder = builder.with_filesystem(FilesystemPolicy {
                root: fs.root.clone(),
                allow_read: fs.read,
                allow_write: fs.write,
                allow_delete: fs.delete,
            });
        }

        if let Some(http) = &self.http {
            builder = builder.with_http(VmHttpPolicy {
                allowed_hosts: http.allowed_hosts.clone(),
            });
        }

        if let Some(exec) = &self.exec {
            builder = builder.with_exec(VmExecPolicy {
                allowed: exec.allowed.clone(),
                timeout: exec.timeout,
            });
        }

        if self.system.allow_exit {
            builder = builder.with_exit();
        }

        if let Some(fuel) = self.limits.max_fuel {
            builder = builder.max_fuel(fuel);
        }
        if let Some(heap) = self.limits.max_heap {
            builder = builder.max_heap(heap);
        }

        builder
    }

    /// Build a VM for this policy and compiled program.
    pub fn build_vm(&self, program: CompiledProgram) -> VM {
        self.apply_to_builder(VMBuilder::new(program)).build()
    }

    /// Generate Rust source code for a main.rs that bakes this policy in.
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

        if self.core.enabled {
            s.push_str("            .with_core()\n");
        }

        if let Some(fs) = &self.filesystem {
            s.push_str(&format!(
                "            .with_filesystem(hiko_vm::builder::FilesystemPolicy {{\n\
                 \x20               root: \"{}\".into(),\n\
                 \x20               allow_read: {},\n\
                 \x20               allow_write: {},\n\
                 \x20               allow_delete: {},\n\
                 \x20           }})\n",
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
                "            .with_http(hiko_vm::builder::HttpPolicy {{\n\
                 \x20               allowed_hosts: vec![{}],\n\
                 \x20           }})\n",
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
                "            .with_exec(hiko_vm::builder::ExecPolicy {{\n\
                 \x20               allowed: vec![{}],\n\
                 \x20               timeout: {},\n\
                 \x20           }})\n",
                cmds.join(", "),
                exec.timeout
            ));
        }

        if self.system.allow_exit {
            s.push_str("            .with_exit()\n");
        }

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
    use super::Policy;

    #[test]
    fn parse_policy_with_entry_and_filesystem() {
        let policy = Policy::from_toml(
            r#"
entry = "scripts/read.hml"

[filesystem]
root = "."
read = true
"#,
        )
        .expect("policy should parse");

        assert_eq!(policy.entry.as_deref(), Some("scripts/read.hml"));
        let fs = policy.filesystem.expect("filesystem policy missing");
        assert_eq!(fs.root, ".");
        assert!(fs.read);
        assert!(!fs.write);
        assert!(!fs.delete);
    }
}
