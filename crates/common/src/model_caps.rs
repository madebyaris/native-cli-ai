//! Heuristic vision / multimodal support by provider and model id.

use crate::config::ProviderKind;

/// Whether the active provider+model is treated as supporting **native** image inputs
/// in chat (not MCP OCR fallback).
pub fn model_accepts_native_images(kind: ProviderKind, model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    match kind {
        ProviderKind::MiniMax => {
            // MiniMax M-series on the Anthropic-compatible endpoint supports image blocks.
            !m.is_empty()
        }
        ProviderKind::Anthropic => {
            m.contains("claude-3")
                || m.contains("claude-4")
                || m.contains("claude-opus-4")
                || m.contains("claude-sonnet-4")
        }
        ProviderKind::OpenAi => {
            m.contains("gpt-4o")
                || m.contains("gpt-4-turbo")
                || m.contains("gpt-5")
                || m.contains("o1")
                || m.contains("o3")
                || m.contains("vision")
        }
        ProviderKind::OpenRouter => {
            m.contains("gpt-4o")
                || m.contains("gpt-4-turbo")
                || m.contains("gpt-5")
                || m.contains("claude-3")
                || m.contains("claude-4")
                || m.contains("gemini")
                || m.contains("vision")
                || m.contains("qwen-vl")
        }
        ProviderKind::Custom => {
            m.contains("gpt-4o")
                || m.contains("gpt-4-turbo")
                || m.contains("gpt-5")
                || m.contains("claude-3")
                || m.contains("claude-4")
                || m.contains("gemini")
                || m.contains("vision")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimax_default_is_vision() {
        assert!(model_accepts_native_images(
            ProviderKind::MiniMax,
            "MiniMax-M2.5"
        ));
    }

    #[test]
    fn gpt35_is_not_vision_openai() {
        assert!(!model_accepts_native_images(
            ProviderKind::OpenAi,
            "gpt-3.5-turbo"
        ));
    }
}
