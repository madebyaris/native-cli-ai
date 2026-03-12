use futures_util::StreamExt;
use nca_common::config::{MiniMaxConfig, NcaConfig};
use nca_common::message::{Message, Role};
use nca_common::tool::{ToolCall, ToolDefinition};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Provider, ProviderError, StreamChunk};

/// MiniMax provider using the Anthropic-compatible endpoint.
/// Endpoint: <base_url>/v1/messages
/// Auth: Authorization: Bearer <api_key>
///
/// The Anthropic format gives reliable streaming for reasoning models:
/// thinking blocks are separate from text/tool_use blocks, and tool use
/// is represented as typed content blocks rather than a parallel JSON field.
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

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                ProviderError::Configuration(format!(
                    "failed to build MiniMax authorization header: {err}"
                ))
            })?,
        );
        // Anthropic-compatible endpoint also accepts x-api-key
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&api_key).map_err(|err| {
                ProviderError::Configuration(format!(
                    "failed to build x-api-key header: {err}"
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
            config: minimax,
            max_tokens: config.model.max_tokens,
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/messages",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

// ── Anthropic request types ──────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    stream: bool,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

// ── Message conversion ────────────────────────────────────────────────────────

/// Convert nca internal messages to Anthropic format.
/// Returns `(system_prompt, anthropic_messages)`.
///
/// Rules:
/// - System messages at the front are extracted into a separate `system` field.
/// - Assistant messages with tool calls are encoded as multi-part content blocks.
/// - Consecutive Tool messages are grouped into a single user message of
///   `tool_result` content blocks (Anthropic API requirement).
fn to_anthropic_messages(
    messages: &[Message],
) -> (Option<String>, Vec<AnthropicMessage>) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut out: Vec<AnthropicMessage> = Vec::new();
    let mut i = 0;

    // Collect leading system messages
    while i < messages.len() && messages[i].role == Role::System {
        system_parts.push(messages[i].content.clone());
        i += 1;
    }

    while i < messages.len() {
        let msg = &messages[i];
        match msg.role {
            Role::User => {
                out.push(AnthropicMessage {
                    role: "user".into(),
                    content: json!(msg.content),
                });
                i += 1;
            }

            Role::Assistant => {
                let mut blocks: Vec<serde_json::Value> = Vec::new();

                if !msg.content.is_empty() {
                    blocks.push(json!({"type": "text", "text": msg.content}));
                }

                if let Some(calls) = &msg.tool_calls {
                    for tc in calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments,
                        }));
                    }
                }

                let content = if blocks.is_empty() {
                    json!(msg.content)
                } else {
                    json!(blocks)
                };

                out.push(AnthropicMessage {
                    role: "assistant".into(),
                    content,
                });
                i += 1;
            }

            Role::Tool => {
                // Group all consecutive Tool messages into one user message
                let mut results: Vec<serde_json::Value> = Vec::new();
                while i < messages.len() && messages[i].role == Role::Tool {
                    let tm = &messages[i];
                    results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": tm.tool_call_id.as_deref().unwrap_or(""),
                        "content": tm.content,
                    }));
                    i += 1;
                }
                out.push(AnthropicMessage {
                    role: "user".into(),
                    content: json!(results),
                });
            }

            // System messages after the leading block are folded into the
            // preceding assistant message's text or appended as user context.
            Role::System => {
                i += 1;
            }
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    (system, out)
}

// ── Provider implementation ──────────────────────────────────────────────────

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

        let (system, anthropic_messages) = to_anthropic_messages(messages);

        let anthropic_tools: Option<Vec<AnthropicTool>> = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| AnthropicTool {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        input_schema: t.parameters.clone(),
                    })
                    .collect(),
            )
        };

        let body = AnthropicRequest {
            model,
            max_tokens: self.max_tokens,
            system,
            messages: anthropic_messages,
            tools: anthropic_tools,
            stream: true,
            // Anthropic requires temperature=1 when extended thinking is active.
            // MiniMax-M2.5 is a reasoning model; using 1.0 avoids API errors.
            temperature: 1.0,
        };

        if std::env::var("NCA_DEBUG_REQUEST").is_ok() {
            eprintln!(
                "[minimax:request] {}",
                serde_json::to_string_pretty(&body).unwrap_or_default()
            );
        }

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
            return match status.as_u16() {
                401 | 403 => Err(ProviderError::AuthError(body_text)),
                404 => Err(ProviderError::ModelNotFound(body_text)),
                429 => Err(ProviderError::RateLimited {
                    retry_after_ms: 1000,
                }),
                _ => Err(ProviderError::RequestFailed(body_text)),
            };
        }

        let mut byte_stream = response.bytes_stream();
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            let mut buffer = String::new();
            // Current SSE event type (from `event:` lines)
            let mut event_type = String::new();
            // Tool-use block being assembled
            let mut tool_id = String::new();
            let mut tool_name = String::new();
            let mut tool_input = String::new();
            // Tokens from message_start (input) and message_delta (output)
            let mut input_tokens: u64 = 0;

            while let Some(item) = byte_stream.next().await {
                let chunk = match item {
                    Ok(c) => c,
                    Err(err) => {
                        let _ = tx
                            .send(StreamChunk::TextDelta(format!(
                                "\n[stream error: {err}]"
                            )))
                            .await;
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete lines
                while let Some(nl) = buffer.find('\n') {
                    let raw = buffer[..nl].to_string();
                    buffer.drain(..=nl);
                    let line = raw.trim_end_matches('\r').trim();

                    if line.is_empty() {
                        // Blank line resets event type (SSE event boundary)
                        event_type.clear();
                        continue;
                    }

                    if let Some(ev) = line.strip_prefix("event:") {
                        event_type = ev.trim().to_string();
                        continue;
                    }

                    if !line.starts_with("data:") {
                        continue;
                    }

                    let data = line["data:".len()..].trim();
                    if data == "[DONE]" {
                        break;
                    }

                    let Ok(ev) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };

                    match event_type.as_str() {
                        "message_start" => {
                            input_tokens = ev["message"]["usage"]["input_tokens"]
                                .as_u64()
                                .unwrap_or(0);
                        }

                        "content_block_start" => {
                            let block = &ev["content_block"];
                            let btype = block["type"].as_str().unwrap_or("");
                            if btype == "tool_use" {
                                tool_id = block["id"].as_str().unwrap_or("").to_string();
                                tool_name = block["name"].as_str().unwrap_or("").to_string();
                                tool_input.clear();
                            }
                        }

                        "content_block_delta" => {
                            let delta = &ev["delta"];
                            match delta["type"].as_str().unwrap_or("") {
                                "text_delta" => {
                                    if let Some(text) = delta["text"].as_str() {
                                        if !text.is_empty() {
                                            let _ = tx
                                                .send(StreamChunk::TextDelta(text.to_string()))
                                                .await;
                                        }
                                    }
                                }
                                "input_json_delta" => {
                                    if let Some(partial) = delta["partial_json"].as_str() {
                                        tool_input.push_str(partial);
                                    }
                                }
                                // thinking_delta — ignore; we don't surface raw thinking
                                _ => {}
                            }
                        }

                        "content_block_stop" => {
                            if !tool_name.is_empty() {
                                if let Ok(input) = serde_json::from_str(&tool_input) {
                                    let _ = tx
                                        .send(StreamChunk::ToolUse(ToolCall {
                                            id: tool_id.clone(),
                                            name: tool_name.clone(),
                                            input,
                                        }))
                                        .await;
                                }
                                tool_id.clear();
                                tool_name.clear();
                                tool_input.clear();
                            }
                        }

                        "message_delta" => {
                            let output_tokens = ev["usage"]["output_tokens"]
                                .as_u64()
                                .unwrap_or(0);
                            if input_tokens > 0 || output_tokens > 0 {
                                let _ = tx
                                    .send(StreamChunk::Usage {
                                        input_tokens,
                                        output_tokens,
                                    })
                                    .await;
                                // Reset so a second message_delta doesn't double-count
                                input_tokens = 0;
                            }
                        }

                        _ => {}
                    }
                }
            }

            let _ = tx.send(StreamChunk::Done).await;
        });

        Ok(rx)
    }
}

// ── Deserialization helpers (unused but kept for optional introspection) ──────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
}
