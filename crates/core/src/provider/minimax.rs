use futures_util::StreamExt;
use nca_common::config::{MiniMaxConfig, NcaConfig};
use nca_common::message::{Message, Role};
use nca_common::tool::{ToolCall, ToolDefinition};
use reqwest::header::AUTHORIZATION;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
        let model = if model.is_empty() {
            self.config.model.clone()
        } else {
            model.to_string()
        };
        let body = MiniMaxRequest {
            model,
            messages: messages.iter().map(MiniMaxRequestMessage::from).collect(),
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.iter().map(MiniMaxToolSpec::from).collect())
            },
            stream: true,
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

        let mut stream = response.bytes_stream();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut pending_tools: BTreeMap<usize, PendingToolCall> = BTreeMap::new();

            while let Some(item) = stream.next().await {
                let chunk = match item {
                    Ok(chunk) => chunk,
                    Err(err) => {
                        let _ = tx
                            .send(StreamChunk::TextDelta(format!(
                                "\n[provider stream error: {err}]"
                            )))
                            .await;
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(newline_idx) = buffer.find('\n') {
                    let line = buffer[..newline_idx].trim().to_string();
                    buffer.drain(..=newline_idx);

                    if line.is_empty() || !line.starts_with("data:") {
                        continue;
                    }

                    let data = line.trim_start_matches("data:").trim();
                    if data == "[DONE]" {
                        break;
                    }

                    let parsed = serde_json::from_str::<MiniMaxStreamEvent>(data);
                    let Ok(event) = parsed else {
                        continue;
                    };

                    if let Some(usage) = event.usage {
                        let input_tokens = usage.prompt_tokens.unwrap_or(0);
                        let output_tokens = usage.completion_tokens.unwrap_or(0);
                        if input_tokens == 0 && output_tokens == 0 {
                            continue;
                        }
                        let _ = tx
                            .send(StreamChunk::Usage {
                                input_tokens,
                                output_tokens,
                            })
                            .await;
                    }

                    for choice in event.choices {
                        if let Some(delta) = choice.delta {
                            if let Some(content) = delta.content {
                                if !content.is_empty() {
                                    let _ = tx.send(StreamChunk::TextDelta(content)).await;
                                }
                            }

                            if let Some(tool_calls) = delta.tool_calls {
                                for tool_call in tool_calls {
                                    let entry = pending_tools
                                        .entry(tool_call.index.unwrap_or(0))
                                        .or_insert_with(PendingToolCall::default);
                                    if let Some(id) = tool_call.id {
                                        entry.id = id;
                                    }
                                    if let Some(function) = tool_call.function {
                                        if let Some(name) = function.name {
                                            entry.name = name;
                                        }
                                        if let Some(arguments) = function.arguments {
                                            entry.arguments.push_str(&arguments);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            for pending in pending_tools.into_values() {
                if pending.name.is_empty() {
                    continue;
                }
                if let Ok(input) = serde_json::from_str(&pending.arguments) {
                    let _ = tx
                        .send(StreamChunk::ToolUse(ToolCall {
                            id: pending.id,
                            name: pending.name,
                            input,
                        }))
                        .await;
                }
            }

            let _ = tx.send(StreamChunk::Done).await;
        });
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

#[derive(Debug, Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct MiniMaxStreamEvent {
    #[serde(default)]
    choices: Vec<MiniMaxStreamChoice>,
    #[serde(default)]
    usage: Option<MiniMaxUsage>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxStreamChoice {
    #[serde(default)]
    delta: Option<MiniMaxDelta>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<MiniMaxDeltaToolCall>>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxDeltaToolCall {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<MiniMaxDeltaFunction>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxDeltaFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}
