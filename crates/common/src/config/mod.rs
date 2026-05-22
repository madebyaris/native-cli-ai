use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod env;
mod hooks;
mod io;
mod mcp;
mod memory;
mod model;
mod paths;
mod permissions;
mod provider;
mod ui;
mod web;
mod workspace;

pub use hooks::{HookCommand, HookConfig};
pub use mcp::{McpConfig, McpServerConfig};
pub use memory::{ContextConfig, MemoryConfig};
pub use model::{ModelConfig, ModelPricing, ModelRetryConfig};
pub use paths::{
    WorkspaceCacheError, global_config_path, nca_home_dir, workspace_cache_dir, workspace_cache_id,
    workspace_cli_index_path, workspace_config_path,
};
pub use permissions::{PermissionConfig, PermissionMode};
pub use provider::{
    AnthropicConfig, CustomProviderConfig, MiniMaxConfig, OpenAiConfig, OpenRouterConfig,
    ProviderCompatibility, ProviderConfig, ProviderKind,
};
pub use ui::UiConfig;
pub use web::WebConfig;
pub use workspace::{HarnessConfig, SessionConfig};

use io::{PartialNcaConfig, load_partial, save_config_to_path};

/// Top-level configuration, merged from global, workspace, env, and CLI sources.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NcaConfig {
    /// LLM provider credentials and defaults.
    pub provider: ProviderConfig,
    /// Model selection, pricing, and retry policy.
    pub model: ModelConfig,
    /// Tool permission tiers (allowed / ask / denied).
    pub permissions: PermissionConfig,
    /// Session persistence paths and turn limits.
    pub session: SessionConfig,
    /// Harness prompt, skill directories, and instruction paths.
    pub harness: HarnessConfig,
    /// MCP server definitions.
    pub mcp: McpConfig,
    /// Memory store and context-window settings.
    pub memory: MemoryConfig,
    /// Lifecycle hook commands.
    pub hooks: HookConfig,
    /// Web search/fetch tool settings.
    pub web: WebConfig,
    /// CLI/TUI preferences (e.g. external editor).
    #[serde(default)]
    pub ui: UiConfig,
}

impl NcaConfig {
    /// Load config from defaults, global file, workspace file, and environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the current directory cannot be read or a config
    /// file cannot be loaded.
    pub fn load() -> Result<Self, ConfigError> {
        let workspace_root = std::env::current_dir().map_err(|source| ConfigError::Io {
            action: "read current directory",
            path: PathBuf::from("."),
            source,
        })?;
        Self::load_for_workspace(&workspace_root)
    }

    /// Load config for an explicit workspace root.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if a global or workspace config file exists but
    /// cannot be read or parsed.
    pub fn load_for_workspace(workspace_root: &Path) -> Result<Self, ConfigError> {
        let mut config = Self::default();

        if let Some(path) = global_config_path()
            && path.exists()
        {
            let partial = load_partial(&path)?;
            config.merge(partial);
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
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the global config exists but cannot be read or
    /// parsed.
    pub fn load_global_file() -> Result<Self, ConfigError> {
        let mut config = Self::default();
        if let Some(path) = global_config_path()
            && path.exists()
        {
            let partial = load_partial(&path)?;
            config.merge(partial);
        }
        Ok(config)
    }

    /// Load only the persisted workspace-local config layered over defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the workspace config exists but cannot be read
    /// or parsed.
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
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::NoHomeDir`] when `HOME` is unset, or a filesystem /
    /// serialization error while writing the file.
    pub fn save_global(&self) -> Result<(), ConfigError> {
        let path = global_config_path().ok_or(ConfigError::NoHomeDir)?;
        save_config_to_path(self, &path)
    }

    /// Save the full config as the workspace-local override file.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the config cannot be serialized or written.
    pub fn save_workspace_file(&self, workspace_root: &Path) -> Result<(), ConfigError> {
        let path = workspace_config_path(workspace_root);
        save_config_to_path(self, &path)
    }

    /// Remove the workspace-local config file, if present.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the config file exists but cannot be removed.
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
        if let Some(ui) = partial.ui {
            self.ui.merge(ui);
        }

        if explicit_model_override {
            self.provider
                .set_model_for_default(self.model.default_model.clone());
        }

        if provider_changed || explicit_model_override {
            self.sync_default_model_from_provider();
        }
    }

    pub fn apply_model_override(&mut self, raw_model: &str) {
        let resolved = self.model.resolve_alias(raw_model);
        self.provider.set_model_for_default(resolved);
        self.sync_default_model_from_provider();
    }

    /// Switch the default LLM provider and keep `default_model` aligned with that provider's model field.
    pub fn set_default_provider(&mut self, provider: ProviderKind) {
        self.provider.default = provider;
        self.sync_default_model_from_provider();
    }

    /// Set the API key stored in config for a provider (workspace save may persist it).
    pub fn set_provider_api_key(&mut self, provider: ProviderKind, key: impl Into<String>) {
        let key = key.into();
        match provider {
            ProviderKind::MiniMax => self.provider.minimax.api_key = Some(key),
            ProviderKind::OpenAi => self.provider.openai.api_key = Some(key),
            ProviderKind::Anthropic => self.provider.anthropic.api_key = Some(key),
            ProviderKind::OpenRouter => self.provider.openrouter.api_key = Some(key),
            ProviderKind::Custom => self.provider.custom.api_key = Some(key),
        }
    }

    pub fn set_provider_base_url(&mut self, provider: ProviderKind, base_url: impl Into<String>) {
        let base_url = base_url.into();
        match provider {
            ProviderKind::MiniMax => self.provider.minimax.base_url = base_url,
            ProviderKind::OpenAi => self.provider.openai.base_url = base_url,
            ProviderKind::Anthropic => self.provider.anthropic.base_url = base_url,
            ProviderKind::OpenRouter => self.provider.openrouter.base_url = base_url,
            ProviderKind::Custom => self.provider.custom.base_url = base_url,
        }
    }

    pub fn set_custom_compatibility(&mut self, compatibility: ProviderCompatibility) {
        self.provider.custom.compatibility = compatibility;
    }

    /// Editor command: `NCA_EDITOR`, then `[ui].editor`, then `EDITOR`, then `vim`.
    #[must_use]
    pub fn effective_editor_command(&self) -> String {
        if let Ok(v) = std::env::var("NCA_EDITOR") {
            let t = v.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        if let Some(ref e) = self.ui.editor {
            let t = e.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        std::env::var("EDITOR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "vim".to_string())
    }

    pub fn sync_default_model_from_provider(&mut self) {
        self.model.default_model = self.provider.active_model().to_string();
    }

    /// Returns `true` if the first-run onboarding gate should be shown.
    /// Triggers when: onboarding not completed OR all API keys have been removed.
    #[must_use]
    pub fn needs_onboarding(&self) -> bool {
        !self.ui.onboarding_completed || !self.provider.any_api_key_present()
    }
}

/// Errors while loading, parsing, or saving configuration files.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// `$HOME` could not be resolved for the global config path.
    #[error("unable to determine the home directory for global config")]
    NoHomeDir,
    /// A config file could not be read from disk.
    #[error("failed to read config file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    /// TOML deserialization failed.
    #[error("failed to parse config file {path}: {source}")]
    ParseToml {
        path: PathBuf,
        source: toml::de::Error,
    },
    /// TOML serialization failed.
    #[error("failed to serialize config file {path}: {source}")]
    SerializeToml {
        path: PathBuf,
        source: toml::ser::Error,
    },
    /// Generic filesystem error with context.
    #[error("failed to {action} at {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
}

pub(super) fn default_true() -> bool {
    true
}

pub(super) fn default_max_memory_notes() -> usize {
    128
}

pub(super) fn resolve_api_key_value(inline: Option<&str>, env_name: &str) -> Option<String> {
    inline
        .filter(|v| !v.trim().is_empty())
        .map(String::from)
        .or_else(|| std::env::var(env_name).ok())
        .filter(|v| !v.trim().is_empty())
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
    fn workspace_cache_id_stable_for_same_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (id1, p1) = workspace_cache_id(dir.path()).expect("id");
        let (id2, p2) = workspace_cache_id(dir.path()).expect("id");
        assert_eq!(id1, id2);
        assert_eq!(p1, p2);
        assert!(id1.contains('-'));
        assert!(id1.len() > 16);
    }

    #[test]
    fn ui_editor_respects_nca_editor_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = NcaConfig::default();
        config.ui.editor = Some("vim".into());
        config.save_workspace_file(dir.path()).expect("save");

        unsafe { std::env::set_var("NCA_EDITOR", "nano") };
        let loaded = NcaConfig::load_for_workspace(dir.path()).expect("load");
        assert_eq!(loaded.effective_editor_command(), "nano");
        unsafe { std::env::remove_var("NCA_EDITOR") };
    }

    #[test]
    fn provider_kind_from_cli_name() {
        assert_eq!(
            ProviderKind::from_cli_name("MINIMAX"),
            Some(ProviderKind::MiniMax)
        );
        assert_eq!(
            ProviderKind::from_cli_name("openai"),
            Some(ProviderKind::OpenAi)
        );
        assert_eq!(
            ProviderKind::from_cli_name("custom"),
            Some(ProviderKind::Custom)
        );
        assert_eq!(ProviderKind::from_cli_name("nope"), None);
    }

    #[test]
    fn onboarding_completed_merges_from_partial() {
        let mut config = NcaConfig::default();
        let toml_str = r"
[ui]
onboarding_completed = true
";
        let partial: PartialNcaConfig = toml::from_str(toml_str).unwrap();
        config.merge(partial);
        assert!(config.ui.onboarding_completed);
    }

    #[test]
    fn any_api_key_present_returns_false_when_no_keys() {
        let config = config_without_env_keys();
        assert!(!config.provider.any_api_key_present());
    }

    #[test]
    fn any_api_key_present_returns_true_when_one_key_set() {
        let mut config = NcaConfig::default();
        config.provider.openai.api_key = Some("sk-test".into());
        assert!(config.provider.any_api_key_present());
    }

    fn config_without_env_keys() -> NcaConfig {
        let mut config = NcaConfig::default();
        config.provider.minimax.api_key_env = "__NCA_TEST_NONE__".into();
        config.provider.openai.api_key_env = "__NCA_TEST_NONE__".into();
        config.provider.anthropic.api_key_env = "__NCA_TEST_NONE__".into();
        config.provider.openrouter.api_key_env = "__NCA_TEST_NONE__".into();
        config.provider.custom.api_key_env = "__NCA_TEST_NONE__".into();
        config
    }

    #[test]
    fn needs_onboarding_true_when_no_flag_and_no_keys() {
        let config = config_without_env_keys();
        assert!(config.needs_onboarding());
    }

    #[test]
    fn needs_onboarding_false_when_flag_set_and_key_present() {
        let mut config = NcaConfig::default();
        config.ui.onboarding_completed = true;
        config.provider.minimax.api_key = Some("test-key".into());
        assert!(!config.needs_onboarding());
    }

    #[test]
    fn needs_onboarding_true_when_flag_set_but_all_keys_removed() {
        let mut config = config_without_env_keys();
        config.ui.onboarding_completed = true;
        assert!(config.needs_onboarding());
    }

    #[test]
    fn needs_onboarding_true_when_key_present_but_flag_not_set() {
        let mut config = NcaConfig::default();
        config.provider.openai.api_key = Some("sk-test".into());
        assert!(config.needs_onboarding());
    }

    #[test]
    fn onboarding_roundtrip_through_toml() {
        let toml_str = r#"
[ui]
onboarding_completed = true

[provider.minimax]
api_key = "test-key"
"#;
        let partial: PartialNcaConfig = toml::from_str(toml_str).unwrap();
        let mut config = NcaConfig::default();
        config.merge(partial);
        assert!(!config.needs_onboarding());
    }

    #[test]
    fn onboarding_triggers_when_key_removed_after_completion() {
        let toml_str = r"
[ui]
onboarding_completed = true
";
        let partial: PartialNcaConfig = toml::from_str(toml_str).unwrap();
        let mut config = config_without_env_keys();
        config.merge(partial);
        assert!(config.needs_onboarding());
    }
}
