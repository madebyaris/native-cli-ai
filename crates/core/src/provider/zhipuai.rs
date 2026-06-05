use std::path::Path;

use nca_common::config::{NcaConfig, ZhipuAIConfig};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

use super::openai_compat::{map_provider_error, openai_request_body, spawn_openai_stream};
use super::{Provider, ProviderError, StreamChunk};

pub struct ZhipuAIProvider {
    client: reqwest::Client,
    config: ZhipuAIConfig,
    max_tokens: u32,
}

impl ZhipuAIProvider {
    pub fn from_config(config: &NcaConfig) -> Result<Self, ProviderError> {
        let zhipuai = config.provider.zhipuai.clone();
        let api_key = zhipuai.resolve_api_key().ok_or_else(|| {
            ProviderError::Configuration(format!(
                "missing ZhipuAI API key; set {} or provide `provider.zhipuai.api_key` in config",
                zhipuai.api_key_env
            ))
        })?;

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                ProviderError::Configuration(format!(
                    "failed to build ZhipuAI authorization header: {err}"
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
            config: zhipuai,
            max_tokens: config.model.max_tokens,
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait::async_trait]
impl Provider for ZhipuAIProvider {
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
            return Err(map_provider_error(status, body_text));
        }

        Ok(spawn_openai_stream(response, "zhipuai"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::test_support::{collect_chunks, spawn_sse_server};
    use nca_common::message::Message;

    #[tokio::test]
    async fn zhipuai_provider_streams_text_and_usage() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"你好 \"},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"世界\"},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let base_url = spawn_sse_server(body, 200, |request| {
            assert_eq!(request.url(), "/chat/completions");
            let auth = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("authorization"))
                .expect("authorization header");
            assert_eq!(auth.value.as_str(), "Bearer zhipuai-test-key");
        });

        let mut config = NcaConfig::default();
        config.provider.zhipuai.api_key = Some("zhipuai-test-key".into());
        config.provider.zhipuai.base_url = base_url;

        let provider = ZhipuAIProvider::from_config(&config).expect("provider");
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
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "你好 "));
        assert!(matches!(&chunks[1], StreamChunk::TextDelta(text) if text == "世界"));
        assert!(matches!(
            &chunks[2],
            StreamChunk::Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..
            }
        ));
        assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
    }
}
