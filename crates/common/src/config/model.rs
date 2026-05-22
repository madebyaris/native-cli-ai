//! Model selection and alias configuration.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Backoff when the model returns an empty assistant message after streaming
/// (provider quirk). Surfaced in `nca models --verbose` and overridable via
/// `[model.retry]` in config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelRetryConfig {
    /// How many times to retry after an empty streamed assistant message when
    /// usage was reported (same semantics as the historical hard-coded cap).
    #[serde(default = "default_max_empty_response_retries")]
    pub max_empty_response_retries: u32,
    /// First backoff before retrying an empty response (ms); doubles each attempt
    /// until capped by `empty_response_backoff_max_ms`.
    #[serde(default = "default_empty_retry_initial_ms")]
    pub empty_response_backoff_initial_ms: u64,
    #[serde(default = "default_empty_retry_backoff_max_ms")]
    pub empty_response_backoff_max_ms: u64,
}

fn default_max_empty_response_retries() -> u32 {
    2
}

fn default_empty_retry_initial_ms() -> u64 {
    250
}

fn default_empty_retry_backoff_max_ms() -> u64 {
    3000
}

impl Default for ModelRetryConfig {
    fn default() -> Self {
        Self {
            max_empty_response_retries: default_max_empty_response_retries(),
            empty_response_backoff_initial_ms: default_empty_retry_initial_ms(),
            empty_response_backoff_max_ms: default_empty_retry_backoff_max_ms(),
        }
    }
}

/// Per-model pricing, in USD per 1M tokens. Used to compute
/// `AgentEvent::CostUpdated.estimated_cost_usd` and the `nca cost` dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModelPricing {
    #[serde(default)]
    pub input_per_million: f64,
    #[serde(default)]
    pub output_per_million: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub default_model: String,
    pub max_tokens: u32,
    pub enable_thinking: bool,
    pub thinking_budget: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, String>,
    /// Last N used model names for F2 cycling.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_models: Vec<String>,
    /// Per-model pricing table keyed by exact model id (post-alias).
    /// When absent, `fallback_pricing` is used.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pricing: BTreeMap<String, ModelPricing>,
    /// Pricing used when a model has no explicit entry in `pricing`.
    #[serde(default = "default_fallback_pricing")]
    pub fallback_pricing: ModelPricing,
    #[serde(default)]
    pub retry: ModelRetryConfig,
}

fn default_fallback_pricing() -> ModelPricing {
    ModelPricing {
        input_per_million: 3.0,
        output_per_million: 15.0,
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            default_model: "MiniMax-M2.5".into(),
            max_tokens: 8192,
            enable_thinking: false,
            thinking_budget: 5120,
            aliases: default_model_aliases(),
            recent_models: Vec::new(),
            pricing: default_model_pricing(),
            fallback_pricing: default_fallback_pricing(),
            retry: ModelRetryConfig::default(),
        }
    }
}

impl ModelConfig {
    /// Resolve the pricing for a given (post-alias) model id, falling back to
    /// the configured `fallback_pricing` if no exact entry exists.
    #[must_use]
    pub fn pricing_for(&self, model: &str) -> ModelPricing {
        self.pricing
            .get(model)
            .cloned()
            .unwrap_or_else(|| self.fallback_pricing.clone())
    }
}

impl ModelConfig {
    pub(super) fn merge(&mut self, partial: PartialModelConfig) {
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
        if let Some(recent_models) = partial.recent_models {
            self.recent_models = recent_models;
        }
        if let Some(pricing) = partial.pricing {
            for (k, v) in pricing {
                self.pricing.insert(k, v);
            }
        }
        if let Some(fallback) = partial.fallback_pricing {
            self.fallback_pricing = fallback;
        }
        if let Some(retry) = partial.retry {
            self.retry = retry;
        }
    }

    #[must_use]
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

    /// Push a model name to the front of the recent list, deduplicating and capping at 8.
    pub fn track_recent_model(&mut self, model: &str) {
        self.recent_models.retain(|m| m != model);
        self.recent_models.insert(0, model.to_string());
        self.recent_models.truncate(8);
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialModelConfig {
    pub(super) default_model: Option<String>,
    pub(super) max_tokens: Option<u32>,
    pub(super) enable_thinking: Option<bool>,
    pub(super) thinking_budget: Option<u32>,
    pub(super) aliases: Option<BTreeMap<String, String>>,
    pub(super) recent_models: Option<Vec<String>>,
    pub(super) pricing: Option<BTreeMap<String, ModelPricing>>,
    pub(super) fallback_pricing: Option<ModelPricing>,
    pub(super) retry: Option<ModelRetryConfig>,
}

fn default_model_pricing() -> BTreeMap<String, ModelPricing> {
    BTreeMap::from([
        (
            "MiniMax-M2.5".into(),
            ModelPricing {
                input_per_million: 0.3,
                output_per_million: 1.2,
            },
        ),
        (
            "gpt-4o".into(),
            ModelPricing {
                input_per_million: 2.5,
                output_per_million: 10.0,
            },
        ),
        (
            "gpt-4o-mini".into(),
            ModelPricing {
                input_per_million: 0.15,
                output_per_million: 0.6,
            },
        ),
        (
            "claude-3-7-sonnet-latest".into(),
            ModelPricing {
                input_per_million: 3.0,
                output_per_million: 15.0,
            },
        ),
        (
            "claude-3-5-sonnet-20240620".into(),
            ModelPricing {
                input_per_million: 3.0,
                output_per_million: 15.0,
            },
        ),
    ])
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
