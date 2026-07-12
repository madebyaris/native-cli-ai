use std::path::Path;

use nca_common::config::{CustomProviderConfig, NcaConfig, ProviderCompatibility};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

use super::anthropic_compat::{
    anthropic_request_body, map_provider_error as map_anthropic_error, spawn_anthropic_stream,
};
use super::openai_compat::{
    map_provider_error as map_openai_error, openai_request_body, spawn_openai_stream,
};
use super::{Provider, ProviderError, StreamChunk};

pub struct CustomProvider {
    client: reqwest::Client,
    config: CustomProviderConfig,
    max_tokens: u32,
}

impl CustomProvider {
    pub fn from_config(config: &NcaConfig) -> Result<Self, ProviderError> {
        let custom = config.provider.custom.clone();
        let api_key = custom.resolve_api_key().ok_or_else(|| {
            ProviderError::Configuration(format!(
                "missing Custom provider API key; set {} or provide `provider.custom.api_key` in config",
                custom.api_key_env
            ))
        })?;
        if custom.base_url.trim().is_empty() {
            return Err(ProviderError::Configuration(
                "missing Custom provider base URL; set `provider.custom.base_url`".into(),
            ));
        }

        let mut headers = HeaderMap::new();
        match custom.compatibility {
            ProviderCompatibility::OpenAi => {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                        ProviderError::Configuration(format!(
                            "failed to build Custom provider authorization header: {err}"
                        ))
                    })?,
                );
            }
            ProviderCompatibility::Anthropic => {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                        ProviderError::Configuration(format!(
                            "failed to build Custom provider authorization header: {err}"
                        ))
                    })?,
                );
                headers.insert(
                    "x-api-key",
                    HeaderValue::from_str(&api_key).map_err(|err| {
                        ProviderError::Configuration(format!(
                            "failed to build Custom provider x-api-key header: {err}"
                        ))
                    })?,
                );
                headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
            }
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|err| {
                ProviderError::Configuration(format!("failed to build HTTP client: {err}"))
            })?;

        Ok(Self {
            client,
            config: custom,
            max_tokens: config.model.max_tokens,
        })
    }

    fn endpoint(&self) -> String {
        match self.config.compatibility {
            ProviderCompatibility::OpenAi => format!(
                "{}/v1/chat/completions",
                self.config.base_url.trim_end_matches('/')
            ),
            ProviderCompatibility::Anthropic => {
                format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'))
            }
        }
    }
}

#[async_trait::async_trait]
impl Provider for CustomProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        workspace_root: &Path,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, ProviderError> {
        let model = if model.is_empty() {
            self.config.model.clone()
        } else {
            model.to_string()
        };

        match self.config.compatibility {
            ProviderCompatibility::OpenAi => {
                let body = openai_request_body(
                    messages,
                    tools,
                    &model,
                    self.max_tokens,
                    self.config.temperature,
                    workspace_root,
                )?;

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
                    return Err(map_openai_error(status, body_text));
                }

                Ok(spawn_openai_stream(response, "custom"))
            }
            ProviderCompatibility::Anthropic => {
                let body = anthropic_request_body(
                    messages,
                    tools,
                    &model,
                    self.max_tokens,
                    self.config.temperature,
                    workspace_root,
                )?;

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
                    return Err(map_anthropic_error(status, body_text));
                }

                Ok(spawn_anthropic_stream(response, "custom"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::test_support::{collect_chunks, spawn_sse_server};
    use serde_json::json;

    #[tokio::test]
    async fn custom_openai_compatible_provider_streams() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let base_url = spawn_sse_server(body, 200, |request| {
            assert_eq!(request.url(), "/v1/chat/completions");
        });

        let mut config = NcaConfig::default();
        config.provider.custom.api_key = Some("custom-test-key".into());
        config.provider.custom.base_url = base_url;
        config.provider.custom.compatibility = ProviderCompatibility::OpenAi;
        config.provider.custom.model = "custom-openai-model".into();

        let provider = CustomProvider::from_config(&config).expect("provider");
        let stream = provider
            .chat(
                &[Message::user("hello")],
                &[],
                "",
                std::path::Path::new("."),
            )
            .await
            .expect("chat stream");

        let chunks = collect_chunks(stream).await;
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Hello "));
        assert!(matches!(
            &chunks[1],
            StreamChunk::Usage {
                input_tokens: 5,
                output_tokens: 2
            }
        ));
    }

    #[tokio::test]
    async fn custom_anthropic_compatible_provider_streams_tools() {
        let body = concat!(
            "event: content_block_start\n",
            "data: {\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"lookup\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"src\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {}\n\n",
            "event: message_delta\n",
            "data: {\"usage\":{\"output_tokens\":5}}\n\n"
        )
        .to_string();
        let base_url = spawn_sse_server(body, 200, |request| {
            assert_eq!(request.url(), "/v1/messages");
        });

        let mut config = NcaConfig::default();
        config.provider.custom.api_key = Some("custom-test-key".into());
        config.provider.custom.base_url = base_url;
        config.provider.custom.compatibility = ProviderCompatibility::Anthropic;
        config.provider.custom.model = "custom-anthropic-model".into();

        let provider = CustomProvider::from_config(&config).expect("provider");
        let stream = provider
            .chat(
                &[Message::user("hello")],
                &[ToolDefinition {
                    name: "lookup".into(),
                    description: "Lookup a path".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {"path": {"type": "string"}}
                    }),
                }],
                "",
                std::path::Path::new("."),
            )
            .await
            .expect("chat stream");

        let chunks = collect_chunks(stream).await;
        assert!(matches!(
            &chunks[0],
            StreamChunk::ToolUse(call) if call.name == "lookup" && call.input == json!({"path":"src"})
        ));
    }
}
