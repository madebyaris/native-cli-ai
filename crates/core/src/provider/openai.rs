use std::path::Path;

use nca_common::config::{NcaConfig, OpenAiConfig};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

use super::openai_compat::{
    map_provider_error, map_send_error, openai_request_body, spawn_openai_stream,
};
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

        let client = super::client_builder()
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

/// GPT-6 Luna and Terra accept function calls on Chat Completions only with
/// `reasoning_effort` set to `none`. Other models keep the request unchanged.
fn apply_gpt6_tool_effort(body: &mut serde_json::Value, model: &str, has_tools: bool) {
    let supports_none = model.starts_with("gpt-6-luna") || model.starts_with("gpt-6.1-terra");
    if has_tools && supports_none {
        body["reasoning_effort"] = serde_json::json!("none");
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
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

        let mut body = openai_request_body(
            messages,
            tools,
            &model,
            self.max_tokens,
            self.config.temperature,
            workspace_root,
        )?;
        apply_gpt6_tool_effort(&mut body, &model, !tools.is_empty());

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

        Ok(spawn_openai_stream(response, "openai"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::test_support::{collect_chunks, spawn_raw_http_server, spawn_sse_server};
    use nca_common::message::Message;
    use nca_common::tool::ToolDefinition;
    use serde_json::json;

    async fn chat_error(raw_response: &str) -> ProviderError {
        let mut config = NcaConfig::default();
        config.provider.openai.api_key = Some("openai-test-key".into());
        config.provider.openai.base_url = spawn_raw_http_server(raw_response.to_string());

        OpenAiProvider::from_config(&config)
            .expect("provider")
            .chat(
                &[Message::user("hello")],
                &[],
                "",
                std::path::Path::new("."),
            )
            .await
            .expect_err("request should fail")
    }

    #[tokio::test]
    async fn openai_provider_reads_retry_after_from_a_429_response() {
        let err = chat_error(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Length: 4\r\nConnection: close\r\n\r\nslow",
        )
        .await;

        assert!(
            matches!(
                err,
                ProviderError::RateLimited {
                    retry_after_ms: 7000
                }
            ),
            "unexpected error: {err:?}"
        );
    }

    #[tokio::test]
    async fn openai_provider_reports_server_errors_as_transient() {
        let err = chat_error(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 10\r\nConnection: close\r\n\r\noverloaded",
        )
        .await;

        assert!(
            matches!(&err, ProviderError::Transient(message) if message.starts_with("HTTP 503")),
            "unexpected error: {err:?}"
        );
        assert!(err.is_retryable());
    }

    #[test]
    fn gpt6_luna_and_terra_disable_reasoning_only_when_tools_are_sent() {
        let mut luna = serde_json::json!({"model": "gpt-6-luna"});
        apply_gpt6_tool_effort(&mut luna, "gpt-6-luna", true);
        assert_eq!(luna["reasoning_effort"], "none");

        let mut terra = serde_json::json!({"model": "gpt-6.1-terra"});
        apply_gpt6_tool_effort(&mut terra, "gpt-6.1-terra", true);
        assert_eq!(terra["reasoning_effort"], "none");

        let mut no_tools = serde_json::json!({"model": "gpt-6-luna"});
        apply_gpt6_tool_effort(&mut no_tools, "gpt-6-luna", false);
        assert!(no_tools.get("reasoning_effort").is_none());

        let mut sol = serde_json::json!({"model": "gpt-6.1-sol"});
        apply_gpt6_tool_effort(&mut sol, "gpt-6.1-sol", true);
        assert!(sol.get("reasoning_effort").is_none());

        let mut older = serde_json::json!({"model": "gpt-4o-mini"});
        apply_gpt6_tool_effort(&mut older, "gpt-4o-mini", true);
        assert!(older.get("reasoning_effort").is_none());
    }

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
                std::path::Path::new("."),
            )
            .await
            .expect("chat stream");

        let chunks = collect_chunks(stream).await;
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Hello "));
        assert!(
            matches!(&chunks[1], StreamChunk::ToolUse(call) if call.id == "call_1" && call.name == "lookup" && call.input == json!({"path":"src"}))
        );
        assert!(matches!(
            &chunks[2],
            StreamChunk::Usage {
                input_tokens: 11,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 7
            }
        ));
        assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
    }
}
