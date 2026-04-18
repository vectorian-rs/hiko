//! hiko-harness: Agentic coding tool powered by hiko scripts.
//!
//! Usage:
//!   hiko-harness "Fix the bug in main.rs"
//!   hiko-harness --model claude-opus "Refactor this function"
//!   hiko-harness --model ollama/qwen3:32b "Explain this code"

mod agent;
mod config;
mod llm;
mod tools;

use std::ffi::OsString;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut model_override: Option<String> = None;
    let mut config_path_override: Option<String> = None;
    let mut tools_dir = String::from("tools");
    let mut system_prompt_path: Option<String> = None;
    let mut prompt_parts: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--model" | "-m" => {
                i += 1;
                model_override = Some(require_arg(&args, i, "--model"));
            }
            "--config" | "-c" => {
                i += 1;
                config_path_override = Some(require_arg(&args, i, "--config"));
            }
            "--tools" | "-t" => {
                i += 1;
                tools_dir = require_arg(&args, i, "--tools");
            }
            "--system" | "-s" => {
                i += 1;
                system_prompt_path = Some(require_arg(&args, i, "--system"));
            }
            "--help" | "-h" => {
                print_usage();
                return;
            }
            arg if arg.starts_with('-') => {
                eprintln!("Unknown option: {arg}");
                std::process::exit(1);
            }
            _ => {
                prompt_parts.push(args[i].clone());
            }
        }
        i += 1;
    }

    // Load config
    let config_path = config_path_override
        .map(std::path::PathBuf::from)
        .or_else(config::Config::find);

    let cfg = match config_path {
        Some(ref path) => {
            eprintln!("Config: {}", path.display());
            config::Config::load(path).unwrap_or_else(|e| {
                eprintln!("Config error: {e}");
                std::process::exit(1);
            })
        }
        None => {
            eprintln!("No config file found, using defaults.");
            eprintln!("Create hiko-harness.toml or set --config.");
            std::process::exit(1);
        }
    };

    // Resolve model
    let model_name = model_override.as_deref().unwrap_or(&cfg.default.model);

    let resolved = cfg.resolve_model(model_name).unwrap_or_else(|e| {
        eprintln!("Model error: {e}");
        std::process::exit(1);
    });

    eprintln!("Model: {} (via {})", resolved.model_id, model_name);

    // Get prompt
    let prompt = if prompt_parts.is_empty() {
        eprintln!("Enter your prompt (Ctrl+D to send):");
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).expect("cannot read stdin");
        buf
    } else {
        prompt_parts.join(" ")
    };

    if prompt.trim().is_empty() {
        eprintln!("No prompt provided.");
        std::process::exit(1);
    }

    // Load system prompt
    let system_prompt = match system_prompt_path {
        Some(path) => std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("Cannot read system prompt '{path}': {e}");
            std::process::exit(1);
        }),
        None => load_default_system_prompt(),
    };

    // Load tools
    let tools_path = Path::new(&tools_dir);
    let registry = if tools_path.exists() {
        let runner = resolve_tool_runner(&cfg);
        tools::ToolRegistry::load(tools_path, runner).unwrap_or_else(|e| {
            eprintln!("Cannot load tools from '{}': {e}", tools_dir);
            std::process::exit(1);
        })
    } else {
        eprintln!(
            "Warning: tools directory '{}' not found, running without tools.",
            tools_dir
        );
        tools::ToolRegistry::empty()
    };

    let client = llm::LlmClient::new(resolved.api_url, resolved.api_key);

    let agent_config = agent::AgentConfig {
        model: resolved.model_id,
        system_prompt,
        max_turns: cfg.default.max_turns,
        max_tokens: cfg.default.max_tokens,
    };

    let mut agent = agent::Agent::new(client, registry, agent_config);

    match agent.run(&prompt) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("\nAgent error: {e}");
            std::process::exit(1);
        }
    }
}

fn resolve_tool_runner(cfg: &config::Config) -> tools::ToolRunner {
    tools::ToolRunner {
        bin: resolve_hiko_bin(&cfg.hiko),
        manifest_path: resolve_hiko_manifest(&cfg.hiko),
        policy_name: resolve_hiko_policy(&cfg.hiko),
        strict: cfg.hiko.strict,
    }
}

fn resolve_hiko_bin(hiko: &config::HikoConfig) -> OsString {
    OsString::from(&hiko.bin)
}

fn resolve_hiko_manifest(hiko: &config::HikoConfig) -> std::path::PathBuf {
    std::path::PathBuf::from(&hiko.manifest)
}

fn resolve_hiko_policy(hiko: &config::HikoConfig) -> String {
    hiko.policy.clone()
}

fn require_arg(args: &[String], i: usize, flag: &str) -> String {
    args.get(i).cloned().unwrap_or_else(|| {
        eprintln!("Error: {flag} requires a value");
        std::process::exit(1);
    })
}

fn load_default_system_prompt() -> String {
    for path in &["SYSTEM.md", ".hiko/SYSTEM.md"] {
        if let Ok(content) = std::fs::read_to_string(path) {
            return content;
        }
    }

    "You are a coding assistant. You have access to tools for reading, \
     searching, and editing files. Use the tools to help the user with \
     their coding tasks. When reading files, use the read tool to get \
     hashline-tagged content for reliable editing."
        .to_string()
}

fn print_usage() {
    eprintln!(
        "hiko-harness: Agentic coding tool powered by hiko scripts

Usage:
  hiko-harness [options] <prompt>

Options:
  -m, --model <name>      Model name, alias, role, or provider/model
  -c, --config <file>     Config file (default: ./hiko-harness.toml)
  -t, --tools <dir>       Tools directory (default: ./tools)
  -s, --system <file>     System prompt file
  -h, --help              Show this help

Config file (hiko-harness.toml):
  [hiko]                  Configure the external hiko-cli tool runner
  [providers.<name>]      Define LLM providers with api_url and api_key_env
  [models.<alias>]        Map short names to provider + model ID
  [roles.<name>]          Assign models to roles (default, fast, reasoning)

Model resolution order:
  1. Role name (e.g. 'fast' -> config roles.fast)
  2. Model alias (e.g. 'claude-sonnet' -> config models.claude-sonnet)
  3. Provider/model (e.g. 'ollama/qwen3:32b' -> provider + model ID)
  4. Raw model ID with default provider

Examples:
  hiko-harness \"Fix the bug in src/main.rs\"
  hiko-harness -m claude-opus \"Refactor this function\"
  hiko-harness -m fast \"List all TODO comments\"
  hiko-harness -m ollama/qwen3:32b \"Explain this code\""
    );
}
