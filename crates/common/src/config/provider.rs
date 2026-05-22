//! Provider (LLM backend) configuration: `MiniMax`, `OpenAI`, `Anthropic`, `OpenRouter`, `Custom`.

use super::resolve_api_key_value;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub default: ProviderKind,
    pub minimax: MiniMaxConfig,
    pub openai: OpenAiConfig,
    pub anthropic: AnthropicConfig,
    pub openrouter: OpenRouterConfig,
    pub custom: CustomProviderConfig,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default: ProviderKind::MiniMax,
            minimax: MiniMaxConfig::default(),
            openai: OpenAiConfig::default(),
            anthropic: AnthropicConfig::default(),
            openrouter: OpenRouterConfig::default(),
            custom: CustomProviderConfig::default(),
        }
    }
}

impl ProviderConfig {
    pub(super) fn merge(&mut self, partial: PartialProviderConfig) {
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
        if let Some(custom) = partial.custom {
            self.custom.merge(custom);
        }
    }

    #[must_use]
    pub fn active_model(&self) -> &str {
        match self.default {
            ProviderKind::MiniMax => &self.minimax.model,
            ProviderKind::OpenRouter => &self.openrouter.model,
            ProviderKind::Anthropic => &self.anthropic.model,
            ProviderKind::OpenAi => &self.openai.model,
            ProviderKind::Custom => &self.custom.model,
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
            ProviderKind::Custom => self.custom.model = model,
        }
    }

    #[must_use]
    pub fn model_for(&self, provider: ProviderKind) -> &str {
        match provider {
            ProviderKind::MiniMax => &self.minimax.model,
            ProviderKind::OpenRouter => &self.openrouter.model,
            ProviderKind::Anthropic => &self.anthropic.model,
            ProviderKind::OpenAi => &self.openai.model,
            ProviderKind::Custom => &self.custom.model,
        }
    }

    #[must_use]
    pub fn base_url_for(&self, provider: ProviderKind) -> &str {
        match provider {
            ProviderKind::MiniMax => &self.minimax.base_url,
            ProviderKind::OpenRouter => &self.openrouter.base_url,
            ProviderKind::Anthropic => &self.anthropic.base_url,
            ProviderKind::OpenAi => &self.openai.base_url,
            ProviderKind::Custom => &self.custom.base_url,
        }
    }

    #[must_use]
    pub fn api_key_env_for(&self, provider: ProviderKind) -> &str {
        match provider {
            ProviderKind::MiniMax => &self.minimax.api_key_env,
            ProviderKind::OpenRouter => &self.openrouter.api_key_env,
            ProviderKind::Anthropic => &self.anthropic.api_key_env,
            ProviderKind::OpenAi => &self.openai.api_key_env,
            ProviderKind::Custom => &self.custom.api_key_env,
        }
    }

    #[must_use]
    pub fn api_key_present_for(&self, provider: ProviderKind) -> bool {
        match provider {
            ProviderKind::MiniMax => self.minimax.resolve_api_key().is_some(),
            ProviderKind::OpenRouter => self.openrouter.resolve_api_key().is_some(),
            ProviderKind::Anthropic => self.anthropic.resolve_api_key().is_some(),
            ProviderKind::OpenAi => self.openai.resolve_api_key().is_some(),
            ProviderKind::Custom => self.custom.resolve_api_key().is_some(),
        }
    }

    #[must_use]
    pub fn resolve_api_key_for(&self, provider: ProviderKind) -> Option<String> {
        match provider {
            ProviderKind::MiniMax => self.minimax.resolve_api_key(),
            ProviderKind::OpenRouter => self.openrouter.resolve_api_key(),
            ProviderKind::Anthropic => self.anthropic.resolve_api_key(),
            ProviderKind::OpenAi => self.openai.resolve_api_key(),
            ProviderKind::Custom => self.custom.resolve_api_key(),
        }
    }

    #[must_use]
    pub fn compatibility_for(&self, provider: ProviderKind) -> Option<ProviderCompatibility> {
        match provider {
            ProviderKind::Custom => Some(self.custom.compatibility),
            _ => None,
        }
    }

    /// Returns `true` if at least one provider has an API key configured
    /// (either in config or via environment variable).
    #[must_use]
    pub fn any_api_key_present(&self) -> bool {
        ProviderKind::ALL
            .iter()
            .any(|p| self.api_key_present_for(*p))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    MiniMax,
    OpenRouter,
    Anthropic,
    OpenAi,
    Custom,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 5] = [
        ProviderKind::MiniMax,
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::OpenRouter,
        ProviderKind::Custom,
    ];

    /// Parse user/CLI input (slash commands, TUI pickers).
    #[must_use]
    pub fn from_cli_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "minimax" | "mini-max" | "minimaxi" => Some(Self::MiniMax),
            "openai" | "open-ai" | "gpt" => Some(Self::OpenAi),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "openrouter" | "open-router" => Some(Self::OpenRouter),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    pub(super) fn from_env(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "openrouter" => Self::OpenRouter,
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAi,
            "custom" => Self::Custom,
            _ => Self::MiniMax,
        }
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderKind::MiniMax => "MiniMax",
            ProviderKind::OpenRouter => "OpenRouter",
            ProviderKind::Anthropic => "Anthropic",
            ProviderKind::OpenAi => "OpenAI",
            ProviderKind::Custom => "Custom",
        }
    }

    /// Match [`display_name`](Self::display_name) output (case-insensitive).
    #[must_use]
    pub fn parse_display_name(s: &str) -> Option<Self> {
        let t = s.trim();
        Self::ALL
            .into_iter()
            .find(|k| k.display_name().eq_ignore_ascii_case(t))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderCompatibility {
    OpenAi,
    Anthropic,
}

impl ProviderCompatibility {
    #[must_use]
    pub fn from_cli_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" | "open-ai" => Some(Self::OpenAi),
            "anthropic" | "claude" => Some(Self::Anthropic),
            _ => None,
        }
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI-compatible",
            Self::Anthropic => "Anthropic-compatible",
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
    #[must_use]
    pub fn resolve_api_key(&self) -> Option<String> {
        resolve_api_key_value(self.api_key.as_deref(), &self.api_key_env)
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
    #[must_use]
    pub fn resolve_api_key(&self) -> Option<String> {
        resolve_api_key_value(self.api_key.as_deref(), &self.api_key_env)
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
    #[must_use]
    pub fn resolve_api_key(&self) -> Option<String> {
        resolve_api_key_value(self.api_key.as_deref(), &self.api_key_env)
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
    #[must_use]
    pub fn resolve_api_key(&self) -> Option<String> {
        resolve_api_key_value(self.api_key.as_deref(), &self.api_key_env)
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
pub struct CustomProviderConfig {
    pub api_key_env: String,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub compatibility: ProviderCompatibility,
}

impl Default for CustomProviderConfig {
    fn default() -> Self {
        Self {
            api_key_env: "CUSTOM_PROVIDER_API_KEY".into(),
            api_key: None,
            base_url: String::new(),
            model: "custom-model".into(),
            temperature: 0.7,
            compatibility: ProviderCompatibility::OpenAi,
        }
    }
}

impl CustomProviderConfig {
    #[must_use]
    pub fn resolve_api_key(&self) -> Option<String> {
        resolve_api_key_value(self.api_key.as_deref(), &self.api_key_env)
    }

    fn merge(&mut self, partial: PartialCustomProviderConfig) {
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
        if let Some(compatibility) = partial.compatibility {
            self.compatibility = compatibility;
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialProviderConfig {
    pub(super) default: Option<ProviderKind>,
    pub(super) minimax: Option<PartialMiniMaxConfig>,
    pub(super) openai: Option<PartialOpenAiConfig>,
    pub(super) anthropic: Option<PartialAnthropicConfig>,
    pub(super) openrouter: Option<PartialOpenRouterConfig>,
    pub(super) custom: Option<PartialCustomProviderConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialMiniMaxConfig {
    pub(super) api_key_env: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) model: Option<String>,
    pub(super) temperature: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialOpenAiConfig {
    pub(super) api_key_env: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) model: Option<String>,
    pub(super) temperature: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialAnthropicConfig {
    pub(super) api_key_env: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) model: Option<String>,
    pub(super) temperature: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialOpenRouterConfig {
    pub(super) api_key_env: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) model: Option<String>,
    pub(super) temperature: Option<f32>,
    pub(super) site_url: Option<String>,
    pub(super) app_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialCustomProviderConfig {
    pub(super) api_key_env: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) model: Option<String>,
    pub(super) temperature: Option<f32>,
    pub(super) compatibility: Option<ProviderCompatibility>,
}
