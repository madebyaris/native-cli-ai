pub mod anthropic;
pub mod anthropic_compat;
pub mod factory;
pub mod minimax;
pub mod minimax_vlm;
pub mod openai;
pub mod openai_compat;
pub mod openrouter;
#[cfg(test)]
pub mod test_support;
pub mod validate;
pub mod zhipuai;

use std::path::Path;

use async_trait::async_trait;
use nca_common::message::Message;
use nca_common::tool::{ToolCall, ToolDefinition};

/// A streamed chunk from the provider.
#[derive(Debug, Clone)]
pub enum StreamChunk {
    TextDelta(String),
    ToolUse(ToolCall),
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Done,
}

/// Abstraction over LLM providers (Anthropic, OpenAI, Gemini, etc.).
#[async_trait]
pub trait Provider: Send + Sync {
    /// Rewrite conversation history before an HTTP request (e.g. MiniMax `coding_plan/vlm` for images).
    /// Default: no-op.
    async fn prepare_messages_for_request(
        &self,
        _messages: &mut Vec<Message>,
        _workspace_root: &Path,
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    /// Send a conversation and receive a streaming response.
    ///
    /// `workspace_root` is used to resolve on-disk image paths embedded in user messages.
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        workspace_root: &Path,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, ProviderError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider configuration error: {0}")]
    Configuration(String),
    #[error("API request failed: {0}")]
    RequestFailed(String),
    #[error("Authentication error: {0}")]
    AuthError(String),
    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    #[error("{0}")]
    Other(String),
}
