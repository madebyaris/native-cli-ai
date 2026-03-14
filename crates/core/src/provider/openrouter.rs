use nca_common::config::{NcaConfig, OpenRouterConfig};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};

use super::openai_compat::{map_provider_error, openai_request_body, spawn_openai_stream};
use super::{Provider, ProviderError, StreamChunk};

pub struct OpenRouterProvider {
    client: reqwest::Client,
    config: OpenRouterConfig,
    max_tokens: u32,
}

impl OpenRouterProvider {
    pub fn from_config(config: &NcaConfig) -> Result<Self, ProviderError> {
        let openrouter = config.provider.openrouter.clone();
        let api_key = openrouter.resolve_api_key().ok_or_else(|| {
            ProviderError::Configuration(format!(
                "missing OpenRouter API key; set {} or provide `provider.openrouter.api_key` in config",
                openrouter.api_key_env
            ))
        })?;

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                ProviderError::Configuration(format!(
                    "failed to build OpenRouter authorization header: {err}"
                ))
            })?,
        );
        if let Some(site_url) = &openrouter.site_url {
            headers.insert(
                HeaderName::from_static("http-referer"),
                HeaderValue::from_str(site_url).map_err(|err| {
                    ProviderError::Configuration(format!(
                        "failed to build OpenRouter HTTP-Referer header: {err}"
                    ))
                })?,
            );
        }
        if let Some(app_name) = &openrouter.app_name {
            headers.insert(
                HeaderName::from_static("x-title"),
                HeaderValue::from_str(app_name).map_err(|err| {
                    ProviderError::Configuration(format!(
                        "failed to build OpenRouter X-Title header: {err}"
                    ))
                })?,
            );
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|err| {
                ProviderError::Configuration(format!("failed to build HTTP client: {err}"))
            })?;

        Ok(Self {
            client,
            config: openrouter,
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
impl Provider for OpenRouterProvider {
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

        let body = openai_request_body(
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

        Ok(spawn_openai_stream(response, "openrouter"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::test_support::{collect_chunks, spawn_sse_server};
    use nca_common::message::Message;

    #[tokio::test]
    async fn openrouter_provider_sends_optional_headers_and_streams_usage() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Router hello\"},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let base_url = spawn_sse_server(body, 200, |request| {
            assert_eq!(request.url(), "/v1/chat/completions");
            assert!(
                request
                    .headers()
                    .iter()
                    .any(|header| header.field.equiv("authorization")
                        && header.value.as_str() == "Bearer openrouter-test-key")
            );
            assert!(
                request
                    .headers()
                    .iter()
                    .any(|header| header.field.equiv("http-referer")
                        && header.value.as_str() == "https://nca.test")
            );
            assert!(
                request
                    .headers()
                    .iter()
                    .any(|header| header.field.equiv("x-title")
                        && header.value.as_str() == "Native CLI AI")
            );
        });

        let mut config = NcaConfig::default();
        config.provider.openrouter.api_key = Some("openrouter-test-key".into());
        config.provider.openrouter.base_url = base_url;
        config.provider.openrouter.site_url = Some("https://nca.test".into());
        config.provider.openrouter.app_name = Some("Native CLI AI".into());

        let provider = OpenRouterProvider::from_config(&config).expect("provider");
        let stream = provider
            .chat(&[Message::user("hello")], &[], "")
            .await
            .expect("chat stream");

        let chunks = collect_chunks(stream).await;
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Router hello"));
        assert!(matches!(
            &chunks[1],
            StreamChunk::Usage {
                input_tokens: 9,
                output_tokens: 4
            }
        ));
        assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
    }
}
