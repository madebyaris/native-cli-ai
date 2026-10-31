use std::path::Path;

use nca_common::config::{AlibabaConfig, NcaConfig};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

use super::openai_compat::{
    map_provider_error, map_send_error, openai_request_body, spawn_openai_stream,
};
use super::{Provider, ProviderError, StreamChunk};

pub struct AlibabaProvider {
    client: reqwest::Client,
    config: AlibabaConfig,
    max_tokens: u32,
}

impl AlibabaProvider {
    pub fn from_config(config: &NcaConfig) -> Result<Self, ProviderError> {
        let alibaba = config.provider.alibaba.clone();
        let api_key = alibaba.resolve_api_key().ok_or_else(|| {
            ProviderError::Configuration(format!(
                "missing Alibaba API key; set {} or provide `provider.alibaba.api_key` in config",
                alibaba.api_key_env
            ))
        })?;

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                ProviderError::Configuration(format!(
                    "failed to build Alibaba authorization header: {err}"
                ))
            })?,
        );

        let client = super::client_builder()
            .default_headers(headers)
            .build()
            .map_err(|err| {
                ProviderError::Configuration(format!("failed to build HTTP client: {err}"))
            })?;

        Ok(Self {
            client,
            config: alibaba,
            max_tokens: config.model.max_tokens,
        })
    }

    fn endpoint(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        }
    }
}

#[async_trait::async_trait]
impl Provider for AlibabaProvider {
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
            .map_err(map_send_error)?;

        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let body_text = response.text().await.unwrap_or_default();
            return Err(map_provider_error(status, &headers, body_text));
        }

        Ok(spawn_openai_stream(response, "alibaba"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_resolves_v1_path_correctly() {
        let mut config = AlibabaConfig::default();
        config.base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1".into();
        let provider = AlibabaProvider {
            client: reqwest::Client::new(),
            config,
            max_tokens: 4096,
        };
        assert_eq!(
            provider.endpoint(),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
        );

        let mut config2 = AlibabaConfig::default();
        config2.base_url = "https://dashscope.aliyuncs.com/compatible-mode".into();
        let provider2 = AlibabaProvider {
            client: reqwest::Client::new(),
            config: config2,
            max_tokens: 4096,
        };
        assert_eq!(
            provider2.endpoint(),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
        );
    }
}
