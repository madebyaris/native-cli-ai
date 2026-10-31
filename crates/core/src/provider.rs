pub mod alibaba;
pub mod anthropic;
pub mod anthropic_compat;
pub mod custom;
pub mod factory;
pub mod minimax;
pub mod minimax_vlm;
pub mod openai;
pub mod openai_compat;
pub mod openrouter;
#[cfg(test)]
pub mod test_support;
pub mod validate;

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use nca_common::message::Message;
use nca_common::tool::{ToolCall, ToolDefinition};

/// A streamed chunk from the provider.
#[derive(Debug, Clone)]
pub enum StreamChunk {
    TextDelta(String),
    /// Model reasoning text. Kept out of the transcript; replayed to providers that require it.
    ReasoningDelta(String),
    ToolUse(ToolCall),
    /// A tool call whose arguments were not valid JSON. Reported back to the model as a tool error.
    InvalidToolUse {
        call_id: String,
        name: String,
        raw_arguments: String,
        error: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// Why the model stopped, normalized to `stop`, `length`, or `tool_calls` where possible.
    Finish {
        reason: String,
    },
    /// The stream failed. Partial output must not be treated as a completed answer.
    Failed(String),
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

/// Backstops for stalled endpoints. Cancelling a turn works without them; these stop a dead
/// connection from holding the request open forever. The read limit is between bytes, not total,
/// so long streams are fine.
pub const PROVIDER_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
pub const PROVIDER_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// HTTP client builder shared by all providers, with connect and idle-read limits.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(PROVIDER_CONNECT_TIMEOUT)
        .read_timeout(PROVIDER_READ_TIMEOUT)
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
    /// Temporary failure worth retrying: 408, 5xx, connection errors, or timeouts.
    #[error("temporary provider failure: {0}")]
    Transient(String),
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    #[error("{0}")]
    Other(String),
}

impl ProviderError {
    /// Whether repeating the same request may succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Transient(_))
    }
}
