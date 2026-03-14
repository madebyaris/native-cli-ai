use nca_common::config::{MiniMaxConfig, NcaConfig};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

use super::anthropic_compat::{
    anthropic_request_body, map_provider_error, spawn_anthropic_stream,
};
use super::{Provider, ProviderError, StreamChunk};

/// MiniMax provider using the Anthropic-compatible endpoint.
/// Endpoint: <base_url>/v1/messages
/// Auth: Authorization: Bearer <api_key>
///
/// The Anthropic format gives reliable streaming for reasoning models:
/// thinking blocks are separate from text/tool_use blocks, and tool use
/// is represented as typed content blocks rather than a parallel JSON field.
pub struct MiniMaxProvider {
    client: reqwest::Client,
    config: MiniMaxConfig,
    max_tokens: u32,
}

impl MiniMaxProvider {
    pub fn from_config(config: &NcaConfig) -> Result<Self, ProviderError> {
        let minimax = config.provider.minimax.clone();
        let api_key = minimax.resolve_api_key().ok_or_else(|| {
            ProviderError::Configuration(format!(
                "missing MiniMax API key; set {} or provide `provider.minimax.api_key` in config",
                minimax.api_key_env
            ))
        })?;

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                ProviderError::Configuration(format!(
                    "failed to build MiniMax authorization header: {err}"
                ))
            })?,
        );
        // Anthropic-compatible endpoint also accepts x-api-key
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&api_key).map_err(|err| {
                ProviderError::Configuration(format!(
                    "failed to build x-api-key header: {err}"
                ))
            })?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static("2023-06-01"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|err| {
                ProviderError::Configuration(format!("failed to build HTTP client: {err}"))
            })?;

        Ok(Self {
            client,
            config: minimax,
            max_tokens: config.model.max_tokens,
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/messages",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait::async_trait]
impl Provider for MiniMaxProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, ProviderError> {
        let model = if model.is_empty() {
            self.config.model.clone()
        } else {
            model.to_string()
        };

        let body = anthropic_request_body(
            messages,
            tools,
            &model,
            self.max_tokens,
            // Anthropic requires temperature=1 when extended thinking is active.
            // MiniMax-M2.5 is a reasoning model; using 1.0 avoids API errors.
            1.0,
        );

        if std::env::var("NCA_DEBUG_REQUEST").is_ok() {
            eprintln!(
                "[minimax:request] {}",
                serde_json::to_string_pretty(&body).unwrap_or_default()
            );
        }

        let response = self
            .client
            .post(self.endpoint())
            .json(&body)
            .send()
            .await
            .map_err(|err| ProviderError::RequestFailed(err.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(map_provider_error(status, body_text));
        }

        Ok(spawn_anthropic_stream(response, "minimax"))
    }
}
