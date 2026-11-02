use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use futures_util::StreamExt;
use nca_common::message::{ContentPart, Message, MessageContent, Role};
use nca_common::tool::ToolDefinition;
use serde_json::{Value, json};

use super::openai_compat::{error_chain, tool_call_chunk};
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
        let mut buffer: Vec<u8> = Vec::new();
        let mut event_type = String::new();
        let mut tool_id = String::new();
        let mut tool_name = String::new();
        let mut tool_input = String::new();
        let mut input_tokens: u64 = 0;
        let mut cached_input_tokens: u64 = 0;
        let mut cache_write_input_tokens: u64 = 0;
        let mut finish_reason: Option<String> = None;

        while let Some(item) = byte_stream.next().await {
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(err) => {
                    // A half-received tool call is dropped so a broken stream never runs it.
                    let _ = tx
                        .send(StreamChunk::Failed(format!(
                            "{provider_name} stream error: {}",
                            error_chain(&err)
                        )))
                        .await;
                    let _ = tx.send(StreamChunk::Done).await;
                    return;
                }
            };

            buffer.extend_from_slice(&chunk);

            // Split on the newline byte before decoding, so a multi-byte character that
            // straddles two network chunks is decoded whole.
            while let Some(nl) = buffer.iter().position(|&byte| byte == b'\n') {
                let line_bytes: Vec<u8> = buffer.drain(..=nl).collect();
                let raw = String::from_utf8_lossy(&line_bytes).into_owned();
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
                    "error" => {
                        // Overloaded and other server-side failures arrive mid-stream as `event: error`.
                        let message = event["error"]["message"]
                            .as_str()
                            .map_or_else(|| event["error"].to_string(), str::to_string);
                        let _ = tx
                            .send(StreamChunk::Failed(format!(
                                "{provider_name} stream error: {message}"
                            )))
                            .await;
                        let _ = tx.send(StreamChunk::Done).await;
                        return;
                    }
                    "message_start" => {
                        let usage = &event["message"]["usage"];
                        // Anthropic's `input_tokens` excludes cache reads and writes, which are
                        // reported beside it. Add them back so `input_tokens` is the whole prompt.
                        let uncached = usage["input_tokens"].as_u64().unwrap_or(0);
                        cached_input_tokens =
                            usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                        cache_write_input_tokens =
                            usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                        input_tokens = uncached + cached_input_tokens + cache_write_input_tokens;
                    }
                    "content_block_start" => {
                        let block = &event["content_block"];
                        match block["type"].as_str().unwrap_or("") {
                            "tool_use" => {
                                tool_id = block["id"].as_str().unwrap_or("").to_string();
                                tool_name = block["name"].as_str().unwrap_or("").to_string();
                                tool_input.clear();
                            }
                            "thinking" => {
                                if let Some(text) = block["thinking"].as_str()
                                    && !text.is_empty()
                                {
                                    let _ = tx
                                        .send(StreamChunk::ReasoningDelta(text.to_string()))
                                        .await;
                                }
                            }
                            _ => {}
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
                            // Thinking signatures (`signature_delta`) are not replayed, so skip them.
                            "thinking_delta" => {
                                if let Some(text) = delta["thinking"].as_str()
                                    && !text.is_empty()
                                {
                                    let _ = tx
                                        .send(StreamChunk::ReasoningDelta(text.to_string()))
                                        .await;
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
                        if let Some(reason) = event["delta"]["stop_reason"]
                            .as_str()
                            .filter(|reason| !reason.is_empty())
                        {
                            finish_reason.get_or_insert_with(|| normalize_stop_reason(reason));
                        }

                        let output_tokens = event["usage"]["output_tokens"].as_u64().unwrap_or(0);
                        if input_tokens > 0 || output_tokens > 0 {
                            let _ = tx
                                .send(StreamChunk::Usage {
                                    input_tokens,
                                    cached_input_tokens,
                                    cache_write_input_tokens,
                                    output_tokens,
                                })
                                .await;
                            input_tokens = 0;
                            cached_input_tokens = 0;
                            cache_write_input_tokens = 0;
                        }
                    }
                    _ => {}
                }
            }
        }

        flush_anthropic_tool_call(&tx, &mut tool_id, &mut tool_name, &mut tool_input).await;
        if let Some(reason) = finish_reason {
            let _ = tx.send(StreamChunk::Finish { reason }).await;
        }
        let _ = tx.send(StreamChunk::Done).await;
    });

    rx
}

/// Same status mapping as the OpenAI wire format; the HTTP layer does not differ.
pub fn map_provider_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body_text: String,
) -> ProviderError {
    super::openai_compat::map_provider_error(status, headers, body_text)
}

/// Map Anthropic stop reasons onto the OpenAI vocabulary; unknown values pass through.
fn normalize_stop_reason(reason: &str) -> String {
    match reason {
        "end_turn" | "stop_sequence" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        other => other,
    }
    .to_string()
}

async fn flush_anthropic_tool_call(
    tx: &tokio::sync::mpsc::Sender<StreamChunk>,
    tool_id: &mut String,
    tool_name: &mut String,
    tool_input: &mut String,
) {
    let id = std::mem::take(tool_id);
    let name = std::mem::take(tool_name);
    let raw_arguments = std::mem::take(tool_input);
    if name.is_empty() {
        return;
    }

    let _ = tx.send(tool_call_chunk(id, name, raw_arguments)).await;
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
    use crate::provider::test_support::{
        chunk_kinds, collect_chunks, spawn_sse_server, spawn_truncated_sse_server, test_client,
    };
    use base64::{Engine, engine::general_purpose::STANDARD as B64};
    use nca_common::message::{ContentPart, Message};
    use reqwest::StatusCode;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
    use tempfile::tempdir;

    /// Wrap each `(event name, payload)` pair as one SSE event.
    fn sse(events: &[(&str, &str)]) -> String {
        events
            .iter()
            .map(|(name, payload)| format!("event: {name}\ndata: {payload}\n\n"))
            .collect()
    }

    async fn stream_chunks(body: String) -> Vec<StreamChunk> {
        let base_url = spawn_sse_server(body, 200, |_| {});
        let response = test_client().get(&base_url).send().await.expect("response");
        collect_chunks(spawn_anthropic_stream(response, "test")).await
    }

    fn finish_reasons(chunks: &[StreamChunk]) -> Vec<&str> {
        chunks
            .iter()
            .filter_map(|chunk| match chunk {
                StreamChunk::Finish { reason } => Some(reason.as_str()),
                _ => None,
            })
            .collect()
    }

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

    #[test]
    fn assistant_reasoning_is_not_sent_to_anthropic() {
        let mut assistant = Message::assistant("answer");
        assistant.reasoning_content = Some("private chain of thought".into());
        let messages = vec![Message::user("question"), assistant];

        let body = anthropic_request_body(&messages, &[], "model", 128, 1.0, Path::new("."))
            .expect("request body");

        let sent = body.to_string();
        assert!(!sent.contains("reasoning_content"), "{sent}");
        assert!(!sent.contains("private chain of thought"), "{sent}");
    }

    #[tokio::test]
    async fn stream_read_error_yields_failed_and_never_assistant_text() {
        // The connection drops mid-body while a tool call is still being assembled.
        let base_url = spawn_truncated_sse_server(sse(&[
            (
                "content_block_delta",
                r#"{"delta":{"type":"text_delta","text":"partial"}}"#,
            ),
            (
                "content_block_start",
                r#"{"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            ),
        ]));
        let response = test_client().get(&base_url).send().await.expect("response");

        let chunks = collect_chunks(spawn_anthropic_stream(response, "test")).await;

        assert_eq!(chunk_kinds(&chunks), ["text", "failed", "done"]);
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "partial"));
        assert!(
            matches!(&chunks[1], StreamChunk::Failed(message) if message.starts_with("test stream error")),
            "unexpected failure message: {:?}",
            chunks[1]
        );
        assert!(!chunks.iter().any(
            |chunk| matches!(chunk, StreamChunk::TextDelta(text) if text.contains("stream error"))
        ));
    }

    #[tokio::test]
    async fn stop_reason_max_tokens_yields_finish_length() {
        let chunks = stream_chunks(sse(&[
            (
                "message_start",
                r#"{"message":{"usage":{"input_tokens":9}}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"text_delta","text":"cut off"}}"#,
            ),
            (
                "message_delta",
                r#"{"delta":{"stop_reason":"max_tokens","stop_sequence":null},"usage":{"output_tokens":300}}"#,
            ),
            ("message_stop", "{}"),
        ]))
        .await;

        assert_eq!(chunk_kinds(&chunks), ["text", "usage", "finish", "done"]);
        assert!(matches!(&chunks[2], StreamChunk::Finish { reason } if reason == "length"));
    }

    #[tokio::test]
    async fn usage_counts_cache_reads_and_writes_as_part_of_the_prompt() {
        let chunks = stream_chunks(sse(&[
            (
                "message_start",
                r#"{"message":{"usage":{"input_tokens":9,"cache_read_input_tokens":4000,"cache_creation_input_tokens":300}}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"text_delta","text":"ok"}}"#,
            ),
            (
                "message_delta",
                r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#,
            ),
            ("message_stop", "{}"),
        ]))
        .await;

        assert_eq!(chunk_kinds(&chunks), ["text", "usage", "finish", "done"]);
        assert!(matches!(
            &chunks[1],
            StreamChunk::Usage {
                input_tokens: 4309,
                cached_input_tokens: 4000,
                cache_write_input_tokens: 300,
                output_tokens: 12,
            }
        ));
    }

    #[tokio::test]
    async fn tool_use_stop_reason_follows_the_tool_call() {
        let chunks = stream_chunks(sse(&[
            (
                "content_block_start",
                r#"{"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"src\"}"}}"#,
            ),
            ("content_block_stop", "{}"),
            (
                "message_delta",
                r#"{"delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
            ),
        ]))
        .await;

        assert_eq!(
            chunk_kinds(&chunks),
            ["tool_use", "usage", "finish", "done"]
        );
        assert_eq!(finish_reasons(&chunks), ["tool_calls"]);
    }

    #[test]
    fn stop_reasons_are_normalized_to_the_openai_vocabulary() {
        for (anthropic, expected) in [
            ("end_turn", "stop"),
            ("max_tokens", "length"),
            ("tool_use", "tool_calls"),
            ("stop_sequence", "stop"),
            ("pause_turn", "pause_turn"),
            ("refusal", "refusal"),
        ] {
            assert_eq!(normalize_stop_reason(anthropic), expected);
        }
    }

    #[tokio::test]
    async fn thinking_blocks_stream_as_reasoning_and_signatures_are_ignored() {
        let chunks = stream_chunks(sse(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"thinking_delta","thinking":"Let me "}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"thinking_delta","thinking":"think."}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"signature_delta","signature":"EqQBCgIYAhIM"}}"#,
            ),
            ("content_block_stop", r#"{"index":0}"#),
            (
                "content_block_start",
                r#"{"index":1,"content_block":{"type":"thinking","thinking":"Second block. "}}"#,
            ),
            ("content_block_stop", r#"{"index":1}"#),
            (
                "content_block_start",
                r#"{"index":2,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":2,"delta":{"type":"text_delta","text":"Answer"}}"#,
            ),
            ("content_block_stop", r#"{"index":2}"#),
        ]))
        .await;

        let reasoning: Vec<&str> = chunks
            .iter()
            .filter_map(|chunk| match chunk {
                StreamChunk::ReasoningDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(reasoning, ["Let me ", "think.", "Second block. "]);
        assert_eq!(
            chunk_kinds(&chunks),
            ["reasoning", "reasoning", "reasoning", "text", "done"]
        );
    }

    #[tokio::test]
    async fn tool_call_cut_off_by_max_tokens_yields_invalid_tool_use() {
        let chunks = stream_chunks(sse(&[
            (
                "content_block_start",
                r#"{"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"input_json_delta","partial_json":"{\"path\": \"sr"}}"#,
            ),
            ("content_block_stop", "{}"),
            (
                "message_delta",
                r#"{"delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":64}}"#,
            ),
        ]))
        .await;

        assert_eq!(
            chunk_kinds(&chunks),
            ["invalid_tool_use", "usage", "finish", "done"]
        );
        match &chunks[0] {
            StreamChunk::InvalidToolUse {
                call_id,
                name,
                raw_arguments,
                error,
            } => {
                assert_eq!(call_id, "toolu_1");
                assert_eq!(name, "lookup");
                assert_eq!(raw_arguments, r#"{"path": "sr"#);
                assert!(!error.is_empty());
            }
            other => panic!("expected InvalidToolUse, got {other:?}"),
        }
        assert_eq!(finish_reasons(&chunks), ["length"]);
    }

    #[tokio::test]
    async fn tool_call_left_open_at_end_of_stream_is_reported() {
        let chunks = stream_chunks(sse(&[
            (
                "content_block_start",
                r#"{"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            ),
        ]))
        .await;

        assert_eq!(chunk_kinds(&chunks), ["invalid_tool_use", "done"]);
    }

    #[tokio::test]
    async fn empty_tool_input_yields_an_empty_object() {
        let chunks = stream_chunks(sse(&[
            (
                "content_block_start",
                r#"{"content_block":{"type":"tool_use","id":"toolu_1","name":"list"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"input_json_delta","partial_json":""}}"#,
            ),
            ("content_block_stop", "{}"),
            (
                "content_block_start",
                r#"{"content_block":{"type":"tool_use","id":"toolu_2","name":"list"}}"#,
            ),
            ("content_block_stop", "{}"),
        ]))
        .await;

        assert_eq!(chunk_kinds(&chunks), ["tool_use", "tool_use", "done"]);
        for (chunk, expected_id) in chunks.iter().zip(["toolu_1", "toolu_2"]) {
            assert!(
                matches!(chunk, StreamChunk::ToolUse(call) if call.id == expected_id && call.name == "list" && call.input == json!({})),
                "unexpected chunk: {chunk:?}"
            );
        }
    }

    #[test]
    fn map_provider_error_applies_the_shared_status_rules() {
        let mut retry_after = HeaderMap::new();
        retry_after.insert(RETRY_AFTER, HeaderValue::from_static("3"));
        let none = HeaderMap::new();

        assert!(matches!(
            map_provider_error(StatusCode::TOO_MANY_REQUESTS, &retry_after, String::new()),
            ProviderError::RateLimited {
                retry_after_ms: 3000
            }
        ));
        assert!(matches!(
            map_provider_error(StatusCode::TOO_MANY_REQUESTS, &none, String::new()),
            ProviderError::RateLimited {
                retry_after_ms: 2000
            }
        ));
        assert!(matches!(
            map_provider_error(StatusCode::SERVICE_UNAVAILABLE, &none, "busy".into()),
            ProviderError::Transient(message) if message == "HTTP 503 Service Unavailable: busy"
        ));
        assert!(matches!(
            map_provider_error(StatusCode::BAD_REQUEST, &none, "bad".into()),
            ProviderError::RequestFailed(body) if body == "bad"
        ));
        assert!(matches!(
            map_provider_error(StatusCode::UNAUTHORIZED, &none, "nope".into()),
            ProviderError::AuthError(body) if body == "nope"
        ));
        assert!(matches!(
            map_provider_error(StatusCode::NOT_FOUND, &none, "gone".into()),
            ProviderError::ModelNotFound(body) if body == "gone"
        ));
    }
}
