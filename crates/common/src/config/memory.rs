//! Memory store and context window configuration.

use super::{default_max_memory_notes, default_true};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub file_path: PathBuf,
    #[serde(default = "default_max_memory_notes")]
    pub max_notes: usize,
    #[serde(default)]
    pub auto_compact_on_finish: bool,
    /// Context management configuration.
    #[serde(default)]
    pub context: ContextConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Target context window size (approximate tokens).
    /// Set to 0 for auto-detection based on model, or specify a custom value.
    /// Auto-detection uses known model context windows.
    #[serde(default)]
    pub context_window_target: usize,
    /// Use model-specific context window detection.
    /// When true, ignores `context_window_target` and auto-detects from model name.
    #[serde(default = "default_true")]
    pub auto_detect_context_window: bool,
    /// When true with `auto_detect_context_window`, query the active provider's models API
    /// before falling back to built-in tables. `OpenRouter`'s catalog is public; `OpenAI` and
    /// Anthropic require configured API keys. Set `NCA_SKIP_CONTEXT_API=1` to disable at runtime.
    /// Catalog responses are cached in-process; override TTL with `NCA_CONTEXT_API_CACHE_TTL_SECS`.
    #[serde(default = "default_true")]
    pub query_provider_models_api: bool,
    /// Maximum messages to retain after compaction.
    #[serde(default = "default_max_retained_messages")]
    pub max_retained_messages: usize,
    /// Percentage of context window that triggers auto-summarize (0-100).
    #[serde(default = "default_summarize_threshold")]
    pub auto_summarize_threshold: u8,
    /// Enable automatic context summarization.
    #[serde(default = "default_true")]
    pub enable_auto_summarize: bool,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            context_window_target: 0, // 0 means auto-detect
            auto_detect_context_window: true,
            query_provider_models_api: true,
            max_retained_messages: default_max_retained_messages(),
            auto_summarize_threshold: default_summarize_threshold(),
            enable_auto_summarize: default_true(),
        }
    }
}

pub(super) fn default_summarize_threshold() -> u8 {
    75
}

pub(super) fn default_max_retained_messages() -> usize {
    50
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            file_path: PathBuf::from(".nca/memory.json"),
            max_notes: default_max_memory_notes(),
            auto_compact_on_finish: false,
            context: ContextConfig::default(),
        }
    }
}

impl MemoryConfig {
    pub(super) fn merge(&mut self, partial: PartialMemoryConfig) {
        if let Some(file_path) = partial.file_path {
            self.file_path = file_path;
        }
        if let Some(max_notes) = partial.max_notes {
            self.max_notes = max_notes;
        }
        if let Some(auto_compact_on_finish) = partial.auto_compact_on_finish {
            self.auto_compact_on_finish = auto_compact_on_finish;
        }
        if let Some(context) = partial.context {
            self.context.merge(context);
        }
    }
}

impl ContextConfig {
    pub(super) fn merge(&mut self, partial: PartialContextConfig) {
        if let Some(auto_detect) = partial.auto_detect_context_window {
            self.auto_detect_context_window = auto_detect;
        }
        if let Some(context_window_target) = partial.context_window_target {
            self.context_window_target = context_window_target;
        }
        if let Some(max_retained_messages) = partial.max_retained_messages {
            self.max_retained_messages = max_retained_messages;
        }
        if let Some(auto_summarize_threshold) = partial.auto_summarize_threshold {
            self.auto_summarize_threshold = auto_summarize_threshold;
        }
        if let Some(enable_auto_summarize) = partial.enable_auto_summarize {
            self.enable_auto_summarize = enable_auto_summarize;
        }
        if let Some(query_provider_models_api) = partial.query_provider_models_api {
            self.query_provider_models_api = query_provider_models_api;
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialMemoryConfig {
    pub(super) file_path: Option<PathBuf>,
    pub(super) max_notes: Option<usize>,
    pub(super) auto_compact_on_finish: Option<bool>,
    pub(super) context: Option<PartialContextConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub(super) struct PartialContextConfig {
    pub(super) context_window_target: Option<usize>,
    pub(super) auto_detect_context_window: Option<bool>,
    pub(super) query_provider_models_api: Option<bool>,
    pub(super) max_retained_messages: Option<usize>,
    pub(super) auto_summarize_threshold: Option<u8>,
    pub(super) enable_auto_summarize: Option<bool>,
}
