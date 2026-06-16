//! Model-specific context window sizes and detection.
//!
//! Different LLM models have vastly different context limits.
//! This module provides detection and defaults for common models.
//!
//! For models routed via [OpenRouter](https://openrouter.ai/models), authoritative
//! per-model `context_length` values are published in the public API:
//! `GET https://openrouter.ai/api/v1/models` (JSON field `context_length` on each entry).

use serde::{Deserialize, Serialize};

/// Context window limits for various LLM models (in tokens).
/// These are approximate and may vary by API version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelContextLimits {
    /// Model identifier pattern (partial match)
    pub pattern: &'static str,
    /// Context window size in tokens
    pub context_window: usize,
    /// Recommended max tokens for the model
    pub max_output_tokens: usize,
}

/// Known model context windows.
/// Order matters - more specific patterns should come first.
pub const MODEL_CONTEXT_LIMITS: &[ModelContextLimits] = &[
    // Claude 3.7 family
    ModelContextLimits {
        pattern: "claude-3-7",
        context_window: 200_000,
        max_output_tokens: 8192,
    },
    // Claude 3.5 family
    ModelContextLimits {
        pattern: "claude-3-5",
        context_window: 200_000,
        max_output_tokens: 8192,
    },
    // Claude 3 Opus
    ModelContextLimits {
        pattern: "claude-3-opus",
        context_window: 200_000,
        max_output_tokens: 4096,
    },
    // Claude 3 Sonnet
    ModelContextLimits {
        pattern: "claude-3-sonnet",
        context_window: 200_000,
        max_output_tokens: 4096,
    },
    // Claude 3 Haiku
    ModelContextLimits {
        pattern: "claude-3-haiku",
        context_window: 200_000,
        max_output_tokens: 4096,
    },
    // GPT-4o family
    ModelContextLimits {
        pattern: "gpt-4o",
        context_window: 128_000,
        max_output_tokens: 16384,
    },
    // GPT-4.5 / GPT-4 Turbo
    ModelContextLimits {
        pattern: "gpt-4-turbo",
        context_window: 128_000,
        max_output_tokens: 4096,
    },
    // GPT-4
    ModelContextLimits {
        pattern: "gpt-4",
        context_window: 128_000,
        max_output_tokens: 4096,
    },
    // GPT-3.5 Turbo
    ModelContextLimits {
        pattern: "gpt-3.5-turbo",
        context_window: 16_385,
        max_output_tokens: 4096,
    },
    // MiniMax M2.7 — OpenRouter `minimax/minimax-m2.7`: context_length 204_800 (must be before `minimax-m2`)
    ModelContextLimits {
        pattern: "minimax-m2.7",
        context_window: 204_800,
        max_output_tokens: 131_072,
    },
    // OpenRouter slug (config may store full id)
    ModelContextLimits {
        pattern: "minimax/minimax-m2.7",
        context_window: 204_800,
        max_output_tokens: 131_072,
    },
    // MiniMax M2.5 (reasoning model)
    ModelContextLimits {
        pattern: "minimax-m2.5",
        context_window: 100_000,
        max_output_tokens: 8192,
    },
    // MiniMax M2 (not M2.5 / M2.7)
    ModelContextLimits {
        pattern: "minimax-m2",
        context_window: 32_000,
        max_output_tokens: 8192,
    },
    // MiniMax M1
    ModelContextLimits {
        pattern: "minimax-m1",
        context_window: 32_000,
        max_output_tokens: 8192,
    },
    // ZhipuAI GLM-5.2 (1M context, 128K output)
    ModelContextLimits {
        pattern: "glm-5.2",
        context_window: 1_000_000,
        max_output_tokens: 131_072,
    },
    // ZhipuAI GLM-5 Turbo (coding plan)
    ModelContextLimits {
        pattern: "glm-5-turbo",
        context_window: 128_000,
        max_output_tokens: 16_384,
    },
    // Gemini 1.5 Pro
    ModelContextLimits {
        pattern: "gemini-1.5-pro",
        context_window: 2_000_000,
        max_output_tokens: 8192,
    },
    // Gemini 1.5 Flash
    ModelContextLimits {
        pattern: "gemini-1.5-flash",
        context_window: 1_000_000,
        max_output_tokens: 8192,
    },
    // Gemini 1.5
    ModelContextLimits {
        pattern: "gemini-1.5",
        context_window: 1_000_000,
        max_output_tokens: 8192,
    },
    // Gemini 2.0 Flash
    ModelContextLimits {
        pattern: "gemini-2.0-flash",
        context_window: 1_000_000,
        max_output_tokens: 8192,
    },
    // DeepSeek V4 Pro (1M context, 384K max output)
    ModelContextLimits {
        pattern: "deepseek-v4-pro",
        context_window: 1_000_000,
        max_output_tokens: 393_216,
    },
    // DeepSeek V4 Flash (1M context, 384K max output)
    ModelContextLimits {
        pattern: "deepseek-v4-flash",
        context_window: 1_000_000,
        max_output_tokens: 393_216,
    },
    // DeepSeek V4 catch-all
    ModelContextLimits {
        pattern: "deepseek-v4",
        context_window: 1_000_000,
        max_output_tokens: 393_216,
    },
    // DeepSeek V3
    ModelContextLimits {
        pattern: "deepseek-v3",
        context_window: 64_000,
        max_output_tokens: 8192,
    },
    // DeepSeek R1
    ModelContextLimits {
        pattern: "deepseek-r1",
        context_window: 64_000,
        max_output_tokens: 8192,
    },
    // Qwen 2.5
    ModelContextLimits {
        pattern: "qwen-2.5",
        context_window: 128_000,
        max_output_tokens: 8192,
    },
    // Llama 3.1 405B
    ModelContextLimits {
        pattern: "llama-3.1-405b",
        context_window: 128_000,
        max_output_tokens: 4096,
    },
    // Llama 3.1 70B
    ModelContextLimits {
        pattern: "llama-3.1-70b",
        context_window: 128_000,
        max_output_tokens: 4096,
    },
    // Llama 3.1 family
    ModelContextLimits {
        pattern: "llama-3.1",
        context_window: 128_000,
        max_output_tokens: 4096,
    },
    // Llama 3 family
    ModelContextLimits {
        pattern: "llama-3",
        context_window: 8_192,
        max_output_tokens: 2048,
    },
    // Default for unknown models
    ModelContextLimits {
        pattern: "*",
        context_window: 32_000,
        max_output_tokens: 4096,
    },
];

/// Detect the context window size for a given model name.
pub fn detect_context_window(model: &str) -> usize {
    let model_lower = model.to_lowercase();

    for limit in MODEL_CONTEXT_LIMITS {
        if limit.pattern != "*" && model_lower.contains(&limit.pattern.to_lowercase()) {
            return limit.context_window;
        }
    }

    // Fallback
    32_000
}

/// Detect the max output tokens for a given model name.
pub fn detect_max_output_tokens(model: &str) -> usize {
    let model_lower = model.to_lowercase();

    for limit in MODEL_CONTEXT_LIMITS {
        if limit.pattern != "*" && model_lower.contains(&limit.pattern.to_lowercase()) {
            return limit.max_output_tokens;
        }
    }

    // Fallback
    4096
}

/// Get both context window and max output tokens for a model.
#[derive(Debug, Clone)]
pub struct ModelLimits {
    pub context_window: usize,
    pub max_output_tokens: usize,
}

impl ModelLimits {
    pub fn for_model(model: &str) -> Self {
        Self {
            context_window: detect_context_window(model),
            max_output_tokens: detect_max_output_tokens(model),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_claude() {
        assert_eq!(detect_context_window("claude-3-7-sonnet-latest"), 200_000);
        assert_eq!(detect_context_window("claude-3-5-sonnet-20241022"), 200_000);
        assert_eq!(detect_context_window("claude-3-opus-20240229"), 200_000);
    }

    #[test]
    fn test_detect_gpt() {
        assert_eq!(detect_context_window("gpt-4o-2024-08-06"), 128_000);
        assert_eq!(detect_context_window("gpt-4o-mini"), 128_000);
        assert_eq!(detect_context_window("gpt-4-turbo-2024-04-09"), 128_000);
    }

    #[test]
    fn test_detect_minimax() {
        assert_eq!(detect_context_window("MiniMax-M2.7"), 204_800);
        assert_eq!(detect_context_window("minimax/minimax-m2.7"), 204_800);
        assert_eq!(detect_context_window("MiniMax-M2.5"), 100_000);
        assert_eq!(detect_context_window("minimax-m2"), 32_000);
    }

    #[test]
    fn test_detect_gemini() {
        assert_eq!(detect_context_window("gemini-1.5-pro-latest"), 2_000_000);
        assert_eq!(detect_context_window("gemini-1.5-flash"), 1_000_000);
    }

    #[test]
    fn test_detect_deepseek_v4() {
        assert_eq!(detect_context_window("deepseek-v4-flash"), 1_000_000);
        assert_eq!(detect_context_window("deepseek-v4-pro"), 1_000_000);
        assert_eq!(detect_max_output_tokens("deepseek-v4-flash"), 393_216);
    }

    #[test]
    fn test_detect_deepseek_legacy() {
        assert_eq!(detect_context_window("deepseek-v3"), 64_000);
        assert_eq!(detect_context_window("deepseek-r1"), 64_000);
    }

    #[test]
    fn test_fallback() {
        assert_eq!(detect_context_window("unknown-model-xyz"), 32_000);
    }

    #[test]
    fn test_model_limits_struct() {
        let limits = ModelLimits::for_model("claude-3-7-sonnet");
        assert_eq!(limits.context_window, 200_000);
        assert_eq!(limits.max_output_tokens, 8192);
    }
}
