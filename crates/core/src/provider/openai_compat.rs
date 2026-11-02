use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use futures_util::StreamExt;
use nca_common::message::{ContentPart, Message, MessageContent, Role};
use nca_common::tool::{ToolCall, ToolDefinition};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::{ProviderError, StreamChunk};

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
        let mut buffer: Vec<u8> = Vec::new();
        let mut tool_calls: BTreeMap<u64, ToolCallAccumulator> = BTreeMap::new();
        let mut finish_reason: Option<String> = None;

        while let Some(item) = byte_stream.next().await {
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(err) => {
                    // Half-received tool calls are dropped so a broken stream never runs them.
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

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if !line.starts_with("data:") {
                    continue;
                }

                let data = line["data:".len()..].trim();
                if data == "[DONE]" {
                    finish_openai_stream(&tx, &mut tool_calls, finish_reason).await;
                    return;
                }

                let Ok(event) = serde_json::from_str::<Value>(data) else {
                    continue;
                };

                // Mid-stream error objects end the response. Calls still pending are dropped.
                if let Some(error) = event.get("error").filter(|error| !error.is_null()) {
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .map_or_else(|| error.to_string(), str::to_string);
                    let _ = tx
                        .send(StreamChunk::Failed(format!(
                            "{provider_name} stream error: {message}"
                        )))
                        .await;
                    let _ = tx.send(StreamChunk::Done).await;
                    return;
                }

                if let Some(usage) = event.get("usage") {
                    let input_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
                    let output_tokens = usage["completion_tokens"].as_u64().unwrap_or(0);
                    if input_tokens > 0 || output_tokens > 0 {
                        let _ = tx
                            .send(StreamChunk::Usage {
                                input_tokens,
                                cached_input_tokens: cached_prompt_tokens(usage),
                                // OpenAI-style caches do not charge for writes.
                                cache_write_input_tokens: 0,
                                output_tokens,
                            })
                            .await;
                    }
                }

                let Some(choices) = event["choices"].as_array() else {
                    continue;
                };

                for choice in choices {
                    let delta = &choice["delta"];
                    if let Some(text) = reasoning_text(delta) {
                        let _ = tx.send(StreamChunk::ReasoningDelta(text.to_string())).await;
                    }

                    if let Some(text) = delta["content"].as_str()
                        && !text.is_empty()
                    {
                        let _ = tx.send(StreamChunk::TextDelta(text.to_string())).await;
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

                    // Some servers send "" instead of null on non-final chunks.
                    if let Some(reason) = choice["finish_reason"]
                        .as_str()
                        .filter(|reason| !reason.is_empty())
                    {
                        if reason == "tool_calls" {
                            flush_openai_tool_calls(&tx, &mut tool_calls).await;
                        }
                        finish_reason.get_or_insert_with(|| reason.to_string());
                    }
                }
            }
        }

        finish_openai_stream(&tx, &mut tool_calls, finish_reason).await;
    });

    rx
}

/// Prompt tokens served from cache. OpenAI-style servers report `prompt_tokens_details.cached_tokens`;
/// DeepSeek reports `prompt_cache_hit_tokens`. Either is a subset of `prompt_tokens`.
fn cached_prompt_tokens(usage: &Value) -> u64 {
    usage["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .or_else(|| usage["prompt_cache_hit_tokens"].as_u64())
        .unwrap_or(0)
}

/// DeepSeek sends the same text in `reasoning_content` and `reasoning`; take only one.
fn reasoning_text(delta: &Value) -> Option<&str> {
    ["reasoning_content", "reasoning"]
        .into_iter()
        .filter_map(|key| delta[key].as_str())
        .find(|text| !text.is_empty())
}

/// End of a healthy stream: flush pending tool calls, report why the model stopped, then `Done`.
async fn finish_openai_stream(
    tx: &tokio::sync::mpsc::Sender<StreamChunk>,
    tool_calls: &mut BTreeMap<u64, ToolCallAccumulator>,
    finish_reason: Option<String>,
) {
    flush_openai_tool_calls(tx, tool_calls).await;
    if let Some(reason) = finish_reason {
        let _ = tx.send(StreamChunk::Finish { reason }).await;
    }
    let _ = tx.send(StreamChunk::Done).await;
}

/// Map a non-success HTTP response to a typed error. Shared by every provider wire format.
pub fn map_provider_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body_text: String,
) -> ProviderError {
    match status.as_u16() {
        401 | 403 => ProviderError::AuthError(body_text),
        404 => ProviderError::ModelNotFound(body_text),
        429 => ProviderError::RateLimited {
            retry_after_ms: retry_after_ms(headers),
        },
        408 | 500..=599 => ProviderError::Transient(format!("HTTP {status}: {body_text}")),
        _ => ProviderError::RequestFailed(body_text),
    }
}

/// Map a failure to send the request or receive the response head.
/// Timeouts and connection problems are worth retrying; anything else is not.
pub fn map_send_error(err: reqwest::Error) -> ProviderError {
    let message = error_chain(&err);
    if err.is_timeout() || err.is_connect() || err.is_request() || err.is_body() {
        ProviderError::Transient(message)
    } else {
        ProviderError::RequestFailed(message)
    }
}

/// `Retry-After` given as integer seconds, in milliseconds. HTTP-dates and junk use the default.
fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> u64 {
    const DEFAULT_RETRY_AFTER_MS: u64 = 2000;
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map_or(DEFAULT_RETRY_AFTER_MS, |seconds| {
            seconds.saturating_mul(1000)
        })
}

/// `reqwest::Error` displays without its cause ("error sending request for url ..."), so append
/// the source chain to say why.
pub(super) fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !message.contains(&cause_text) {
            message.push_str(": ");
            message.push_str(&cause_text);
        }
        source = cause.source();
    }
    message
}

/// Build the chunk for a finished streamed tool call. Empty arguments mean "no parameters";
/// arguments that are not JSON are reported so the model can retry the call.
pub(super) fn tool_call_chunk(id: String, name: String, raw_arguments: String) -> StreamChunk {
    let parsed = if raw_arguments.trim().is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str::<Value>(&raw_arguments)
    };

    match parsed {
        Ok(input) => StreamChunk::ToolUse(ToolCall { id, name, input }),
        Err(err) => StreamChunk::InvalidToolUse {
            call_id: id,
            name,
            raw_arguments,
            error: err.to_string(),
        },
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

        let id = if call.id.is_empty() {
            format!("tool-call-{index}")
        } else {
            call.id
        };
        let _ = tx
            .send(tool_call_chunk(id, call.name, call.arguments))
            .await;
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

                // DeepSeek thinking mode expects the reasoning of tool-call turns echoed back.
                if let Some(reasoning) = message
                    .reasoning_content
                    .as_deref()
                    .filter(|text| !text.is_empty())
                {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::test_support::{
        chunk_kinds, collect_chunks, spawn_sse_server, spawn_truncated_sse_server, test_client,
    };
    use nca_common::message::MessageToolCall;
    use reqwest::StatusCode;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

    /// Wrap each payload as one SSE `data:` event.
    fn sse(payloads: &[&str]) -> String {
        payloads
            .iter()
            .map(|payload| format!("data: {payload}\n\n"))
            .collect()
    }

    async fn stream_chunks(body: String) -> Vec<StreamChunk> {
        let base_url = spawn_sse_server(body, 200, |_| {});
        let response = test_client().get(&base_url).send().await.expect("response");
        collect_chunks(spawn_openai_stream(response, "test")).await
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

    fn headers(pairs: &[(reqwest::header::HeaderName, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(name.clone(), HeaderValue::from_str(value).expect("header"));
        }
        map
    }

    #[tokio::test]
    async fn stream_read_error_yields_failed_and_never_assistant_text() {
        // The connection drops mid-body while a tool call is still being assembled.
        let base_url = spawn_truncated_sse_server(sse(&[
            r#"{"choices":[{"delta":{"content":"partial"},"index":0,"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"lookup","arguments":"{\"path\":"}}]}}]}"#,
        ]));
        let response = test_client().get(&base_url).send().await.expect("response");

        let chunks = collect_chunks(spawn_openai_stream(response, "test")).await;

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
    async fn finish_reason_length_with_empty_content_yields_finish() {
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[{"delta":{"content":""},"index":0,"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{"content":""},"index":0,"finish_reason":"length"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":300}}"#,
            "[DONE]",
        ]))
        .await;

        assert_eq!(chunk_kinds(&chunks), ["usage", "finish", "done"]);
        assert!(matches!(&chunks[1], StreamChunk::Finish { reason } if reason == "length"));
    }

    #[tokio::test]
    async fn finish_reason_is_reported_once_and_unchanged() {
        for reason in ["stop", "length", "tool_calls", "content_filter"] {
            let finish =
                format!(r#"{{"choices":[{{"delta":{{}},"index":0,"finish_reason":"{reason}"}}]}}"#);
            let chunks = stream_chunks(sse(&[
                r#"{"choices":[{"delta":{"content":"hi"},"index":0,"finish_reason":""}]}"#,
                finish.as_str(),
                finish.as_str(),
                "[DONE]",
            ]))
            .await;

            assert_eq!(finish_reasons(&chunks), [reason]);
            assert_eq!(chunk_kinds(&chunks), ["text", "finish", "done"]);
        }
    }

    #[tokio::test]
    async fn finish_reason_arrives_after_flushed_tool_calls_and_before_done() {
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"lookup","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}}"#,
            "[DONE]",
        ]))
        .await;

        assert_eq!(
            chunk_kinds(&chunks),
            ["tool_use", "usage", "finish", "done"]
        );
        assert_eq!(finish_reasons(&chunks), ["tool_calls"]);
    }

    #[tokio::test]
    async fn usage_reports_cached_prompt_tokens_in_either_wire_format() {
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[],"usage":{"prompt_tokens":2600,"completion_tokens":9,"prompt_tokens_details":{"cached_tokens":2555}}}"#,
            "[DONE]",
        ]))
        .await;
        assert!(matches!(
            &chunks[0],
            StreamChunk::Usage {
                input_tokens: 2600,
                cached_input_tokens: 2555,
                cache_write_input_tokens: 0,
                output_tokens: 9,
            }
        ));

        // DeepSeek names the same count `prompt_cache_hit_tokens`.
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":1,"prompt_cache_hit_tokens":64,"prompt_cache_miss_tokens":36}}"#,
            "[DONE]",
        ]))
        .await;
        assert!(matches!(
            &chunks[0],
            StreamChunk::Usage {
                input_tokens: 100,
                cached_input_tokens: 64,
                ..
            }
        ));

        // A server that reports no cache fields reads as zero cached tokens.
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#,
            "[DONE]",
        ]))
        .await;
        assert!(matches!(
            &chunks[0],
            StreamChunk::Usage {
                cached_input_tokens: 0,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn stream_without_done_marker_still_finishes_once() {
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[{"delta":{"content":"hi"},"index":0,"finish_reason":"stop"}]}"#,
        ]))
        .await;

        assert_eq!(chunk_kinds(&chunks), ["text", "finish", "done"]);
        assert_eq!(finish_reasons(&chunks), ["stop"]);
    }

    #[tokio::test]
    async fn reasoning_content_and_reasoning_on_one_delta_emit_one_reasoning_delta() {
        let chunks = stream_chunks(sse(&[
            // DeepSeek repeats the same text in both fields.
            r#"{"choices":[{"delta":{"reasoning_content":"think","reasoning":"think"}}]}"#,
            r#"{"choices":[{"delta":{"reasoning":"more"}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":"","reasoning":"fallback"}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":null,"content":"answer"}}]}"#,
            "[DONE]",
        ]))
        .await;

        let reasoning: Vec<&str> = chunks
            .iter()
            .filter_map(|chunk| match chunk {
                StreamChunk::ReasoningDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(reasoning, ["think", "more", "fallback"]);
        assert_eq!(
            chunk_kinds(&chunks),
            ["reasoning", "reasoning", "reasoning", "text", "done"]
        );
    }

    #[tokio::test]
    async fn tool_call_with_invalid_json_arguments_yields_invalid_tool_use() {
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_ok","function":{"name":"lookup","arguments":"{\"path\":\"src\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_bad","function":{"name":"lookup","arguments":"{\"path\": \"sr"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]))
        .await;

        assert_eq!(
            chunk_kinds(&chunks),
            ["tool_use", "invalid_tool_use", "finish", "done"]
        );
        assert!(
            matches!(&chunks[0], StreamChunk::ToolUse(call) if call.id == "call_ok" && call.input == json!({"path": "src"}))
        );
        match &chunks[1] {
            StreamChunk::InvalidToolUse {
                call_id,
                name,
                raw_arguments,
                error,
            } => {
                assert_eq!(call_id, "call_bad");
                assert_eq!(name, "lookup");
                assert_eq!(raw_arguments, r#"{"path": "sr"#);
                assert!(!error.is_empty());
            }
            other => panic!("expected InvalidToolUse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_or_missing_tool_arguments_mean_an_empty_object() {
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"list","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"list","arguments":"  \n"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":2,"id":"call_c","function":{"name":"list"}}]}}]}"#,
            "[DONE]",
        ]))
        .await;

        assert_eq!(
            chunk_kinds(&chunks),
            ["tool_use", "tool_use", "tool_use", "done"]
        );
        for (chunk, expected_id) in chunks.iter().zip(["call_a", "call_b", "call_c"]) {
            assert!(
                matches!(chunk, StreamChunk::ToolUse(call) if call.id == expected_id && call.name == "list" && call.input == json!({})),
                "unexpected chunk: {chunk:?}"
            );
        }
    }

    #[tokio::test]
    async fn tool_call_without_a_name_is_skipped() {
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_x","function":{"arguments":"{}"}}]}}]}"#,
            "[DONE]",
        ]))
        .await;

        assert_eq!(chunk_kinds(&chunks), ["done"]);
    }

    #[tokio::test]
    async fn tool_call_without_an_id_gets_a_positional_id() {
        let chunks = stream_chunks(sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":3,"function":{"name":"lookup","arguments":"{}"}},{"index":4,"function":{"name":"lookup","arguments":"{"}}]}}]}"#,
            "[DONE]",
        ]))
        .await;

        assert!(matches!(&chunks[0], StreamChunk::ToolUse(call) if call.id == "tool-call-3"));
        assert!(
            matches!(&chunks[1], StreamChunk::InvalidToolUse { call_id, .. } if call_id == "tool-call-4")
        );
    }

    #[test]
    fn assistant_reasoning_content_is_replayed_in_the_request_body() {
        let mut with_reasoning = Message::assistant_with_tool_calls(
            "",
            vec![MessageToolCall {
                id: "call_1".into(),
                name: "lookup".into(),
                arguments: json!({"path": "src"}),
            }],
        );
        with_reasoning.reasoning_content = Some("plan the lookup".into());
        let mut empty_reasoning = Message::assistant("done");
        empty_reasoning.reasoning_content = Some(String::new());
        let messages = vec![
            Message::user("find it"),
            with_reasoning,
            Message::tool("call_1", "found"),
            empty_reasoning,
            Message::assistant("plain"),
        ];

        let body = openai_request_body(&messages, &[], "model", 128, 0.0, Path::new("."))
            .expect("request body");
        let sent = body["messages"].as_array().expect("messages");

        assert_eq!(sent[1]["role"], "assistant");
        assert_eq!(sent[1]["reasoning_content"], "plan the lookup");
        assert_eq!(sent[1]["tool_calls"][0]["id"], "call_1");
        for index in [0, 2, 3, 4] {
            assert!(
                sent[index].get("reasoning_content").is_none(),
                "message {index} must not carry reasoning_content: {}",
                sent[index]
            );
        }
    }

    #[test]
    fn rate_limit_honors_retry_after_seconds() {
        let err = map_provider_error(
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "3")]),
            "slow down".into(),
        );

        assert!(matches!(
            err,
            ProviderError::RateLimited {
                retry_after_ms: 3000
            }
        ));
    }

    #[test]
    fn rate_limit_without_usable_retry_after_waits_two_seconds() {
        for value in [
            None,
            Some("soon"),
            Some("1.5"),
            Some("-4"),
            Some("Wed, 21 Oct 2026 07:28:00 GMT"),
        ] {
            let map = match value {
                Some(value) => headers(&[(RETRY_AFTER, value)]),
                None => HeaderMap::new(),
            };
            let err = map_provider_error(StatusCode::TOO_MANY_REQUESTS, &map, String::new());

            assert!(
                matches!(
                    err,
                    ProviderError::RateLimited {
                        retry_after_ms: 2000
                    }
                ),
                "Retry-After {value:?} gave {err:?}"
            );
        }
    }

    #[test]
    fn server_errors_and_request_timeouts_are_transient() {
        for code in [408u16, 500, 502, 503, 504, 529] {
            let status = StatusCode::from_u16(code).expect("status");
            let err = map_provider_error(status, &HeaderMap::new(), "upstream down".into());

            assert!(
                matches!(&err, ProviderError::Transient(message) if *message == format!("HTTP {status}: upstream down")),
                "{code} gave {err:?}"
            );
            assert!(err.is_retryable());
        }
    }

    #[test]
    fn client_errors_keep_their_typed_mapping() {
        let none = HeaderMap::new();
        let map = |status: StatusCode| map_provider_error(status, &none, "body".into());

        assert!(
            matches!(map(StatusCode::BAD_REQUEST), ProviderError::RequestFailed(body) if body == "body")
        );
        assert!(matches!(
            map(StatusCode::UNPROCESSABLE_ENTITY),
            ProviderError::RequestFailed(_)
        ));
        assert!(
            matches!(map(StatusCode::UNAUTHORIZED), ProviderError::AuthError(body) if body == "body")
        );
        assert!(matches!(
            map(StatusCode::FORBIDDEN),
            ProviderError::AuthError(_)
        ));
        assert!(
            matches!(map(StatusCode::NOT_FOUND), ProviderError::ModelNotFound(body) if body == "body")
        );
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::NOT_FOUND,
        ] {
            assert!(!map(status).is_retryable());
        }
    }

    #[tokio::test]
    async fn refused_connection_is_transient_and_names_the_cause() {
        // Bind to learn a free port, then release it so nothing is listening there.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .expect("addr")
            .port();
        let err = test_client()
            .get(format!("http://127.0.0.1:{port}"))
            .send()
            .await
            .expect_err("nothing is listening");

        let mapped = map_send_error(err);

        assert!(
            matches!(&mapped, ProviderError::Transient(message) if message.starts_with("error sending request") && message.contains("onnect")),
            "unexpected mapping: {mapped:?}"
        );
        assert!(mapped.is_retryable());
    }

    #[tokio::test]
    async fn invalid_request_url_is_not_retryable() {
        let err = test_client()
            .get("not a url")
            .send()
            .await
            .expect_err("invalid url");

        let mapped = map_send_error(err);

        assert!(matches!(mapped, ProviderError::RequestFailed(_)));
        assert!(!mapped.is_retryable());
    }

    #[derive(Debug)]
    struct Layered {
        message: &'static str,
        cause: Option<Box<Layered>>,
        repeat_cause_in_message: bool,
    }

    impl std::fmt::Display for Layered {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.message)?;
            match &self.cause {
                Some(cause) if self.repeat_cause_in_message => write!(f, ": {cause}"),
                _ => Ok(()),
            }
        }
    }

    impl std::error::Error for Layered {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.cause
                .as_deref()
                .map(|cause| cause as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn error_chain_appends_causes_without_repeating_them() {
        let root = Layered {
            message: "connection refused",
            cause: None,
            repeat_cause_in_message: false,
        };
        let plain = Layered {
            message: "error sending request",
            cause: Some(Box::new(root)),
            repeat_cause_in_message: false,
        };
        assert_eq!(
            error_chain(&plain),
            "error sending request: connection refused"
        );

        let root = Layered {
            message: "connection refused",
            cause: None,
            repeat_cause_in_message: false,
        };
        let verbose = Layered {
            message: "error sending request",
            cause: Some(Box::new(root)),
            repeat_cause_in_message: true,
        };
        assert_eq!(
            error_chain(&verbose),
            "error sending request: connection refused"
        );
    }
}
