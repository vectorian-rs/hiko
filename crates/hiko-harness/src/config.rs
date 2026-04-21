//! Configuration loaded from hiko-harness.toml.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub default: DefaultConfig,
    #[serde(default)]
    pub hiko: HikoConfig,
    #[serde(default)]
    pub providers: HashMap<String, Provider>,
    #[serde(default)]
    pub models: HashMap<String, ModelAlias>,
    #[serde(default)]
    pub roles: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultConfig {
    pub model: String,
    pub provider: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct HikoConfig {
    #[serde(default = "default_hiko_bin")]
    pub bin: String,
    #[serde(default = "default_hiko_manifest")]
    pub manifest: String,
    #[serde(default = "default_hiko_policy")]
    pub policy: String,
    #[serde(default = "default_hiko_strict")]
    pub strict: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiFormat {
    #[default]
    Openai,
    Anthropic,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub api_url: String,
    /// Direct API key value. Takes precedence over api_key_env.
    #[serde(default)]
    pub api_key: String,
    /// Environment variable name to read the API key from.
    #[serde(default)]
    pub api_key_env: String,
    /// API wire format: "openai" (default) or "anthropic".
    #[serde(default)]
    pub api_format: ApiFormat,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAlias {
    pub provider: String,
    pub id: String,
}

fn default_max_tokens() -> u32 {
    4096
}
fn default_max_turns() -> usize {
    50
}
fn default_hiko_bin() -> String {
    "hiko-cli".to_string()
}
fn default_hiko_manifest() -> String {
    "hiko.toml".to_string()
}
fn default_hiko_policy() -> String {
    "harness-tools".to_string()
}
fn default_hiko_strict() -> bool {
    true
}

impl Default for HikoConfig {
    fn default() -> Self {
        Self {
            bin: default_hiko_bin(),
            manifest: default_hiko_manifest(),
            policy: default_hiko_policy(),
            strict: default_hiko_strict(),
        }
    }
}
impl Config {
    /// Load config from a TOML file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read config '{}': {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("invalid config: {e}"))
    }

    /// Find config file: CLI arg > ./hiko-harness.toml > ~/.config/hiko/harness.toml
    pub fn find() -> Option<std::path::PathBuf> {
        let local = Path::new("hiko-harness.toml");
        if local.exists() {
            return Some(local.to_path_buf());
        }
        if let Ok(home) = std::env::var("HOME") {
            let global = Path::new(&home).join(".config/hiko/harness.toml");
            if global.exists() {
                return Some(global);
            }
        }
        None
    }

    /// Resolve a model name (alias or direct ID) to (api_url, api_key, model_id).
    pub fn resolve_model(&self, name: &str) -> Result<ResolvedModel, String> {
        // Check if it's a role name first
        let model_name = self.roles.get(name).map(|s| s.as_str()).unwrap_or(name);

        // Check if it's a model alias
        if let Some(alias) = self.models.get(model_name) {
            let provider = self
                .providers
                .get(&alias.provider)
                .ok_or_else(|| format!("provider '{}' not found in config", alias.provider))?;
            return ResolvedModel::from_provider(provider, alias.id.clone());
        }

        // Check if it's provider/model format
        if let Some((provider_name, model_id)) = name.split_once('/') {
            let provider = self
                .providers
                .get(provider_name)
                .ok_or_else(|| format!("provider '{provider_name}' not found in config"))?;
            return ResolvedModel::from_provider(provider, model_id.to_string());
        }

        // Fall back to default provider with the name as model ID
        let provider = self
            .providers
            .get(&self.default.provider)
            .ok_or_else(|| format!("default provider '{}' not found", self.default.provider))?;
        ResolvedModel::from_provider(provider, name.to_string())
    }
}

#[derive(Debug)]
pub struct ResolvedModel {
    pub api_url: String,
    pub api_key: String,
    pub model_id: String,
    pub api_format: ApiFormat,
}

impl ResolvedModel {
    fn from_provider(provider: &Provider, model_id: String) -> Result<Self, String> {
        let api_key = if !provider.api_key.is_empty() {
            provider.api_key.clone()
        } else if !provider.api_key_env.is_empty() {
            std::env::var(&provider.api_key_env)
                .map_err(|_| format!("environment variable '{}' not set", provider.api_key_env))?
        } else {
            String::new()
        };
        Ok(Self {
            api_url: provider.api_url.clone(),
            api_key,
            model_id,
            api_format: provider.api_format,
        })
    }
}
