use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;

/// Top-level configuration, merged from global, workspace, env, and CLI sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NcaConfig {
    pub provider: ProviderConfig,
    pub model: ModelConfig,
    pub permissions: PermissionConfig,
    pub session: SessionConfig,
    pub harness: HarnessConfig,
}

impl Default for NcaConfig {
    fn default() -> Self {
        Self {
            provider: ProviderConfig::default(),
            model: ModelConfig::default(),
            permissions: PermissionConfig::default(),
            session: SessionConfig::default(),
            harness: HarnessConfig::default(),
        }
    }
}

impl NcaConfig {
    /// Load config from defaults, global file, workspace file, and environment.
    pub fn load() -> Result<Self, ConfigError> {
        let mut config = Self::default();

        if let Some(path) = global_config_path() {
            if path.exists() {
                let partial = load_partial(&path)?;
                config.merge(partial);
            }
        }

        let local_path = PathBuf::from(".nca/config.local.toml");
        if local_path.exists() {
            let partial = load_partial(&local_path)?;
            config.merge(partial);
        }

        config.apply_env();
        Ok(config)
    }

    fn merge(&mut self, partial: PartialNcaConfig) {
        if let Some(provider) = partial.provider {
            self.provider.merge(provider);
        }

        if let Some(model) = partial.model {
            self.model.merge(model);
        }

        if let Some(permissions) = partial.permissions {
            self.permissions.merge(permissions);
        }

        if let Some(session) = partial.session {
            self.session.merge(session);
        }
        if let Some(harness) = partial.harness {
            self.harness.merge(harness);
        }
    }

    fn apply_env(&mut self) {
        if let Ok(provider) = env::var("NCA_DEFAULT_PROVIDER") {
            self.provider.default = ProviderKind::from_env(&provider);
        }

        if let Ok(model) = env::var("NCA_MODEL") {
            self.model.default_model = model.clone();
            self.provider.minimax.model = model;
        }

        if let Ok(api_key) = env::var("MINIMAX_API_KEY") {
            self.provider.minimax.api_key = Some(api_key);
        }

        if let Ok(base_url) = env::var("MINIMAX_BASE_URL") {
            self.provider.minimax.base_url = base_url;
        }

        if let Ok(model) = env::var("MINIMAX_MODEL") {
            self.provider.minimax.model = model;
        }
    }
}

fn global_config_path() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".nca/config.toml"))
}

fn load_partial(path: &PathBuf) -> Result<PartialNcaConfig, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.clone(),
        source,
    })?;

    toml::from_str(&raw).map_err(|source| ConfigError::ParseToml {
        path: path.clone(),
        source,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    ParseToml {
        path: PathBuf,
        source: toml::de::Error,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub default: ProviderKind,
    pub minimax: MiniMaxConfig,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default: ProviderKind::MiniMax,
            minimax: MiniMaxConfig::default(),
        }
    }
}

impl ProviderConfig {
    fn merge(&mut self, partial: PartialProviderConfig) {
        if let Some(default) = partial.default {
            self.default = default;
        }

        if let Some(minimax) = partial.minimax {
            self.minimax.merge(minimax);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    MiniMax,
    OpenRouter,
    Anthropic,
    OpenAi,
}

impl ProviderKind {
    fn from_env(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "openrouter" => Self::OpenRouter,
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAi,
            _ => Self::MiniMax,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiniMaxConfig {
    pub api_key_env: String,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
}

impl Default for MiniMaxConfig {
    fn default() -> Self {
        Self {
            api_key_env: "MINIMAX_API_KEY".into(),
            api_key: None,
            base_url: "https://api.minimax.io".into(),
            model: "MiniMax-M2.5".into(),
            temperature: 0.7,
        }
    }
}

impl MiniMaxConfig {
    pub fn resolve_api_key(&self) -> Option<String> {
        self.api_key
            .clone()
            .or_else(|| env::var(&self.api_key_env).ok())
    }

    fn merge(&mut self, partial: PartialMiniMaxConfig) {
        if let Some(api_key_env) = partial.api_key_env {
            self.api_key_env = api_key_env;
        }
        if let Some(api_key) = partial.api_key {
            self.api_key = Some(api_key);
        }
        if let Some(base_url) = partial.base_url {
            self.base_url = base_url;
        }
        if let Some(model) = partial.model {
            self.model = model;
        }
        if let Some(temperature) = partial.temperature {
            self.temperature = temperature;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub default_model: String,
    pub max_tokens: u32,
    pub enable_thinking: bool,
    pub thinking_budget: u32,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            default_model: "MiniMax-M2.5".into(),
            max_tokens: 8192,
            enable_thinking: false,
            thinking_budget: 5120,
        }
    }
}

impl ModelConfig {
    fn merge(&mut self, partial: PartialModelConfig) {
        if let Some(default_model) = partial.default_model {
            self.default_model = default_model;
        }
        if let Some(max_tokens) = partial.max_tokens {
            self.max_tokens = max_tokens;
        }
        if let Some(enable_thinking) = partial.enable_thinking {
            self.enable_thinking = enable_thinking;
        }
        if let Some(thinking_budget) = partial.thinking_budget {
            self.thinking_budget = thinking_budget;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionConfig {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
}

impl PermissionConfig {
    fn merge(&mut self, partial: PartialPermissionConfig) {
        if let Some(allow) = partial.allow {
            self.allow = allow;
        }
        if let Some(deny) = partial.deny {
            self.deny = deny;
        }
        if let Some(ask) = partial.ask {
            self.ask = ask;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub history_dir: PathBuf,
    pub max_turns_per_run: u32,
    pub max_tool_calls_per_turn: u32,
    pub checkpoint_interval: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            history_dir: PathBuf::from(".nca/sessions"),
            max_turns_per_run: 16,
            max_tool_calls_per_turn: 200,
            checkpoint_interval: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessConfig {
    pub built_in_enabled: bool,
    pub project_instructions_path: PathBuf,
    pub local_instructions_path: PathBuf,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            built_in_enabled: true,
            project_instructions_path: PathBuf::from(".ncarc"),
            local_instructions_path: PathBuf::from(".nca/instructions.md"),
        }
    }
}

impl HarnessConfig {
    fn merge(&mut self, partial: PartialHarnessConfig) {
        if let Some(enabled) = partial.built_in_enabled {
            self.built_in_enabled = enabled;
        }
        if let Some(path) = partial.project_instructions_path {
            self.project_instructions_path = path;
        }
        if let Some(path) = partial.local_instructions_path {
            self.local_instructions_path = path;
        }
    }
}

impl SessionConfig {
    fn merge(&mut self, partial: PartialSessionConfig) {
        if let Some(history_dir) = partial.history_dir {
            self.history_dir = history_dir;
        }
        if let Some(max_turns_per_run) = partial.max_turns_per_run {
            self.max_turns_per_run = max_turns_per_run;
        }
        if let Some(max_tool_calls_per_turn) = partial.max_tool_calls_per_turn {
            self.max_tool_calls_per_turn = max_tool_calls_per_turn;
        }
        if let Some(checkpoint_interval) = partial.checkpoint_interval {
            self.checkpoint_interval = checkpoint_interval;
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialNcaConfig {
    provider: Option<PartialProviderConfig>,
    model: Option<PartialModelConfig>,
    permissions: Option<PartialPermissionConfig>,
    session: Option<PartialSessionConfig>,
    harness: Option<PartialHarnessConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialProviderConfig {
    default: Option<ProviderKind>,
    minimax: Option<PartialMiniMaxConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialMiniMaxConfig {
    api_key_env: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    temperature: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialModelConfig {
    default_model: Option<String>,
    max_tokens: Option<u32>,
    enable_thinking: Option<bool>,
    thinking_budget: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialPermissionConfig {
    allow: Option<Vec<String>>,
    deny: Option<Vec<String>>,
    ask: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialSessionConfig {
    history_dir: Option<PathBuf>,
    max_turns_per_run: Option<u32>,
    max_tool_calls_per_turn: Option<u32>,
    checkpoint_interval: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialHarnessConfig {
    built_in_enabled: Option<bool>,
    project_instructions_path: Option<PathBuf>,
    local_instructions_path: Option<PathBuf>,
}
