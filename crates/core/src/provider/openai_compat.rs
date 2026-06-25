use super::{Provider, ProviderError, StreamChunk};

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use futures_util::StreamExt;
use nca_common::message::{ContentPart, Message, MessageContent, Role};
use nca_common::tool::{ToolCall, ToolDefinition};
use serde_json::{Value, json};

pub fn openai_request_body(
    messages: &[Message],
    tools: &[ToolDefinition],
    model: &str,
    max_tokens: u32,
    temperature: f32,
    workspace_root: &Path,
) -> Result<Value, ProviderError> {
    let tools = if tools.is_empty() {
        None
    } else {
        Some(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    })
                })
                .collect::<Vec<_>>(),
        )
    };

    Ok(json!({
        "model": model,
        "messages": to_openai_messages(messages, workspace_root)?,
        "tools": tools,
        "stream": true,
        "stream_options": {
            "include_usage": true
        },
        "max_tokens": max_tokens,
        "temperature": temperature,
    }))
}

pub fn spawn_openai_stream(
    response: reqwest::Response,
    provider_name: &'static str,
) -> tokio::sync::mpsc::Receiver<StreamChunk> {
    let mut byte_stream = response.bytes_stream();
    let (tx, rx) = tokio::sync::mpsc::channel(64);

    tokio::spawn(async move {
        let mut buffer = String::new();
        let mut tool_calls: BTreeMap<u64, ToolCallAccumulator> = BTreeMap::new();

        while let Some(item) = byte_stream.next().await {
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(err) => {
                    let _ = tx
                        .send(StreamChunk::TextDelta(format!(
                            "\n[{provider_name} stream error: {err}]"
                        )))
                        .await;
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(nl) = buffer.find('\n') {
                let raw = buffer[..nl].to_string();
                buffer.drain(..=nl);
                let line = raw.trim_end_matches('\r').trim();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if !line.starts_with("data:") {
                    continue;
                }

                let data = line["data:".len()..].trim();
                if data == "[DONE]" {
                    flush_openai_tool_calls(&tx, &mut tool_calls).await;
                    let _ = tx.send(StreamChunk::Done).await;
                    return;
                }

                let Ok(event) = serde_json::from_str::<Value>(data) else {
                    continue;
                };

                if let Some(usage) = event.get("usage") {
                    let input_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
                    let output_tokens = usage["completion_tokens"].as_u64().unwrap_or(0);

                    // OpenAI format: prompt_tokens_details.cached_tokens
                    // DeepSeek format: prompt_cache_hit_tokens / prompt_cache_miss_tokens
                    let cached_tokens = usage
                        .get("prompt_tokens_details")
                        .and_then(|d| d.get("cached_tokens"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let (cache_creation_tokens, cache_read_tokens) = if cached_tokens > 0 {
                        // OpenAI style: cached_tokens are hits; misses are input - cached
                        (0, cached_tokens)
                    } else if let Some(miss) = usage
                        .get("prompt_cache_miss_tokens")
                        .and_then(|v| v.as_u64())
                    {
                        // DeepSeek style: explicit hit/miss fields
                        let hit = usage
                            .get("prompt_cache_hit_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        (miss, hit)
                    } else {
                        (0, 0)
                    };

                    if input_tokens > 0 || output_tokens > 0 {
                        let _ = tx
                            .send(StreamChunk::Usage {
                                input_tokens,
                                output_tokens,
                                cache_creation_tokens,
                                cache_read_tokens,
                            })
                            .await;
                    }
                }

                let Some(choices) = event["choices"].as_array() else {
                    continue;
                };

                for choice in choices {
                    let delta = &choice["delta"];
                    if let Some(text) = delta["content"].as_str()
                        && !text.is_empty()
                    {
                        let _ = tx.send(StreamChunk::TextDelta(text.to_string())).await;
                    }

                    if let Some(reasoning) = delta["reasoning_content"].as_str()
                        && !reasoning.is_empty()
                    {
                        let _ = tx
                            .send(StreamChunk::ReasoningDelta(reasoning.to_string()))
                            .await;
                    }

                    if let Some(tool_deltas) = delta["tool_calls"].as_array() {
                        for tool_delta in tool_deltas {
                            let index = tool_delta["index"].as_u64().unwrap_or(0);
                            let entry = tool_calls.entry(index).or_default();
                            if let Some(id) = tool_delta["id"].as_str() {
                                entry.id = id.to_string();
                            }
                            if let Some(name) = tool_delta["function"]["name"].as_str() {
                                entry.name.push_str(name);
                            }
                            if let Some(arguments) = tool_delta["function"]["arguments"].as_str() {
                                entry.arguments.push_str(arguments);
                            }
                        }
                    }

                    if choice["finish_reason"].as_str() == Some("tool_calls") {
                        flush_openai_tool_calls(&tx, &mut tool_calls).await;
                    }
                }
            }
        }

        flush_openai_tool_calls(&tx, &mut tool_calls).await;
        let _ = tx.send(StreamChunk::Done).await;
    });

    rx
}

pub fn map_provider_error(status: reqwest::StatusCode, body_text: String) -> ProviderError {
    match status.as_u16() {
        401 | 403 => ProviderError::AuthError(body_text),
        404 => ProviderError::ModelNotFound(body_text),
        429 => ProviderError::RateLimited {
            retry_after_ms: 1000,
        },
        _ => ProviderError::RequestFailed(body_text),
    }
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

async fn flush_openai_tool_calls(
    tx: &tokio::sync::mpsc::Sender<StreamChunk>,
    tool_calls: &mut BTreeMap<u64, ToolCallAccumulator>,
) {
    let drained = std::mem::take(tool_calls);
    for (index, call) in drained {
        if call.name.is_empty() {
            continue;
        }

        if let Ok(input) = serde_json::from_str(&call.arguments) {
            let _ = tx
                .send(StreamChunk::ToolUse(ToolCall {
                    id: if call.id.is_empty() {
                        format!("tool-call-{index}")
                    } else {
                        call.id
                    },
                    name: call.name,
                    input,
                }))
                .await;
        }
    }
}

fn tool_content_string(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Parts(_) => content.to_summary_text(),
    }
}

fn openai_user_content_value(
    content: &MessageContent,
    workspace_root: &Path,
) -> Result<Value, ProviderError> {
    match content {
        MessageContent::Text(s) => Ok(json!(s)),
        MessageContent::Parts(parts) => {
            let mut blocks = Vec::new();
            for p in parts {
                match p {
                    ContentPart::Text { text } => {
                        blocks.push(json!({
                            "type": "text",
                            "text": text,
                        }));
                    }
                    ContentPart::Image { media_type, path } => {
                        let full = workspace_root.join(path);
                        let bytes = std::fs::read(&full).map_err(|e| {
                            ProviderError::RequestFailed(format!(
                                "failed to read image {}: {e}",
                                full.display()
                            ))
                        })?;
                        let b64 = B64.encode(bytes);
                        let url = format!("data:{media_type};base64,{b64}");
                        blocks.push(json!({
                            "type": "image_url",
                            "image_url": { "url": url }
                        }));
                    }
                }
            }
            Ok(Value::Array(blocks))
        }
    }
}

fn to_openai_messages(
    messages: &[Message],
    workspace_root: &Path,
) -> Result<Vec<Value>, ProviderError> {
    let mut out = Vec::new();

    for message in messages {
        match message.role {
            Role::System => out.push(json!({
                "role": "system",
                "content": tool_content_string(&message.content),
            })),
            Role::User => {
                let c = openai_user_content_value(&message.content, workspace_root)?;
                out.push(json!({
                    "role": "user",
                    "content": c,
                }));
            }
            Role::Assistant => {
                let mut value = json!({
                    "role": "assistant",
                    "content": if message.content.is_empty() && message.tool_calls.is_some() {
                        Value::Null
                    } else {
                        openai_user_content_value(&message.content, workspace_root)?
                    },
                });

                if let Some(reasoning) = &message.reasoning_content {
                    value["reasoning_content"] = json!(reasoning);
                }

                if let Some(calls) = &message.tool_calls {
                    value["tool_calls"] = Value::Array(
                        calls
                            .iter()
                            .map(|call| {
                                json!({
                                    "id": call.id,
                                    "type": "function",
                                    "function": {
                                        "name": call.name,
                                        "arguments": serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".into()),
                                    }
                                })
                            })
                            .collect(),
                    );
                }

                out.push(value);
            }
            Role::Tool => out.push(json!({
                "role": "tool",
                "tool_call_id": message.tool_call_id,
                "content": tool_content_string(&message.content),
            })),
        }
    }

    Ok(out)
}

/// Static profile describing the unique aspects of an OpenAI-compatible provider.
pub struct CompatProfile {
    /// Human-readable provider name (for error messages and stream labels).
    pub name: &'static str,
    /// Provider-specific endpoint suffix appended to base_url.
    pub endpoint_suffix: &'static str,
    /// Whether prepare_messages_for_request should strip reasoning_content.
    pub strip_reasoning: bool,
}

/// Generic OpenAI-compatible provider parameterized by a CompatProfile.
pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    name: &'static str,
    model: String,
    max_tokens: u32,
    temperature: f32,
    base_url: String,
    endpoint_suffix: &'static str,
    strip_reasoning: bool,
}

impl OpenAiCompatProvider {
    pub fn from_config(
        compat: &dyn nca_common::config::OpenAiCompatConfig,
        max_tokens: u32,
        profile: CompatProfile,
        extra_headers: reqwest::header::HeaderMap,
    ) -> Result<Self, ProviderError> {
        let api_key = compat.resolve_api_key().ok_or_else(|| {
            ProviderError::Configuration(format!(
                "missing {} API key; set {} or provide in config",
                profile.name,
                compat.api_key_env()
            ))
        })?;

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(
                |err| {
                    ProviderError::Configuration(format!(
                        "failed to build {} authorization header: {err}",
                        profile.name
                    ))
                },
            )?,
        );
        // Merge any extra headers (e.g. OpenRouter's http-referer, x-title).
        headers.extend(extra_headers);

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| {
                ProviderError::Configuration(format!("failed to build HTTP client: {err}"))
            })?;

        Ok(Self {
            client,
            name: profile.name,
            model: compat.model().to_string(),
            max_tokens,
            temperature: compat.temperature(),
            base_url: compat.base_url().to_string(),
            endpoint_suffix: profile.endpoint_suffix,
            strip_reasoning: profile.strip_reasoning,
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            self.endpoint_suffix
        )
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiCompatProvider {
    async fn prepare_messages_for_request(
        &self,
        messages: &mut Vec<Message>,
        _workspace_root: &Path,
    ) -> Result<(), ProviderError> {
        if self.strip_reasoning {
            for msg in messages.iter_mut() {
                msg.reasoning_content = None;
            }
        }
        Ok(())
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        workspace_root: &Path,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, ProviderError> {
        let model = if model.is_empty() {
            self.model.clone()
        } else {
            model.to_string()
        };

        let body = openai_request_body(
            messages,
            tools,
            &model,
            self.max_tokens,
            self.temperature,
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

        Ok(spawn_openai_stream(response, self.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_content_is_serialized_by_default() {
        let messages = vec![
            Message::user("hello"),
            Message::assistant("response").with_reasoning("I was thinking...".into()),
            Message::tool("call_1", "result"),
        ];

        let out = to_openai_messages(&messages, std::path::Path::new(".")).expect("messages");
        assert_eq!(out.len(), 3);

        // By default (non-DeepSeek providers), reasoning_content IS serialized
        // because some models require it for multi-turn reasoning continuity.
        let assistant = &out[1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(
            assistant["reasoning_content"].as_str().unwrap(),
            "I was thinking..."
        );
        assert_eq!(assistant["content"], "response");
    }
}
