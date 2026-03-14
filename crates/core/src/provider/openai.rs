use nca_common::config::{NcaConfig, OpenAiConfig};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

use super::openai_compat::{map_provider_error, openai_request_body, spawn_openai_stream};
use super::{Provider, ProviderError, StreamChunk};

pub struct OpenAiProvider {
    client: reqwest::Client,
    config: OpenAiConfig,
    max_tokens: u32,
}

impl OpenAiProvider {
    pub fn from_config(config: &NcaConfig) -> Result<Self, ProviderError> {
        let openai = config.provider.openai.clone();
        let api_key = openai.resolve_api_key().ok_or_else(|| {
            ProviderError::Configuration(format!(
                "missing OpenAI API key; set {} or provide `provider.openai.api_key` in config",
                openai.api_key_env
            ))
        })?;

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                ProviderError::Configuration(format!(
                    "failed to build OpenAI authorization header: {err}"
                ))
            })?,
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|err| {
                ProviderError::Configuration(format!("failed to build HTTP client: {err}"))
            })?;

        Ok(Self {
            client,
            config: openai,
            max_tokens: config.model.max_tokens,
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
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

        let body =
            openai_request_body(messages, tools, &model, self.max_tokens, self.config.temperature);

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

        Ok(spawn_openai_stream(response, "openai"))
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
    async fn openai_provider_streams_text_tool_and_usage() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"path\\\":\\\"\"}}]},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"src\\\"}\"}}]},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let base_url = spawn_sse_server(body, 200, |request| {
            assert_eq!(request.url(), "/v1/chat/completions");
            let auth = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("authorization"))
                .expect("authorization header");
            assert_eq!(auth.value.as_str(), "Bearer openai-test-key");
        });

        let mut config = NcaConfig::default();
        config.provider.openai.api_key = Some("openai-test-key".into());
        config.provider.openai.base_url = base_url;

        let provider = OpenAiProvider::from_config(&config).expect("provider");
        let stream = provider
            .chat(
                &[Message::user("hello")],
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
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Hello "));
        assert!(matches!(&chunks[1], StreamChunk::ToolUse(call) if call.id == "call_1" && call.name == "lookup" && call.input == json!({"path":"src"})));
        assert!(matches!(&chunks[2], StreamChunk::Usage { input_tokens: 11, output_tokens: 7 }));
        assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
    }
}
