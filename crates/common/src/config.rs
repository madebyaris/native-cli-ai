use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

/// Top-level configuration, merged from global, workspace, env, and CLI sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NcaConfig {
    pub provider: ProviderConfig,
    pub model: ModelConfig,
    pub permissions: PermissionConfig,
    pub session: SessionConfig,
    pub harness: HarnessConfig,
    pub mcp: McpConfig,
    pub memory: MemoryConfig,
    pub hooks: HookConfig,
    pub web: WebConfig,
}

impl Default for NcaConfig {
    fn default() -> Self {
        Self {
            provider: ProviderConfig::default(),
            model: ModelConfig::default(),
            permissions: PermissionConfig::default(),
            session: SessionConfig::default(),
            harness: HarnessConfig::default(),
            mcp: McpConfig::default(),
            memory: MemoryConfig::default(),
            hooks: HookConfig::default(),
            web: WebConfig::default(),
        }
    }
}

impl NcaConfig {
    /// Load config from defaults, global file, workspace file, and environment.
    pub fn load() -> Result<Self, ConfigError> {
        let workspace_root = env::current_dir().map_err(|source| ConfigError::Io {
            action: "read current directory",
            path: PathBuf::from("."),
            source,
        })?;
        Self::load_for_workspace(&workspace_root)
    }

    /// Load config for an explicit workspace root.
    pub fn load_for_workspace(workspace_root: &Path) -> Result<Self, ConfigError> {
        let mut config = Self::default();

        if let Some(path) = global_config_path() {
            if path.exists() {
                let partial = load_partial(&path)?;
                config.merge(partial);
            }
        }

        let local_path = workspace_config_path(workspace_root);
        if local_path.exists() {
            let partial = load_partial(&local_path)?;
            config.merge(partial);
        }

        config.apply_env();
        Ok(config)
    }

    /// Load only the persisted global config file layered over defaults.
    pub fn load_global_file() -> Result<Self, ConfigError> {
        let mut config = Self::default();
        if let Some(path) = global_config_path() {
            if path.exists() {
                let partial = load_partial(&path)?;
                config.merge(partial);
            }
        }
        Ok(config)
    }

    /// Load only the persisted workspace-local config layered over defaults.
    pub fn load_workspace_file(workspace_root: &Path) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        let local_path = workspace_config_path(workspace_root);
        if local_path.exists() {
            let partial = load_partial(&local_path)?;
            config.merge(partial);
        }
        Ok(config)
    }

    /// Save the full config as the user's global defaults.
    pub fn save_global(&self) -> Result<(), ConfigError> {
        let path = global_config_path().ok_or_else(|| ConfigError::NoHomeDir)?;
        save_config_to_path(self, &path)
    }

    /// Save the full config as the workspace-local override file.
    pub fn save_workspace_file(&self, workspace_root: &Path) -> Result<(), ConfigError> {
        let path = workspace_config_path(workspace_root);
        save_config_to_path(self, &path)
    }

    /// Remove the workspace-local config file, if present.
    pub fn clear_workspace_file(workspace_root: &Path) -> Result<(), ConfigError> {
        let path = workspace_config_path(workspace_root);
        if !path.exists() {
            return Ok(());
        }
        std::fs::remove_file(&path).map_err(|source| ConfigError::Io {
            action: "remove config file",
            path,
            source,
        })
    }

    fn merge(&mut self, partial: PartialNcaConfig) {
        let provider_changed = partial.provider.is_some();
        let explicit_model_override = partial
            .model
            .as_ref()
            .and_then(|model| model.default_model.as_ref())
            .is_some();
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
        if let Some(mcp) = partial.mcp {
            self.mcp.merge(mcp);
        }
        if let Some(memory) = partial.memory {
            self.memory.merge(memory);
        }
        if let Some(hooks) = partial.hooks {
            self.hooks.merge(hooks);
        }
        if let Some(web) = partial.web {
            self.web.merge(web);
        }

        if explicit_model_override {
            self.provider
                .set_model_for_default(self.model.default_model.clone());
        }

        if provider_changed || explicit_model_override {
            self.sync_default_model_from_provider();
        }
    }

    fn apply_env(&mut self) {
        if let Ok(provider) = env::var("NCA_DEFAULT_PROVIDER") {
            self.provider.default = ProviderKind::from_env(&provider);
            self.sync_default_model_from_provider();
        }

        if let Ok(model) = env::var("NCA_MODEL") {
            self.apply_model_override(&model);
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

        if let Ok(api_key) = env::var("OPENAI_API_KEY") {
            self.provider.openai.api_key = Some(api_key);
        }

        if let Ok(base_url) = env::var("OPENAI_BASE_URL") {
            self.provider.openai.base_url = base_url;
        }

        if let Ok(model) = env::var("OPENAI_MODEL") {
            self.provider.openai.model = model;
        }

        if let Ok(api_key) = env::var("ANTHROPIC_API_KEY") {
            self.provider.anthropic.api_key = Some(api_key);
        }

        if let Ok(base_url) = env::var("ANTHROPIC_BASE_URL") {
            self.provider.anthropic.base_url = base_url;
        }

        if let Ok(model) = env::var("ANTHROPIC_MODEL") {
            self.provider.anthropic.model = model;
        }

        if let Ok(api_key) = env::var("OPENROUTER_API_KEY") {
            self.provider.openrouter.api_key = Some(api_key);
        }

        if let Ok(base_url) = env::var("OPENROUTER_BASE_URL") {
            self.provider.openrouter.base_url = base_url;
        }

        if let Ok(model) = env::var("OPENROUTER_MODEL") {
            self.provider.openrouter.model = model;
        }

        if let Ok(site_url) = env::var("OPENROUTER_SITE_URL") {
            self.provider.openrouter.site_url = Some(site_url);
        }

        if let Ok(app_name) = env::var("OPENROUTER_APP_NAME") {
            self.provider.openrouter.app_name = Some(app_name);
        }

        if let Ok(memory_path) = env::var("NCA_MEMORY_PATH") {
            self.memory.file_path = PathBuf::from(memory_path);
        }

        if let Ok(timeout_secs) = env::var("NCA_WEB_TIMEOUT_SECS") {
            if let Ok(timeout_secs) = timeout_secs.parse() {
                self.web.timeout_secs = timeout_secs;
            }
        }

        if let Ok(max_fetch_chars) = env::var("NCA_WEB_MAX_FETCH_CHARS") {
            if let Ok(max_fetch_chars) = max_fetch_chars.parse() {
                self.web.max_fetch_chars = max_fetch_chars;
            }
        }

        self.sync_default_model_from_provider();
    }

    pub fn apply_model_override(&mut self, raw_model: &str) {
        let resolved = self.model.resolve_alias(raw_model);
        self.provider.set_model_for_default(resolved);
        self.sync_default_model_from_provider();
    }

    fn sync_default_model_from_provider(&mut self) {
        self.model.default_model = self.provider.active_model().to_string();
    }
}

pub fn global_config_path() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".nca/config.toml"))
}

pub fn workspace_config_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".nca").join("config.local.toml")
}

fn load_partial(path: &Path) -> Result<PartialNcaConfig, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;

    toml::from_str(&raw).map_err(|source| ConfigError::ParseToml {
        path: path.to_path_buf(),
        source,
    })
}

fn save_config_to_path(config: &NcaConfig, path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            action: "create config directory",
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let raw = toml::to_string_pretty(config).map_err(|source| ConfigError::SerializeToml {
        path: path.to_path_buf(),
        source,
    })?;

    std::fs::write(path, raw).map_err(|source| ConfigError::Io {
        action: "write config file",
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("unable to determine the home directory for global config")]
    NoHomeDir,
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
    #[error("failed to serialize config file {path}: {source}")]
    SerializeToml {
        path: PathBuf,
        source: toml::ser::Error,
    },
    #[error("failed to {action} at {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub default: ProviderKind,
    pub minimax: MiniMaxConfig,
    pub openai: OpenAiConfig,
    pub anthropic: AnthropicConfig,
    pub openrouter: OpenRouterConfig,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default: ProviderKind::MiniMax,
            minimax: MiniMaxConfig::default(),
            openai: OpenAiConfig::default(),
            anthropic: AnthropicConfig::default(),
            openrouter: OpenRouterConfig::default(),
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
        if let Some(openai) = partial.openai {
            self.openai.merge(openai);
        }
        if let Some(anthropic) = partial.anthropic {
            self.anthropic.merge(anthropic);
        }
        if let Some(openrouter) = partial.openrouter {
            self.openrouter.merge(openrouter);
        }
    }

    pub fn active_model(&self) -> &str {
        match self.default {
            ProviderKind::MiniMax => &self.minimax.model,
            ProviderKind::OpenRouter => &self.openrouter.model,
            ProviderKind::Anthropic => &self.anthropic.model,
            ProviderKind::OpenAi => &self.openai.model,
        }
    }

    pub fn set_model_for_default(&mut self, model: impl Into<String>) {
        self.set_model_for(self.default, model);
    }

    pub fn set_model_for(&mut self, provider: ProviderKind, model: impl Into<String>) {
        let model = model.into();
        match provider {
            ProviderKind::MiniMax => self.minimax.model = model,
            ProviderKind::OpenRouter => self.openrouter.model = model,
            ProviderKind::Anthropic => self.anthropic.model = model,
            ProviderKind::OpenAi => self.openai.model = model,
        }
    }

    pub fn model_for(&self, provider: ProviderKind) -> &str {
        match provider {
            ProviderKind::MiniMax => &self.minimax.model,
            ProviderKind::OpenRouter => &self.openrouter.model,
            ProviderKind::Anthropic => &self.anthropic.model,
            ProviderKind::OpenAi => &self.openai.model,
        }
    }

    pub fn base_url_for(&self, provider: ProviderKind) -> &str {
        match provider {
            ProviderKind::MiniMax => &self.minimax.base_url,
            ProviderKind::OpenRouter => &self.openrouter.base_url,
            ProviderKind::Anthropic => &self.anthropic.base_url,
            ProviderKind::OpenAi => &self.openai.base_url,
        }
    }

    pub fn api_key_env_for(&self, provider: ProviderKind) -> &str {
        match provider {
            ProviderKind::MiniMax => &self.minimax.api_key_env,
            ProviderKind::OpenRouter => &self.openrouter.api_key_env,
            ProviderKind::Anthropic => &self.anthropic.api_key_env,
            ProviderKind::OpenAi => &self.openai.api_key_env,
        }
    }

    pub fn api_key_present_for(&self, provider: ProviderKind) -> bool {
        match provider {
            ProviderKind::MiniMax => self.minimax.resolve_api_key().is_some(),
            ProviderKind::OpenRouter => self.openrouter.resolve_api_key().is_some(),
            ProviderKind::Anthropic => self.anthropic.resolve_api_key().is_some(),
            ProviderKind::OpenAi => self.openai.resolve_api_key().is_some(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    MiniMax,
    OpenRouter,
    Anthropic,
    OpenAi,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 4] = [
        ProviderKind::MiniMax,
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::OpenRouter,
    ];

    fn from_env(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "openrouter" => Self::OpenRouter,
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAi,
            _ => Self::MiniMax,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ProviderKind::MiniMax => "MiniMax",
            ProviderKind::OpenRouter => "OpenRouter",
            ProviderKind::Anthropic => "Anthropic",
            ProviderKind::OpenAi => "OpenAI",
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
            // Anthropic-compatible endpoint (recommended for agentic/coding use).
            // International: https://api.minimax.io/anthropic
            // China:         https://api.minimaxi.com/anthropic
            base_url: "https://api.minimax.io/anthropic".into(),
            model: "MiniMax-M2.5".into(),
            temperature: 0.7,
        }
    }
}

impl MiniMaxConfig {
    pub fn resolve_api_key(&self) -> Option<String> {
        resolve_api_key_value(&self.api_key, &self.api_key_env)
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
pub struct OpenAiConfig {
    pub api_key_env: String,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            api_key_env: "OPENAI_API_KEY".into(),
            api_key: None,
            base_url: "https://api.openai.com".into(),
            model: "gpt-4o-mini".into(),
            temperature: 0.7,
        }
    }
}

impl OpenAiConfig {
    pub fn resolve_api_key(&self) -> Option<String> {
        resolve_api_key_value(&self.api_key, &self.api_key_env)
    }

    fn merge(&mut self, partial: PartialOpenAiConfig) {
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
pub struct AnthropicConfig {
    pub api_key_env: String,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key_env: "ANTHROPIC_API_KEY".into(),
            api_key: None,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-3-7-sonnet-latest".into(),
            temperature: 1.0,
        }
    }
}

impl AnthropicConfig {
    pub fn resolve_api_key(&self) -> Option<String> {
        resolve_api_key_value(&self.api_key, &self.api_key_env)
    }

    fn merge(&mut self, partial: PartialAnthropicConfig) {
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
pub struct OpenRouterConfig {
    pub api_key_env: String,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            api_key_env: "OPENROUTER_API_KEY".into(),
            api_key: None,
            base_url: "https://openrouter.ai/api".into(),
            model: "openai/gpt-4o-mini".into(),
            temperature: 0.7,
            site_url: None,
            app_name: None,
        }
    }
}

impl OpenRouterConfig {
    pub fn resolve_api_key(&self) -> Option<String> {
        resolve_api_key_value(&self.api_key, &self.api_key_env)
    }

    fn merge(&mut self, partial: PartialOpenRouterConfig) {
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
        if let Some(site_url) = partial.site_url {
            self.site_url = Some(site_url);
        }
        if let Some(app_name) = partial.app_name {
            self.app_name = Some(app_name);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub default_model: String,
    pub max_tokens: u32,
    pub enable_thinking: bool,
    pub thinking_budget: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, String>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            default_model: "MiniMax-M2.5".into(),
            max_tokens: 8192,
            enable_thinking: false,
            thinking_budget: 5120,
            aliases: default_model_aliases(),
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
        if let Some(aliases) = partial.aliases {
            self.aliases = aliases;
        }
    }

    pub fn resolve_alias(&self, raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return self.default_model.clone();
        }

        let lowered = trimmed.to_ascii_lowercase();
        self.aliases
            .get(&lowered)
            .cloned()
            .unwrap_or_else(|| trimmed.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionConfig {
    pub mode: PermissionMode,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
}

impl PermissionConfig {
    fn merge(&mut self, partial: PartialPermissionConfig) {
        if let Some(mode) = partial.mode {
            self.mode = mode;
        }
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    Default,
    Plan,
    AcceptEdits,
    DontAsk,
    BypassPermissions,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Default
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
    pub skill_directories: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub expose_in_safe_mode: bool,
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub file_path: PathBuf,
    #[serde(default = "default_max_memory_notes")]
    pub max_notes: usize,
    #[serde(default)]
    pub auto_compact_on_finish: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookConfig {
    #[serde(default)]
    pub session_start: Vec<HookCommand>,
    #[serde(default)]
    pub session_end: Vec<HookCommand>,
    #[serde(default)]
    pub pre_tool_use: Vec<HookCommand>,
    #[serde(default)]
    pub post_tool_use: Vec<HookCommand>,
    #[serde(default)]
    pub post_tool_failure: Vec<HookCommand>,
    #[serde(default)]
    pub approval_requested: Vec<HookCommand>,
    #[serde(default)]
    pub subagent_start: Vec<HookCommand>,
    #[serde(default)]
    pub subagent_stop: Vec<HookCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCommand {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    #[serde(default)]
    pub blocking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    pub timeout_secs: u64,
    pub max_fetch_chars: usize,
    pub default_search_limit: usize,
    pub user_agent: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 15,
            max_fetch_chars: 25_000,
            default_search_limit: 5,
            user_agent: "nca/0.1 (+https://github.com/user/native-cli-ai)".into(),
        }
    }
}

impl WebConfig {
    fn merge(&mut self, partial: PartialWebConfig) {
        if let Some(timeout_secs) = partial.timeout_secs {
            self.timeout_secs = timeout_secs;
        }
        if let Some(max_fetch_chars) = partial.max_fetch_chars {
            self.max_fetch_chars = max_fetch_chars;
        }
        if let Some(default_search_limit) = partial.default_search_limit {
            self.default_search_limit = default_search_limit;
        }
        if let Some(user_agent) = partial.user_agent {
            self.user_agent = user_agent;
        }
    }
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            built_in_enabled: true,
            project_instructions_path: PathBuf::from(".ncarc"),
            local_instructions_path: PathBuf::from(".nca/instructions.md"),
            skill_directories: default_skill_directories(),
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
        if let Some(skill_directories) = partial.skill_directories {
            self.skill_directories = skill_directories;
        }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            expose_in_safe_mode: false,
            servers: Vec::new(),
        }
    }
}

impl McpConfig {
    fn merge(&mut self, partial: PartialMcpConfig) {
        if let Some(expose_in_safe_mode) = partial.expose_in_safe_mode {
            self.expose_in_safe_mode = expose_in_safe_mode;
        }
        if let Some(servers) = partial.servers {
            self.servers = servers;
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            file_path: PathBuf::from(".nca/memory.json"),
            max_notes: default_max_memory_notes(),
            auto_compact_on_finish: false,
        }
    }
}

impl MemoryConfig {
    fn merge(&mut self, partial: PartialMemoryConfig) {
        if let Some(file_path) = partial.file_path {
            self.file_path = file_path;
        }
        if let Some(max_notes) = partial.max_notes {
            self.max_notes = max_notes;
        }
        if let Some(auto_compact_on_finish) = partial.auto_compact_on_finish {
            self.auto_compact_on_finish = auto_compact_on_finish;
        }
    }
}

impl HookConfig {
    fn merge(&mut self, partial: PartialHookConfig) {
        if let Some(session_start) = partial.session_start {
            self.session_start = session_start;
        }
        if let Some(session_end) = partial.session_end {
            self.session_end = session_end;
        }
        if let Some(pre_tool_use) = partial.pre_tool_use {
            self.pre_tool_use = pre_tool_use;
        }
        if let Some(post_tool_use) = partial.post_tool_use {
            self.post_tool_use = post_tool_use;
        }
        if let Some(post_tool_failure) = partial.post_tool_failure {
            self.post_tool_failure = post_tool_failure;
        }
        if let Some(approval_requested) = partial.approval_requested {
            self.approval_requested = approval_requested;
        }
        if let Some(subagent_start) = partial.subagent_start {
            self.subagent_start = subagent_start;
        }
        if let Some(subagent_stop) = partial.subagent_stop {
            self.subagent_stop = subagent_stop;
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
    mcp: Option<PartialMcpConfig>,
    memory: Option<PartialMemoryConfig>,
    hooks: Option<PartialHookConfig>,
    web: Option<PartialWebConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialProviderConfig {
    default: Option<ProviderKind>,
    minimax: Option<PartialMiniMaxConfig>,
    openai: Option<PartialOpenAiConfig>,
    anthropic: Option<PartialAnthropicConfig>,
    openrouter: Option<PartialOpenRouterConfig>,
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
struct PartialOpenAiConfig {
    api_key_env: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    temperature: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialAnthropicConfig {
    api_key_env: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    temperature: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialOpenRouterConfig {
    api_key_env: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    temperature: Option<f32>,
    site_url: Option<String>,
    app_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialModelConfig {
    default_model: Option<String>,
    max_tokens: Option<u32>,
    enable_thinking: Option<bool>,
    thinking_budget: Option<u32>,
    aliases: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialPermissionConfig {
    mode: Option<PermissionMode>,
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
    skill_directories: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialMcpConfig {
    expose_in_safe_mode: Option<bool>,
    servers: Option<Vec<McpServerConfig>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialMemoryConfig {
    file_path: Option<PathBuf>,
    max_notes: Option<usize>,
    auto_compact_on_finish: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialHookConfig {
    session_start: Option<Vec<HookCommand>>,
    session_end: Option<Vec<HookCommand>>,
    pre_tool_use: Option<Vec<HookCommand>>,
    post_tool_use: Option<Vec<HookCommand>>,
    post_tool_failure: Option<Vec<HookCommand>>,
    approval_requested: Option<Vec<HookCommand>>,
    subagent_start: Option<Vec<HookCommand>>,
    subagent_stop: Option<Vec<HookCommand>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialWebConfig {
    timeout_secs: Option<u64>,
    max_fetch_chars: Option<usize>,
    default_search_limit: Option<usize>,
    user_agent: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_max_memory_notes() -> usize {
    128
}

fn default_model_aliases() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("default".into(), "MiniMax-M2.5".into()),
        ("minimax".into(), "MiniMax-M2.5".into()),
        ("m2.5".into(), "MiniMax-M2.5".into()),
        ("coding".into(), "MiniMax-M2.5".into()),
        ("reasoning".into(), "MiniMax-M2.5".into()),
        ("openai".into(), "gpt-4o-mini".into()),
        ("gpt4o".into(), "gpt-4o".into()),
        ("gpt4omini".into(), "gpt-4o-mini".into()),
        ("claude".into(), "claude-3-7-sonnet-latest".into()),
        ("claude-sonnet".into(), "claude-3-7-sonnet-latest".into()),
        ("openrouter".into(), "openai/gpt-4o-mini".into()),
    ])
}

fn resolve_api_key_value(inline: &Option<String>, env_name: &str) -> Option<String> {
    inline.clone().or_else(|| env::var(env_name).ok())
}

fn default_skill_directories() -> Vec<PathBuf> {
    vec![
        PathBuf::from(".nca/skills"),
        PathBuf::from(".claude/skills"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_model_override_updates_selected_provider_model() {
        let mut config = NcaConfig::default();
        config.provider.default = ProviderKind::OpenAi;
        config.sync_default_model_from_provider();

        config.apply_model_override("gpt4o");

        assert_eq!(config.provider.openai.model, "gpt-4o");
        assert_eq!(config.model.default_model, "gpt-4o");
        assert_eq!(config.provider.minimax.model, "MiniMax-M2.5");
    }

    #[test]
    fn apply_env_supports_openai_anthropic_and_openrouter() {
        let _guard = EnvGuard::set(&[
            ("NCA_DEFAULT_PROVIDER", Some("openrouter")),
            ("OPENAI_API_KEY", Some("openai-key")),
            ("OPENAI_MODEL", Some("gpt-4o")),
            ("ANTHROPIC_API_KEY", Some("anthropic-key")),
            ("ANTHROPIC_MODEL", Some("claude-3-7-sonnet-20250219")),
            ("OPENROUTER_API_KEY", Some("openrouter-key")),
            ("OPENROUTER_MODEL", Some("anthropic/claude-3.7-sonnet")),
            ("OPENROUTER_SITE_URL", Some("https://nca.test")),
            ("OPENROUTER_APP_NAME", Some("Native CLI AI")),
        ]);

        let mut config = NcaConfig::default();
        config.apply_env();

        assert_eq!(config.provider.default, ProviderKind::OpenRouter);
        assert_eq!(config.provider.openai.resolve_api_key().as_deref(), Some("openai-key"));
        assert_eq!(
            config.provider.anthropic.resolve_api_key().as_deref(),
            Some("anthropic-key")
        );
        assert_eq!(
            config.provider.openrouter.resolve_api_key().as_deref(),
            Some("openrouter-key")
        );
        assert_eq!(config.provider.openai.model, "gpt-4o");
        assert_eq!(
            config.provider.anthropic.model,
            "claude-3-7-sonnet-20250219"
        );
        assert_eq!(
            config.provider.openrouter.model,
            "anthropic/claude-3.7-sonnet"
        );
        assert_eq!(
            config.provider.openrouter.site_url.as_deref(),
            Some("https://nca.test")
        );
        assert_eq!(
            config.provider.openrouter.app_name.as_deref(),
            Some("Native CLI AI")
        );
        assert_eq!(config.model.default_model, "anthropic/claude-3.7-sonnet");
    }

    struct EnvGuard {
        previous: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn set(vars: &[(&str, Option<&str>)]) -> Self {
            let mut previous = Vec::new();
            for (key, value) in vars {
                previous.push((key.to_string(), env::var(key).ok()));
                match value {
                    Some(value) => unsafe { env::set_var(key, value) },
                    None => unsafe { env::remove_var(key) },
                }
            }
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.previous.drain(..) {
                match value {
                    Some(value) => unsafe { env::set_var(&key, value) },
                    None => unsafe { env::remove_var(&key) },
                }
            }
        }
    }
}
