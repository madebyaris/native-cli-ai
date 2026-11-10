use std::path::Path;

use nca_common::config::{AnthropicConfig, NcaConfig, ProviderKind};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use reqwest::header::{HeaderMap, HeaderValue};

use super::anthropic_compat::{anthropic_request_body, map_provider_error, spawn_anthropic_stream};
use super::openai_compat::map_send_error;
use super::{Provider, ProviderError, StreamChunk};

pub struct AnthropicProvider {
    client: reqwest::Client,
    config: AnthropicConfig,
    oauth: bool,
    /// Subscription token a plugin handed back after a 401/403; used instead of the one the client was built with.
    refreshed_key: std::sync::Mutex<Option<String>>,
    max_tokens: u32,
}

/// Prefix of the `AuthError` message when the plugin cannot recover the sign-in. The UI
/// matches it to start the login flow again.
pub const SIGNED_OUT_PREFIX: &str = "signed out: ";

const OAUTH_BETA: &str = "oauth-2025-04-20";
const OAUTH_USER_AGENT: &str = "claude-cli/2.1.280 (external, cli)";
const OAUTH_SYSTEM_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Claude subscription OAuth access tokens are prefixed `sk-ant-oat`;
/// regular API keys are `sk-ant-api`.
fn is_oauth_token(key: &str) -> bool {
    key.starts_with("sk-ant-oat")
}

/// Adds Anthropic credentials to a request that is not a chat turn (the models list, for
/// one): `Authorization: Bearer` plus the OAuth beta for a subscription token, `x-api-key`
/// for a regular key.
pub fn with_anthropic_auth(req: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    let req = req.header("anthropic-version", "2023-06-01");
    if is_oauth_token(key) {
        req.bearer_auth(key)
            .header("anthropic-beta", OAUTH_BETA)
            .header("x-app", "cli")
            .header(reqwest::header::USER_AGENT, OAUTH_USER_AGENT)
    } else {
        req.header("x-api-key", key)
    }
}

/// Subscription tokens require the Claude Code identity as the first system block.
fn with_oauth_system(body: &mut serde_json::Value) {
    let mut blocks = vec![serde_json::json!({"type": "text", "text": OAUTH_SYSTEM_IDENTITY})];
    match body.get("system").cloned() {
        Some(serde_json::Value::String(text)) if !text.is_empty() => {
            blocks.push(serde_json::json!({"type": "text", "text": text}));
        }
        Some(serde_json::Value::Array(existing)) => blocks.extend(existing),
        _ => {}
    }
    body["system"] = serde_json::Value::Array(blocks);
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

        let oauth = is_oauth_token(&api_key);
        let mut headers = HeaderMap::new();
        if oauth {
            // Claude subscription (Pro/Max) OAuth access token, e.g. supplied by an auth plugin.
            headers.insert(
                reqwest::header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                    ProviderError::Configuration(format!(
                        "failed to build Anthropic Authorization header: {err}"
                    ))
                })?,
            );
            headers.insert("anthropic-beta", HeaderValue::from_static(OAUTH_BETA));
            headers.insert("x-app", HeaderValue::from_static("cli"));
            headers.insert(
                reqwest::header::USER_AGENT,
                HeaderValue::from_static(OAUTH_USER_AGENT),
            );
        } else {
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(&api_key).map_err(|err| {
                    ProviderError::Configuration(format!(
                        "failed to build Anthropic x-api-key header: {err}"
                    ))
                })?,
            );
        }
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        let client = super::client_builder()
            .default_headers(headers)
            .build()
            .map_err(|err| {
                ProviderError::Configuration(format!("failed to build HTTP client: {err}"))
            })?;

        Ok(Self {
            client,
            config: anthropic,
            oauth,
            refreshed_key: std::sync::Mutex::new(None),
            max_tokens: config.model.max_tokens,
        })
    }

    async fn send(&self, body: &serde_json::Value) -> Result<reqwest::Response, ProviderError> {
        let mut req = self.client.post(self.endpoint()).json(body);
        let key = self
            .refreshed_key
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(key) = key {
            // Per-request headers override the client's defaults.
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
        }
        req.send().await.map_err(map_send_error)
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
        workspace_root: &Path,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, ProviderError> {
        let model = if model.is_empty() {
            self.config.model.clone()
        } else {
            model.to_string()
        };

        let mut body = anthropic_request_body(
            messages,
            tools,
            &model,
            self.max_tokens,
            self.config.temperature,
            workspace_root,
        )?;
        if self.oauth {
            with_oauth_system(&mut body);
        }

        let mut response = self.send(&body).await?;
        let mut status = response.status();

        // A subscription token that an auth plugin supplied was rejected: let the plugin
        // refresh it and retry once. A dropped connection is not an auth failure and never
        // gets here (it fails in `send`), so a flaky network does not sign the user out.
        if self.oauth && matches!(status.as_u16(), 401 | 403) {
            let code = status.as_u16();
            let recovery = tokio::task::spawn_blocking(move || {
                nca_plugin::on_auth_failure(ProviderKind::Anthropic, code)
            })
            .await
            .unwrap_or_else(|e| nca_plugin::AuthRecovery::Unavailable(e.to_string()));
            match recovery {
                nca_plugin::AuthRecovery::NoPlugin => {}
                nca_plugin::AuthRecovery::Refreshed(key) => {
                    *self.refreshed_key.lock().unwrap_or_else(|e| e.into_inner()) = Some(key);
                    response = self.send(&body).await?;
                    status = response.status();
                }
                nca_plugin::AuthRecovery::Unavailable(message) => {
                    return Err(ProviderError::AuthError(format!(
                        "could not refresh the Claude sign-in ({message}); check your connection and try again"
                    )));
                }
                nca_plugin::AuthRecovery::LoginRequired(message) => {
                    return Err(ProviderError::AuthError(format!(
                        "{SIGNED_OUT_PREFIX}{message}. Run /login to sign in again."
                    )));
                }
            }
            if matches!(status.as_u16(), 401 | 403) && self.oauth {
                return Err(ProviderError::AuthError(format!(
                    "{SIGNED_OUT_PREFIX}the refreshed token was rejected too. Run /login to sign in again."
                )));
            }
        }

        if !status.is_success() {
            let headers = response.headers().clone();
            let body_text = response.text().await.unwrap_or_default();
            return Err(map_provider_error(status, &headers, body_text));
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

    fn auth_headers(key: &str) -> reqwest::header::HeaderMap {
        let client = reqwest::Client::new();
        with_anthropic_auth(client.get("http://localhost/v1/models"), key)
            .build()
            .expect("request builds")
            .headers()
            .clone()
    }

    #[test]
    fn models_request_uses_bearer_and_oauth_beta_for_a_subscription_token() {
        let headers = auth_headers("sk-ant-oat01-test");
        assert_eq!(headers["authorization"], "Bearer sk-ant-oat01-test");
        assert_eq!(headers["anthropic-beta"], OAUTH_BETA);
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        assert!(headers.get("x-api-key").is_none());
    }

    #[test]
    fn models_request_uses_x_api_key_for_a_regular_key() {
        let headers = auth_headers("sk-ant-api03-test");
        assert_eq!(headers["x-api-key"], "sk-ant-api03-test");
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        assert!(headers.get("authorization").is_none());
        assert!(headers.get("anthropic-beta").is_none());
    }

    #[test]
    fn oauth_system_identity_comes_first() {
        let mut body = json!({"system": "be helpful"});
        with_oauth_system(&mut body);
        assert_eq!(body["system"][0]["text"], OAUTH_SYSTEM_IDENTITY);
        assert_eq!(body["system"][1]["text"], "be helpful");
        let mut none = json!({"system": null});
        with_oauth_system(&mut none);
        assert_eq!(none["system"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn anthropic_provider_sends_bearer_for_oauth_token() {
        let body = "event: message_delta\ndata: {\"usage\":{\"output_tokens\":1}}\n\n".to_string();
        let base_url = spawn_sse_server(body, 200, |request| {
            fn h(request: &tiny_http::Request, name: &'static str) -> Option<String> {
                request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv(name))
                    .map(|header| header.value.as_str().to_string())
            }
            assert_eq!(
                h(request, "authorization").as_deref(),
                Some("Bearer sk-ant-oat01-test")
            );
            assert_eq!(h(request, "anthropic-beta").as_deref(), Some(OAUTH_BETA));
            assert!(h(request, "x-api-key").is_none());
        });
        let mut config = NcaConfig::default();
        config.provider.anthropic.api_key = Some("sk-ant-oat01-test".into());
        config.provider.anthropic.base_url = base_url;
        let provider = AnthropicProvider::from_config(&config).expect("provider");
        let stream = provider
            .chat(
                &[Message::system("s"), Message::user("hi")],
                &[],
                "",
                std::path::Path::new("."),
            )
            .await
            .expect("chat stream");
        let _ = collect_chunks(stream).await;
    }

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
                    .any(|header| header.field.equiv("x-api-key")
                        && header.value.as_str() == "anthropic-test-key")
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
                std::path::Path::new("."),
            )
            .await
            .expect("chat stream");

        let chunks = collect_chunks(stream).await;
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Hello from Claude"));
        assert!(
            matches!(&chunks[1], StreamChunk::ToolUse(call) if call.id == "toolu_1" && call.name == "lookup" && call.input == json!({"path":"src"}))
        );
        assert!(matches!(
            &chunks[2],
            StreamChunk::Usage {
                input_tokens: 13,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 5
            }
        ));
        assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
    }
}
