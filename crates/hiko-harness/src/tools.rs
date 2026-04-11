//! Tool registry: loads .hml tool scripts and runs them in the hiko VM.

use crate::llm::{FunctionDef, ToolDef};
use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_vm::vm::VM;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A tool backed by a hiko script.
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub script_path: PathBuf,
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Tool>,
}

impl ToolRegistry {
    /// Create an empty registry (no tools).
    pub fn empty() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Load all .hml files from a tools directory.
    /// Each file's name (without .hml) becomes the tool name.
    /// The first comment block is parsed for metadata.
    pub fn load(tools_dir: &Path) -> Result<Self, String> {
        let mut tools = HashMap::new();

        let entries = std::fs::read_dir(tools_dir)
            .map_err(|e| format!("cannot read tools directory: {e}"))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("read_dir: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("hml") {
                continue;
            }

            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            let source =
                std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;

            let (description, parameters) = parse_tool_metadata(&source, &name);

            tools.insert(
                name.clone(),
                Tool {
                    name,
                    description,
                    parameters,
                    script_path: path,
                },
            );
        }

        Ok(Self { tools })
    }

    /// Get tool definitions for the LLM API.
    pub fn tool_defs(&self) -> Vec<ToolDef> {
        self.tools
            .values()
            .map(|t| ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect()
    }

    /// Execute a tool by name with the given JSON arguments.
    /// Sets arguments as environment variables for the script.
    pub fn execute(&self, name: &str, args_json: &str) -> Result<String, String> {
        let tool = self.tools.get(name).ok_or_else(|| format!("unknown tool: {name}"))?;

        // Parse arguments and set as env vars so the script can read them via getenv
        let args: serde_json::Value =
            serde_json::from_str(args_json).map_err(|e| format!("invalid tool args: {e}"))?;

        if let Some(obj) = args.as_object() {
            for (key, value) in obj {
                let val_str = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    other => other.to_string(),
                };
                // SAFETY: hiko-harness is single-threaded; no concurrent env access.
                unsafe { std::env::set_var(key.to_uppercase(), &val_str) };
            }
        }

        // Compile and run the script
        let source = std::fs::read_to_string(&tool.script_path)
            .map_err(|e| format!("cannot read tool script: {e}"))?;

        let tokens = Lexer::new(&source, 0)
            .tokenize()
            .map_err(|e| format!("tool lex error: {}", e.message))?;

        let program = Parser::new(tokens)
            .parse_program()
            .map_err(|e| format!("tool parse error: {}", e.message))?;

        let (compiled, _) = Compiler::compile_file(program, &tool.script_path)
            .map_err(|e| format!("tool compile error: {e:?}"))?;

        let mut vm = VM::new(compiled);
        vm.run().map_err(|e| format!("tool runtime error: {}", e.message))?;

        // Collect output
        let output = vm.get_output().join("");
        Ok(output)
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
}

/// Parse tool metadata from the first comment block in a .hml file.
/// Expected format:
/// ```
/// (* tool: read_file_tagged
///  * description: Read a file with hashline content-hash anchors
///  * param path: string - File path to read
///  * param offset: number - Start line (0 = beginning)
///  * param limit: number - Max lines (0 = all)
///  *)
/// ```
fn parse_tool_metadata(source: &str, default_name: &str) -> (String, serde_json::Value) {
    let mut description = format!("Tool: {default_name}");
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    // Find the first (* ... *) comment block
    if let Some(start) = source.find("(*") {
        if let Some(end) = source[start..].find("*)") {
            let comment = &source[start + 2..start + end];
            for line in comment.lines() {
                let line = line.trim().trim_start_matches('*').trim();
                if let Some(desc) = line.strip_prefix("description:") {
                    description = desc.trim().to_string();
                } else if let Some(param) = line.strip_prefix("param ") {
                    // Parse "param name: type - description"
                    if let Some((name_type, desc)) = param.split_once('-') {
                        let name_type = name_type.trim();
                        let desc = desc.trim();
                        if let Some((name, typ)) = name_type.split_once(':') {
                            let name = name.trim();
                            let typ = typ.trim();
                            let json_type = match typ {
                                "number" | "int" | "integer" => "integer",
                                "bool" | "boolean" => "boolean",
                                _ => "string",
                            };
                            properties.insert(
                                name.to_string(),
                                json!({
                                    "type": json_type,
                                    "description": desc,
                                }),
                            );
                            required.push(serde_json::Value::String(name.to_string()));
                        }
                    }
                }
            }
        }
    }

    let params = json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });

    (description, params)
}
