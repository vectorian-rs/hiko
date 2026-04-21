//! Tool registry: loads .hml tool scripts and runs them through hiko-cli.

use crate::llm::{FunctionDef, ToolDef};
use serde_json::json;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct ToolRunner {
    pub bin: OsString,
    pub manifest_path: PathBuf,
    pub policy_name: String,
    pub strict: bool,
}

/// A tool backed by a hiko script.
pub struct Tool {
    pub name: String,
    pub description: String,
    pub notes: String,
    pub parameters: serde_json::Value,
    pub script_path: PathBuf,
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Tool>,
    runner: ToolRunner,
}

impl ToolRegistry {
    /// Create an empty registry (no tools).
    pub fn empty() -> Self {
        Self {
            tools: HashMap::new(),
            runner: ToolRunner {
                bin: OsString::from("hiko-cli"),
                manifest_path: PathBuf::from("hiko.toml"),
                policy_name: "harness-tools".to_string(),
                strict: true,
            },
        }
    }

    /// Load all .hml files from a tools directory.
    /// Each file's name (without .hml) becomes the tool name.
    /// The first comment block is parsed for metadata.
    pub fn load(tools_dir: &Path, runner: ToolRunner) -> Result<Self, String> {
        if !runner.manifest_path.exists() {
            return Err(format!(
                "hiko project manifest not found: {}",
                runner.manifest_path.display()
            ));
        }

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

            let source = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;

            let meta = parse_tool_metadata(&source, &name);

            tools.insert(
                name.clone(),
                Tool {
                    name,
                    description: meta.description,
                    notes: meta.notes,
                    parameters: meta.parameters,
                    script_path: path,
                },
            );
        }

        Ok(Self { tools, runner })
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

    /// Generate a tool guide section for the system prompt.
    /// Each tool gets a heading with its description and notes.
    pub fn tool_guide(&self) -> String {
        let mut tools: Vec<&Tool> = self.tools.values().collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));

        let mut out = String::from("## Tools\n");
        for tool in tools {
            out.push_str(&format!("\n### {}\n{}\n", tool.name, tool.description));
            if !tool.notes.is_empty() {
                out.push_str(&format!("{}\n", tool.notes));
            }
        }
        out
    }

    /// Execute a tool by name with the given JSON arguments.
    /// Injects the JSON argument object as stdin for the script.
    pub fn execute(&self, name: &str, args_json: &str) -> Result<String, String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| format!("unknown tool: {name}"))?;

        // Parse arguments once so invalid payloads fail before the tool starts.
        let args: serde_json::Value =
            serde_json::from_str(args_json).map_err(|e| format!("invalid tool args: {e}"))?;
        let stdin_json =
            serde_json::to_string(&args).map_err(|e| format!("invalid tool args: {e}"))?;

        let mut command = Command::new(&self.runner.bin);
        command.arg("run");
        if self.runner.strict {
            command.arg("--strict");
        }
        command
            .arg("--config")
            .arg(&self.runner.manifest_path)
            .arg("--policy")
            .arg(&self.runner.policy_name)
            .arg(&tool.script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|e| {
            format!(
                "cannot launch hiko runner '{}': {e}",
                Path::new(&self.runner.bin).display()
            )
        })?;

        child
            .stdin
            .take()
            .ok_or_else(|| "runner stdin was not piped".to_string())?
            .write_all(stdin_json.as_bytes())
            .map_err(|e| format!("cannot send tool input to runner: {e}"))?;

        let output = child
            .wait_with_output()
            .map_err(|e| format!("cannot wait for hiko runner: {e}"))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !stderr.is_empty() {
                Err(stderr)
            } else if !stdout.is_empty() {
                Err(stdout)
            } else {
                Err(format!("tool exited with status {}", output.status))
            }
        }
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
/// Parsed metadata from a tool's `(* ... *)` comment block.
struct ToolMetadata {
    description: String,
    notes: String,
    parameters: serde_json::Value,
}

fn parse_tool_metadata(source: &str, default_name: &str) -> ToolMetadata {
    let mut description = format!("Tool: {default_name}");
    let mut notes_lines: Vec<String> = Vec::new();
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    let mut in_notes = false;

    // Find the first (* ... *) comment block
    #[allow(clippy::collapsible_if)]
    if let Some(start) = source.find("(*") {
        if let Some(end) = source[start..].find("*)") {
            let comment = &source[start + 2..start + end];
            for line in comment.lines() {
                let line = line.trim().trim_start_matches('*').trim();
                if let Some(desc) = line.strip_prefix("description:") {
                    in_notes = false;
                    description = desc.trim().to_string();
                } else if let Some(param) = line.strip_prefix("param ") {
                    in_notes = false;
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
                } else if line.strip_prefix("notes:").is_some() {
                    in_notes = true;
                    let rest = line.strip_prefix("notes:").unwrap().trim();
                    if !rest.is_empty() {
                        notes_lines.push(rest.to_string());
                    }
                } else if in_notes && !line.is_empty() {
                    notes_lines.push(line.to_string());
                } else if line.is_empty() && in_notes {
                    // blank line inside notes is preserved
                    notes_lines.push(String::new());
                }
            }
        }
    }

    let parameters = json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });

    ToolMetadata {
        description,
        notes: notes_lines.join(" ").trim().to_string(),
        parameters,
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolRegistry, ToolRunner};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_tools_dir(name: &str) -> PathBuf {
        let unique = format!(
            "hiko-harness-tools-{}-{}-{}",
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

    #[cfg(unix)]
    #[test]
    fn execute_passes_json_args_via_hiko_runner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_tools_dir("stdin");
        let script = dir.join("echo_path.hml");
        let runner = dir.join("fake-hiko-cli.sh");
        let runner_config = dir.join("tool.policy.toml");
        fs::write(
            &script,
            "(* tool: echo_path\n\
             * description: Echo a path from structured input\n\
             * param path: string - Path to echo\n\
             *)\n\
             val _ = ()\n",
        )
        .unwrap();
        fs::write(
            &runner,
            "#!/bin/sh\n\
             mode=\"$1\"\n\
             strict=\"$2\"\n\
             config_flag=\"$3\"\n\
             config_path=\"$4\"\n\
             policy_flag=\"$5\"\n\
             policy_name=\"$6\"\n\
             script_path=\"$7\"\n\
             input=$(cat)\n\
             printf 'mode=%s\\nstrict=%s\\nconfig_flag=%s\\nconfig=%s\\npolicy_flag=%s\\npolicy=%s\\nscript=%s\\ninput=%s\\n' \"$mode\" \"$strict\" \"$config_flag\" \"$config_path\" \"$policy_flag\" \"$policy_name\" \"$script_path\" \"$input\"\n",
        )
        .unwrap();
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            &runner_config,
            r#"
[project]
name = "test"

[defaults]
policy = "harness-tools"

[policies.harness-tools]
path = "policies/harness-tools.policy.toml"
"#,
        )
        .unwrap();

        let registry = ToolRegistry::load(
            &dir,
            ToolRunner {
                bin: runner.as_os_str().to_os_string(),
                manifest_path: runner_config.clone(),
                policy_name: "harness-tools".to_string(),
                strict: true,
            },
        )
        .unwrap();
        let output = registry
            .execute("echo_path", r#"{"path":"src/main.rs"}"#)
            .unwrap();

        assert!(output.contains("mode=run"));
        assert!(output.contains("strict=--strict"));
        assert!(output.contains(&format!("config={}", runner_config.display())));
        assert!(output.contains("policy_flag=--policy"));
        assert!(output.contains("policy=harness-tools"));
        assert!(output.contains(&format!("script={}", script.display())));
        assert!(output.contains(r#"input={"path":"src/main.rs"}"#));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_notes_from_tool_metadata() {
        use super::parse_tool_metadata;

        let source = "\
(* tool: read
 * description: Read a file with anchors
 * param path: string - File path
 *
 * notes: Returns lines as LINE:HASH\\tCONTENT.
 * Use offset and limit for large files.
 *)
val _ = ()
";
        let meta = parse_tool_metadata(source, "read");
        assert_eq!(meta.description, "Read a file with anchors");
        assert!(meta.notes.contains("LINE:HASH"));
        assert!(meta.notes.contains("offset and limit"));
    }

    #[test]
    fn tool_guide_includes_notes() {
        let dir = temp_tools_dir("guide");
        let runner_config = dir.join("hiko.toml");
        fs::write(
            &runner_config,
            "[project]\nname = \"test\"\n[defaults]\npolicy = \"p\"\n[policies.p]\npath = \"p.toml\"\n",
        )
        .unwrap();
        fs::write(
            dir.join("alpha.hml"),
            "(* tool: alpha\n * description: Alpha tool\n * notes: Alpha notes here.\n *)\nval _ = ()\n",
        )
        .unwrap();
        fs::write(
            dir.join("beta.hml"),
            "(* tool: beta\n * description: Beta tool\n *)\nval _ = ()\n",
        )
        .unwrap();

        let registry = ToolRegistry::load(
            &dir,
            ToolRunner {
                bin: "hiko-cli".into(),
                manifest_path: runner_config,
                policy_name: "p".to_string(),
                strict: true,
            },
        )
        .unwrap();

        let guide = registry.tool_guide();
        assert!(guide.contains("### alpha"));
        assert!(guide.contains("Alpha notes here."));
        assert!(guide.contains("### beta"));
        assert!(guide.contains("Beta tool"));

        fs::remove_dir_all(&dir).ok();
    }
}
