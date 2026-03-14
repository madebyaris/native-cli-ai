use nca_common::config::{AnthropicConfig, NcaConfig};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use reqwest::header::{HeaderMap, HeaderValue};

use super::anthropic_compat::{
    anthropic_request_body, map_provider_error, spawn_anthropic_stream,
};
use super::{Provider, ProviderError, StreamChunk};

pub struct AnthropicProvider {
    client: reqwest::Client,
    config: AnthropicConfig,
    max_tokens: u32,
}

impl AnthropicProvider {
    pub fn from_config(config: &NcaConfig) -> Result<Self, ProviderError> {
        let anthropic = config.provider.anthropic.clone();
        let api_key = anthropic.resolve_api_key().ok_or_else(|| {
            ProviderError::Configuration(format!(
                "missing Anthropic API key; set {} or provide `provider.anthropic.api_key` in config",
                anthropic.api_key_env
            ))
        })?;

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&api_key).map_err(|err| {
                ProviderError::Configuration(format!(
                    "failed to build Anthropic x-api-key header: {err}"
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
            config: anthropic,
            max_tokens: config.model.max_tokens,
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'))
    }
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
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
            self.config.temperature,
        );

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

        Ok(spawn_anthropic_stream(response, "anthropic"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::test_support::{collect_chunks, spawn_sse_server};
    use nca_common::message::Message;
    use nca_common::tool::ToolDefinition;
    use serde_json::json;

    #[tokio::test]
    async fn anthropic_provider_streams_text_tool_and_usage() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"message\":{\"usage\":{\"input_tokens\":13}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello from Claude\"}}\n\n",
            "event: content_block_start\n",
            "data: {\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"lookup\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"src\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {}\n\n",
            "event: message_delta\n",
            "data: {\"usage\":{\"output_tokens\":5}}\n\n"
        )
        .to_string();
        let base_url = spawn_sse_server(body, 200, |request| {
            assert_eq!(request.url(), "/v1/messages");
            assert!(
                request
                    .headers()
                    .iter()
                    .any(|header| header.field.equiv("x-api-key") && header.value.as_str() == "anthropic-test-key")
            );
            assert!(
                request
                    .headers()
                    .iter()
                    .any(|header| header.field.equiv("anthropic-version"))
            );
        });

        let mut config = NcaConfig::default();
        config.provider.anthropic.api_key = Some("anthropic-test-key".into());
        config.provider.anthropic.base_url = base_url;

        let provider = AnthropicProvider::from_config(&config).expect("provider");
        let stream = provider
            .chat(
                &[Message::system("be helpful"), Message::user("hello")],
                &[ToolDefinition {
                    name: "lookup".into(),
                    description: "Lookup a path".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"}
                        }
                    }),
                }],
                "",
            )
            .await
            .expect("chat stream");

        let chunks = collect_chunks(stream).await;
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Hello from Claude"));
        assert!(matches!(&chunks[1], StreamChunk::ToolUse(call) if call.id == "toolu_1" && call.name == "lookup" && call.input == json!({"path":"src"})));
        assert!(matches!(&chunks[2], StreamChunk::Usage { input_tokens: 13, output_tokens: 5 }));
        assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
    }
}
