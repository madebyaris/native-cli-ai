use nca_common::config::{MiniMaxConfig, NcaConfig};
use nca_common::message::{Message, Role};
use nca_common::tool::{ToolCall, ToolDefinition};
use reqwest::header::AUTHORIZATION;
use serde::{Deserialize, Serialize};

use super::{Provider, ProviderError, StreamChunk};

/// MiniMax provider implementation using the native chat completion API.
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

        let client = reqwest::Client::builder()
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(
                    AUTHORIZATION,
                    format!("Bearer {api_key}").parse().map_err(|err| {
                        ProviderError::Configuration(format!(
                            "failed to build MiniMax authorization header: {err}"
                        ))
                    })?,
                );
                headers
            })
            .build()
            .map_err(|err| {
                ProviderError::Configuration(format!("failed to build MiniMax HTTP client: {err}"))
            })?;

        Ok(Self {
            client,
            config: minimax,
            max_tokens: config.model.max_tokens,
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/text/chatcompletion_v2",
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
        let body = MiniMaxRequest {
            model: if model.is_empty() {
                self.config.model.clone()
            } else {
                model.to_string()
            },
            messages: messages.iter().map(MiniMaxRequestMessage::from).collect(),
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.iter().map(MiniMaxToolSpec::from).collect())
            },
            stream: false,
            max_tokens: self.max_tokens,
            temperature: self.config.temperature,
        };

        let response = self
            .client
            .post(self.endpoint())
            .json(&body)
            .send()
            .await
            .map_err(|err| ProviderError::RequestFailed(err.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return match status.as_u16() {
                401 | 403 => Err(ProviderError::AuthError(body)),
                404 => Err(ProviderError::ModelNotFound(body)),
                429 => Err(ProviderError::RateLimited {
                    retry_after_ms: 1000,
                }),
                _ => Err(ProviderError::RequestFailed(body)),
            };
        }

        let payload: MiniMaxResponse = response
            .json()
            .await
            .map_err(|err| ProviderError::RequestFailed(err.to_string()))?;

        let choice = payload
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Other("MiniMax returned no choices".into()))?;

        let message = choice
            .message
            .ok_or_else(|| ProviderError::Other("MiniMax returned no message".into()))?;

        let (tx, rx) = tokio::sync::mpsc::channel(16);

        if !message.content.trim().is_empty() {
            tx.send(StreamChunk::TextDelta(message.content))
                .await
                .map_err(|_| ProviderError::Other("failed to emit MiniMax text chunk".into()))?;
        }

        if let Some(tool_calls) = message.tool_calls {
            for call in tool_calls {
                let input = serde_json::from_str(&call.function.arguments).map_err(|err| {
                    ProviderError::Other(format!(
                        "failed to parse MiniMax tool arguments for {}: {err}",
                        call.function.name
                    ))
                })?;

                tx.send(StreamChunk::ToolUse(ToolCall {
                    id: call.id,
                    name: call.function.name,
                    input,
                }))
                .await
                .map_err(|_| ProviderError::Other("failed to emit MiniMax tool call".into()))?;
            }
        }

        tx.send(StreamChunk::Done)
            .await
            .map_err(|_| ProviderError::Other("failed to emit MiniMax completion event".into()))?;

        Ok(rx)
    }
}

#[derive(Debug, Serialize)]
struct MiniMaxRequest {
    model: String,
    messages: Vec<MiniMaxRequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<MiniMaxToolSpec>>,
    stream: bool,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct MiniMaxRequestMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<MiniMaxReplayToolCall>>,
}

impl From<&Message> for MiniMaxRequestMessage {
    fn from(message: &Message) -> Self {
        Self {
            role: match message.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
                Role::Tool => "tool",
            }
            .to_string(),
            content: message.content.clone(),
            tool_call_id: message.tool_call_id.clone(),
            tool_calls: message.tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|c| MiniMaxReplayToolCall {
                        id: c.id.clone(),
                        r#type: "function".into(),
                        function: MiniMaxReplayToolFunction {
                            name: c.name.clone(),
                            arguments: c.arguments.to_string(),
                        },
                    })
                    .collect()
            }),
        }
    }
}

#[derive(Debug, Serialize)]
struct MiniMaxReplayToolCall {
    id: String,
    r#type: String,
    function: MiniMaxReplayToolFunction,
}

#[derive(Debug, Serialize)]
struct MiniMaxReplayToolFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct MiniMaxToolSpec {
    r#type: String,
    function: MiniMaxFunctionSpec,
}

impl From<&ToolDefinition> for MiniMaxToolSpec {
    fn from(tool: &ToolDefinition) -> Self {
        Self {
            r#type: "function".into(),
            function: MiniMaxFunctionSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct MiniMaxFunctionSpec {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct MiniMaxResponse {
    choices: Vec<MiniMaxChoice>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxChoice {
    message: Option<MiniMaxResponseMessage>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxResponseMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Option<Vec<MiniMaxToolCall>>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxToolCall {
    id: String,
    function: MiniMaxToolFunction,
}

#[derive(Debug, Deserialize)]
struct MiniMaxToolFunction {
    name: String,
    arguments: String,
}
