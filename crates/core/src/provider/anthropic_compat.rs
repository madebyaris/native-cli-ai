use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use futures_util::StreamExt;
use nca_common::message::{ContentPart, Message, MessageContent, Role};
use nca_common::tool::{ToolCall, ToolDefinition};
use serde_json::{Value, json};

use super::{ProviderError, StreamChunk};

pub fn anthropic_request_body(
    messages: &[Message],
    tools: &[ToolDefinition],
    model: &str,
    max_tokens: u32,
    temperature: f32,
    workspace_root: &Path,
) -> Result<Value, ProviderError> {
    let (system, anthropic_messages) = to_anthropic_messages(messages, workspace_root)?;
    let tools = if tools.is_empty() {
        None
    } else {
        Some(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.parameters,
                    })
                })
                .collect::<Vec<_>>(),
        )
    };

    Ok(json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": anthropic_messages,
        "tools": tools,
        "stream": true,
        "temperature": temperature,
    }))
}

pub fn spawn_anthropic_stream(
    response: reqwest::Response,
    provider_name: &'static str,
) -> tokio::sync::mpsc::Receiver<StreamChunk> {
    let mut byte_stream = response.bytes_stream();
    let (tx, rx) = tokio::sync::mpsc::channel(64);

    tokio::spawn(async move {
        let mut buffer = String::new();
        let mut event_type = String::new();
        let mut tool_id = String::new();
        let mut tool_name = String::new();
        let mut tool_input = String::new();
        let mut input_tokens: u64 = 0;

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

                if line.is_empty() {
                    event_type.clear();
                    continue;
                }

                if let Some(event) = line.strip_prefix("event:") {
                    event_type = event.trim().to_string();
                    continue;
                }

                if !line.starts_with("data:") {
                    continue;
                }

                let data = line["data:".len()..].trim();
                if data == "[DONE]" {
                    break;
                }

                let Ok(event) = serde_json::from_str::<Value>(data) else {
                    continue;
                };

                match event_type.as_str() {
                    "message_start" => {
                        input_tokens = event["message"]["usage"]["input_tokens"]
                            .as_u64()
                            .unwrap_or(0);
                    }
                    "content_block_start" => {
                        let block = &event["content_block"];
                        if block["type"].as_str().unwrap_or("") == "tool_use" {
                            tool_id = block["id"].as_str().unwrap_or("").to_string();
                            tool_name = block["name"].as_str().unwrap_or("").to_string();
                            tool_input.clear();
                        }
                    }
                    "content_block_delta" => {
                        let delta = &event["delta"];
                        match delta["type"].as_str().unwrap_or("") {
                            "text_delta" => {
                                if let Some(text) = delta["text"].as_str()
                                    && !text.is_empty()
                                {
                                    let _ = tx.send(StreamChunk::TextDelta(text.to_string())).await;
                                }
                            }
                            "input_json_delta" => {
                                if let Some(partial) = delta["partial_json"].as_str() {
                                    tool_input.push_str(partial);
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_stop" => {
                        flush_anthropic_tool_call(
                            &tx,
                            &mut tool_id,
                            &mut tool_name,
                            &mut tool_input,
                        )
                        .await;
                    }
                    "message_delta" => {
                        let output_tokens = event["usage"]["output_tokens"].as_u64().unwrap_or(0);
                        if input_tokens > 0 || output_tokens > 0 {
                            let _ = tx
                                .send(StreamChunk::Usage {
                                    input_tokens,
                                    output_tokens,
                                    cache_creation_tokens: 0,
                                    cache_read_tokens: 0,
                                })
                                .await;
                            input_tokens = 0;
                        }
                    }
                    _ => {}
                }
            }
        }

        flush_anthropic_tool_call(&tx, &mut tool_id, &mut tool_name, &mut tool_input).await;
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

async fn flush_anthropic_tool_call(
    tx: &tokio::sync::mpsc::Sender<StreamChunk>,
    tool_id: &mut String,
    tool_name: &mut String,
    tool_input: &mut String,
) {
    if tool_name.is_empty() {
        return;
    }

    if let Ok(input) = serde_json::from_str(tool_input) {
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

fn tool_content_string(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Parts(_) => content.to_summary_text(),
    }
}

fn user_content_value(
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
                        let data = B64.encode(bytes);
                        blocks.push(json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": data,
                            }
                        }));
                    }
                }
            }
            Ok(Value::Array(blocks))
        }
    }
}

fn to_anthropic_messages(
    messages: &[Message],
    workspace_root: &Path,
) -> Result<(Option<String>, Vec<Value>), ProviderError> {
    let mut system_parts = Vec::new();
    let mut out = Vec::new();
    let mut i = 0;

    while i < messages.len() && messages[i].role == Role::System {
        match &messages[i].content {
            MessageContent::Text(t) => system_parts.push(t.clone()),
            MessageContent::Parts(_) => system_parts.push(messages[i].content.to_summary_text()),
        }
        i += 1;
    }

    while i < messages.len() {
        let message = &messages[i];
        match message.role {
            Role::User => {
                let content = user_content_value(&message.content, workspace_root)?;
                out.push(json!({
                    "role": "user",
                    "content": content,
                }));
                i += 1;
            }
            Role::Assistant => {
                let mut blocks = Vec::new();
                if let MessageContent::Text(t) = &message.content {
                    if !t.is_empty() {
                        blocks.push(json!({
                            "type": "text",
                            "text": t,
                        }));
                    }
                } else {
                    let v = user_content_value(&message.content, workspace_root)?;
                    if let Value::Array(arr) = v {
                        blocks.extend(arr);
                    }
                }
                if let Some(calls) = &message.tool_calls {
                    for call in calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": call.arguments,
                        }));
                    }
                }

                let content_out = if blocks.is_empty() {
                    match &message.content {
                        MessageContent::Text(t) => json!(t),
                        MessageContent::Parts(_) => {
                            user_content_value(&message.content, workspace_root)?
                        }
                    }
                } else {
                    Value::Array(blocks)
                };

                out.push(json!({
                    "role": "assistant",
                    "content": content_out,
                }));
                i += 1;
            }
            Role::Tool => {
                let mut results = Vec::new();
                while i < messages.len() && messages[i].role == Role::Tool {
                    let tool_message = &messages[i];
                    results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": tool_message.tool_call_id.as_deref().unwrap_or(""),
                        "content": tool_content_string(&tool_message.content),
                    }));
                    i += 1;
                }
                out.push(json!({
                    "role": "user",
                    "content": results,
                }));
            }
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

    Ok((system, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD as B64};
    use nca_common::message::{ContentPart, Message};
    use tempfile::tempdir;

    #[test]
    fn user_multimodal_message_serializes_image_base64_block() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        let rel = ".nca/sessions/s1/attachments/x.png";
        let att_dir = workspace.join(".nca/sessions/s1/attachments");
        std::fs::create_dir_all(&att_dir).unwrap();
        let png = B64
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
            .unwrap();
        std::fs::write(att_dir.join("x.png"), png).unwrap();

        let messages = vec![Message::user_with_parts(vec![
            ContentPart::Text {
                text: "describe".into(),
            },
            ContentPart::Image {
                media_type: "image/png".into(),
                path: rel.into(),
            },
        ])];

        let body = anthropic_request_body(&messages, &[], "MiniMax-M2.5", 128, 1.0, workspace)
            .expect("body");
        let content = body["messages"][0]["content"].as_array().expect("array");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert!(content[1]["source"]["data"].as_str().unwrap().len() > 8);
    }
}
