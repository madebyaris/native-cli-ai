pub mod factory;
pub mod minimax;

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
    /// Send a conversation and receive a streaming response.
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
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
