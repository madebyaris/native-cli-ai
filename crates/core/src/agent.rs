use nca_common::event::{AgentEvent, BusyState};
use nca_common::message::{ContentPart, ImageAttachment, Message, MessageToolCall, Role};
use nca_common::tool::{ToolCall, ToolDefinition};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::approval::ApprovalPolicy;
use crate::cost::CostTracker;
use crate::hooks::{HookEventKind, HookRunner};
use crate::provider::{Provider, ProviderError, StreamChunk};
use crate::tool_pipeline;
use crate::tools::ToolRegistry;

/// Drives the multi-turn conversation and tool-use loop.
pub struct AgentLoop {
    pub provider: Box<dyn Provider>,
    pub tools: ToolRegistry,
    pub approval: ApprovalPolicy,
    pub messages: Vec<Message>,
    pub model: String,
    pub cost_tracker: CostTracker,
    event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    max_turns: u32,
    max_tool_calls_per_turn: u32,
    checkpoint_interval: u32,
    cancel_flag: Arc<AtomicBool>,
    hooks: Option<HookRunner>,
    /// Start instant per pending tool call_id, for duration tracking.
    tool_start_times: HashMap<String, Instant>,
}

impl AgentLoop {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Box<dyn Provider>,
        tools: ToolRegistry,
        approval: ApprovalPolicy,
        model: String,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
        max_turns: u32,
        max_tool_calls_per_turn: u32,
        checkpoint_interval: u32,
        hooks: Option<HookRunner>,
    ) -> Self {
        Self {
            provider,
            tools,
            approval,
            messages: Vec::new(),
            model,
            cost_tracker: CostTracker::default(),
            event_tx,
            max_turns,
            max_tool_calls_per_turn,
            checkpoint_interval,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            hooks,
            tool_start_times: HashMap::new(),
        }
    }

    /// Add a system prompt once at startup.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.messages.push(Message::system(prompt));
    }

    /// Replace the LLM provider (e.g. after user switches provider in-session).
    pub fn replace_provider(&mut self, provider: Box<dyn Provider>) {
        self.provider = provider;
    }

    /// Run one turn: send messages to the provider, execute any tool calls,
    /// and repeat until the provider returns a final text response.
    pub async fn run_turn(
        &mut self,
        user_input: &str,
        workspace_root: &Path,
        attachments: &[ImageAttachment],
    ) -> Result<String, ProviderError> {
        let turn_start = Instant::now();
        self.cancel_flag.store(false, Ordering::SeqCst);
        let user_msg = if attachments.is_empty() {
            Message::user(user_input)
        } else {
            let mut parts: Vec<ContentPart> = Vec::new();
            let trimmed = user_input.trim();
            if !trimmed.is_empty() {
                parts.push(ContentPart::Text {
                    text: user_input.to_string(),
                });
            } else {
                parts.push(ContentPart::Text {
                    text: "(See attached image(s).)".into(),
                });
            }
            for a in attachments {
                parts.push(ContentPart::Image {
                    media_type: a.media_type.clone(),
                    path: a.path.clone(),
                });
            }
            Message::user_with_parts(parts)
        };
        let preview = user_msg.event_preview();
        self.messages.push(user_msg);
        self.emit(AgentEvent::MessageReceived {
            role: "user".into(),
            content: preview,
        })
        .await;

        let result = self.run_turn_inner(workspace_root, attachments).await;

        // On failure, remove the user message we just pushed so the message
        // history isn't left in a corrupted state (consecutive user messages
        // with no assistant reply confuses providers and causes repeated
        // empty responses).
        if result.is_err() {
            self.messages.pop();
        }

        self.emit(AgentEvent::TurnCompleted {
            duration_ms: turn_start.elapsed().as_millis() as u64,
        })
        .await;
        self.emit(AgentEvent::BusyStateChanged {
            state: BusyState::Idle,
        })
        .await;
        result
    }

    /// Inner loop for `run_turn`. Separated so the outer function can handle
    /// cleanup (message removal, busy-state reset) on all error paths.
    async fn run_turn_inner(
        &mut self,
        workspace_root: &Path,
        attachments: &[ImageAttachment],
    ) -> Result<String, ProviderError> {
        let mut turn = 0_u32;
        let mut empty_retries = 0_u32;
        let mut attachments_cleaned = attachments.is_empty();
        const MAX_EMPTY_RETRIES: u32 = 2;
        // Consecutive failures of the same tool — stops infinite retry loops.
        let mut consecutive_tool_failures: u32 = 0;
        let mut last_failed_tool: String = String::new();
        // Diagnostic details from the most recent failure (populated by the
        // `all_failed_same_tool` branch; only read when the max is reached).
        let mut last_failed_output: String = String::new();
        let mut last_failed_error: Option<String> = None;
        const MAX_CONSECUTIVE_TOOL_FAILURES: u32 = 3;

        let final_text = loop {
            if self.is_cancelled() {
                self.emit(AgentEvent::Error {
                    message: "Run cancelled".into(),
                })
                .await;
                return Err(ProviderError::Other("run cancelled".into()));
            }
            turn += 1;
            if turn > self.max_turns {
                let msg = format!("turn budget exceeded (max {})", self.max_turns);
                self.emit(AgentEvent::Error {
                    message: msg.clone(),
                })
                .await;
                return Err(ProviderError::Other(msg));
            }

            self.emit(AgentEvent::BusyStateChanged {
                state: BusyState::Thinking,
            })
            .await;
            self.emit(AgentEvent::Checkpoint {
                phase: "provider_request".into(),
                detail: format!("Starting model turn {turn}"),
                turn,
            })
            .await;
            self.provider
                .prepare_messages_for_request(&mut self.messages, workspace_root)
                .await?;
            // Repair any orphaned `tool_calls` left by a previously interrupted
            // turn (budget/pipeline error) so strict providers like DeepSeek don't
            // reject the request with "tool_calls must be followed by tool
            // messages". Persisted to `self.messages` so resumed sessions stay valid.
            sanitize_tool_call_pairs(&mut self.messages);
            let mut stream = self
                .provider
                .chat(
                    &self.messages,
                    &self.tool_definitions(),
                    &self.model,
                    workspace_root,
                )
                .await?;

            let mut assistant_text = String::new();
            let mut reasoning_text = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut got_usage = false;

            let mut cancel_poll = tokio::time::interval(Duration::from_millis(25));
            cancel_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let chunk = tokio::select! {
                    _ = cancel_poll.tick() => {
                        if self.is_cancelled() {
                            self.emit(AgentEvent::Error {
                                message: "Run cancelled while streaming model output".into(),
                            })
                            .await;
                            return Err(ProviderError::Other("run cancelled".into()));
                        }
                        continue;
                    }
                    chunk = stream.recv() => chunk,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                match chunk {
                    StreamChunk::TextDelta(delta) => {
                        if assistant_text.is_empty() {
                            self.emit(AgentEvent::BusyStateChanged {
                                state: BusyState::Streaming,
                            })
                            .await;
                        }
                        assistant_text.push_str(&delta);
                        self.emit(AgentEvent::TokensStreamed { delta }).await;
                    }
                    StreamChunk::ReasoningDelta(delta) => {
                        reasoning_text.push_str(&delta);
                        self.emit(AgentEvent::ReasoningStreamed { delta }).await;
                    }
                    StreamChunk::ToolUse(call) => {
                        self.tool_start_times
                            .insert(call.id.clone(), Instant::now());
                        self.emit(AgentEvent::ToolCallStarted {
                            call_id: call.id.clone(),
                            tool: call.name.clone(),
                            input: call.input.clone(),
                        })
                        .await;
                        tool_calls.push(call);
                    }
                    StreamChunk::Usage {
                        input_tokens,
                        output_tokens,
                        cache_creation_tokens,
                        cache_read_tokens,
                    } => {
                        got_usage = true;
                        self.cost_tracker.add(
                            input_tokens,
                            output_tokens,
                            cache_creation_tokens,
                            cache_read_tokens,
                        );
                        self.emit(AgentEvent::CostUpdated {
                            input_tokens: self.cost_tracker.input_tokens,
                            output_tokens: self.cost_tracker.output_tokens,
                            cache_read_tokens: self.cost_tracker.cache_read_tokens,
                            estimated_cost_usd: self.cost_tracker.estimated_cost_usd(),
                        })
                        .await;
                    }
                    StreamChunk::Done => break,
                }
            }

            if !attachments_cleaned {
                cleanup_processed_attachments(&mut self.messages, workspace_root, attachments);
                attachments_cleaned = true;
            }

            if tool_calls.is_empty() {
                if assistant_text.trim().is_empty() {
                    empty_retries += 1;
                    if empty_retries <= MAX_EMPTY_RETRIES && got_usage {
                        self.emit(AgentEvent::Error {
                            message: format!(
                                "Provider returned empty response (retry {empty_retries}/{MAX_EMPTY_RETRIES})"
                            ),
                        })
                        .await;
                        continue;
                    }
                    self.emit(AgentEvent::Error {
                        message: "Provider returned empty response with no tool calls".into(),
                    })
                    .await;
                    return Err(ProviderError::Other(
                        "Provider returned empty response with no tool calls after retries".into(),
                    ));
                }
                let mut msg = Message::assistant(assistant_text.clone());
                if !reasoning_text.is_empty() {
                    msg = msg.with_reasoning(std::mem::take(&mut reasoning_text));
                }
                self.messages.push(msg);
                self.emit(AgentEvent::MessageReceived {
                    role: "assistant".into(),
                    content: assistant_text.clone(),
                })
                .await;
                break assistant_text;
            }

            let replay_tool_calls = tool_calls
                .iter()
                .map(|call| MessageToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.input.clone(),
                })
                .collect();

            let mut msg = Message::assistant_with_tool_calls(assistant_text, replay_tool_calls);
            if !reasoning_text.is_empty() {
                msg = msg.with_reasoning(std::mem::take(&mut reasoning_text));
            }
            self.messages.push(msg);

            if tool_calls.len() as u32 > self.max_tool_calls_per_turn {
                return Err(ProviderError::Other(format!(
                    "tool-call budget exceeded in turn {turn} ({} > {})",
                    tool_calls.len(),
                    self.max_tool_calls_per_turn
                )));
            }

            // ── Tool pipeline: permission checks + concurrent execution ──
            let pipeline = tool_pipeline::run_tool_pipeline(
                &self.tools,
                &mut self.approval,
                &self.hooks,
                &self.event_tx,
                &self.cancel_flag,
                tool_calls.clone(),
            )
            .await
            .map_err(ProviderError::Other)?;

            let n = pipeline.results.len();

            // Checkpoint
            if self.checkpoint_interval > 0 && n as u32 >= self.checkpoint_interval {
                self.emit(AgentEvent::Checkpoint {
                    phase: "tool_execution".into(),
                    detail: format!("Executed {n} tool calls in turn {turn}"),
                    turn,
                })
                .await;
            }

            // Track consecutive failures of the same tool to detect infinite retry loops.
            let all_failed_same_tool = !pipeline.results.is_empty()
                && pipeline.results.iter().all(|r| !r.success)
                && tool_calls.len() == 1;
            if all_failed_same_tool {
                let tool_name = &tool_calls[0].name;
                let last_result = &pipeline.results[0];
                if *tool_name == last_failed_tool {
                    consecutive_tool_failures += 1;
                } else {
                    last_failed_tool = tool_name.clone();
                    consecutive_tool_failures = 1;
                }
                // Capture failure details for diagnostics and the final error message.
                last_failed_output = last_result.output.clone();
                last_failed_error = last_result.error.clone();
                tracing::warn!(
                    tool = %tool_name,
                    attempt = consecutive_tool_failures,
                    max_attempts = MAX_CONSECUTIVE_TOOL_FAILURES,
                    output = %truncate_str(&last_result.output, 500),
                    error = ?last_result.error,
                    "consecutive tool failure detected"
                );
            } else {
                consecutive_tool_failures = 0;
                last_failed_tool.clear();
            }

            for result in pipeline.results {
                let duration_ms = self
                    .tool_start_times
                    .remove(&result.call_id)
                    .map(|t| t.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                self.messages.push(Message::tool(
                    result.call_id.clone(),
                    format_tool_result(&result),
                ));
                self.emit(AgentEvent::ToolCallCompleted {
                    call_id: result.call_id.clone(),
                    output: result,
                    duration_ms,
                })
                .await;
            }

            if consecutive_tool_failures >= MAX_CONSECUTIVE_TOOL_FAILURES {
                let detail = last_failed_error
                    .as_deref()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| truncate_str(&last_failed_output, 300));
                let msg = format!(
                    "Tool `{}` failed {} times consecutively — stopping to avoid infinite loop.\n\nLast failure detail:\n{}",
                    last_failed_tool, consecutive_tool_failures, detail
                );
                self.emit(AgentEvent::Error {
                    message: msg.clone(),
                })
                .await;
                break msg;
            }
        };

        if self.cost_tracker.input_tokens == 0 && self.cost_tracker.output_tokens == 0 {
            let estimated_input = (self
                .messages
                .iter()
                .map(|message| message.content.approx_chars())
                .sum::<usize>()
                / 4) as u64;
            let estimated_output = (final_text.len() / 4) as u64;
            self.cost_tracker
                .add(estimated_input, estimated_output, 0, 0);
            self.emit(AgentEvent::CostUpdated {
                input_tokens: self.cost_tracker.input_tokens,
                output_tokens: self.cost_tracker.output_tokens,
                cache_read_tokens: self.cost_tracker.cache_read_tokens,
                estimated_cost_usd: self.cost_tracker.estimated_cost_usd(),
            })
            .await;
        }

        if let Some(hooks) = &self.hooks {
            let response_preview = truncate_str(&final_text, 300);
            hooks
                .run_best_effort(
                    HookEventKind::TurnComplete,
                    None,
                    &json!({
                        "response_preview": response_preview,
                    }),
                )
                .await;
        }

        Ok(final_text)
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.definitions()
    }

    async fn emit(&self, event: AgentEvent) {
        let _ = self.event_tx.send(event).await;
    }

    pub fn event_sender(&self) -> Option<tokio::sync::mpsc::Sender<AgentEvent>> {
        Some(self.event_tx.clone())
    }

    pub fn request_cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

/// Ensure every assistant message carrying `tool_calls` is followed by a
/// matching `tool` message for *each* `tool_call_id`.
///
/// A run interrupted mid-turn — after the assistant message was already pushed
/// but before its tools executed (turn/tool-call budget exceeded, pipeline
/// error, or cancellation) — can leave the history with an assistant message
/// whose `tool_calls` have no corresponding tool results. OpenAI-compatible
/// providers (DeepSeek especially) reject such sequences with:
/// "an assistant message with 'tool_calls' must be followed by tool messages".
///
/// This repairs the history in-place by injecting a synthetic error tool
/// message for every missing `tool_call_id`. The repair is persisted to
/// `self.messages`, so resumed sessions stay valid.
fn sanitize_tool_call_pairs(messages: &mut Vec<Message>) {
    const SYNTHETIC_RESULT: &str = "[tool execution interrupted — a budget or \
        pipeline limit was reached before this call ran; synthetic result \
        inserted to keep the message history valid for the provider]";

    let mut out: Vec<Message> = Vec::with_capacity(messages.len() + 4);
    let mut i = 0;
    while i < messages.len() {
        let expected: Option<Vec<String>> = if messages[i].role == Role::Assistant {
            messages[i]
                .tool_calls
                .as_ref()
                .filter(|calls| !calls.is_empty())
                .map(|calls| calls.iter().map(|c| c.id.clone()).collect())
        } else {
            None
        };

        out.push(messages[i].clone());

        if let Some(expected) = expected {
            let mut seen: HashSet<String> = HashSet::new();
            let mut j = i + 1;
            while j < messages.len() && messages[j].role == Role::Tool {
                if let Some(id) = messages[j].tool_call_id.as_ref()
                    && expected.iter().any(|e| e == id)
                {
                    seen.insert(id.clone());
                }
                out.push(messages[j].clone());
                j += 1;
            }
            for id in &expected {
                if !seen.contains(id) {
                    out.push(Message::tool(id.clone(), SYNTHETIC_RESULT));
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }

    *messages = out;
}

/// Truncate a string to `max_chars` characters, appending "…" if truncated.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

/// Maximum bytes for a single tool result before it enters the model's context.
/// ~32KB ≈ 8K tokens — enough for a full file read or a busy search, while
/// preventing one accidental "read this 5 MB log" from blowing the context
/// window before compaction runs.
const MAX_TOOL_OUTPUT_BYTES: usize = 32 * 1024;

/// Head/tail budget for tool output truncation.
const TOOL_TRUNCATE_HEAD: usize = 16 * 1024;
const TOOL_TRUNCATE_TAIL: usize = 16 * 1024;

/// Truncate a tool result string if it exceeds [`MAX_TOOL_OUTPUT_BYTES`].
/// Uses head+tail strategy to keep the beginning and end visible.
fn truncate_tool_output(output: &str) -> String {
    let byte_len = output.len();
    if byte_len <= MAX_TOOL_OUTPUT_BYTES {
        return output.to_string();
    }

    // Find safe UTF-8 boundaries for head and tail.
    let head_end = match output.is_char_boundary(TOOL_TRUNCATE_HEAD) {
        true => TOOL_TRUNCATE_HEAD,
        false => {
            let mut pos = TOOL_TRUNCATE_HEAD;
            while pos > 0 && !output.is_char_boundary(pos) {
                pos -= 1;
            }
            pos
        }
    };

    let tail_start = match output.is_char_boundary(byte_len - TOOL_TRUNCATE_TAIL) {
        true => byte_len - TOOL_TRUNCATE_TAIL,
        false => {
            let mut pos = byte_len - TOOL_TRUNCATE_TAIL;
            while pos < byte_len && !output.is_char_boundary(pos) {
                pos += 1;
            }
            pos
        }
    };

    let truncated_bytes = byte_len - (TOOL_TRUNCATE_HEAD + TOOL_TRUNCATE_TAIL);
    format!(
        "{}\n\n[... truncated {} bytes ({} → {}KB limit) ...]\n\n{}",
        &output[..head_end],
        truncated_bytes,
        byte_len / 1024,
        MAX_TOOL_OUTPUT_BYTES / 1024,
        &output[tail_start..]
    )
}

fn cleanup_processed_attachments(
    messages: &mut [Message],
    workspace_root: &Path,
    attachments: &[ImageAttachment],
) {
    let removed_paths: HashSet<String> = attachments.iter().map(|a| a.path.clone()).collect();
    if removed_paths.is_empty() {
        return;
    }

    for message in messages {
        let _ = message.content.strip_image_paths(&removed_paths);
    }

    for attachment in attachments {
        let full_path = workspace_root.join(&attachment.path);
        if let Err(err) = std::fs::remove_file(&full_path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "failed to remove processed image attachment {}: {}",
                full_path.display(),
                err
            );
        }
        if let Some(parent) = full_path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

fn format_tool_result(result: &nca_common::tool::ToolResult) -> String {
    let raw = if result.success {
        result.output.clone()
    } else {
        result
            .error
            .clone()
            .unwrap_or_else(|| "tool failed".to_string())
    };
    truncate_tool_output(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(id: &str) -> MessageToolCall {
        MessageToolCall {
            id: id.to_string(),
            name: "read".to_string(),
            arguments: json!({}),
        }
    }

    #[test]
    fn sanitize_is_noop_when_tool_pairs_complete() {
        let mut msgs = vec![
            Message::user("hi"),
            Message::assistant_with_tool_calls("checking", vec![tc("a"), tc("b")]),
            Message::tool("a", "ra"),
            Message::tool("b", "rb"),
        ];
        let before = msgs.clone();
        sanitize_tool_call_pairs(&mut msgs);
        assert_eq!(msgs, before, "complete pairs must be left untouched");
    }

    #[test]
    fn sanitize_fills_all_missing_tool_results() {
        // assistant emitted 2 tool_calls but a budget error fired before any ran.
        let mut msgs = vec![
            Message::assistant_with_tool_calls("checking", vec![tc("a"), tc("b")]),
            Message::user("continue"),
        ];
        sanitize_tool_call_pairs(&mut msgs);
        // [assistant, synthetic(a), synthetic(b), user]
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[1].role, Role::Tool);
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("a"));
        assert_eq!(msgs[2].role, Role::Tool);
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("b"));
        assert_eq!(msgs[3].role, Role::User);
    }

    #[test]
    fn sanitize_fills_only_missing_tool_results() {
        // 2 calls, only "a" got a result before interruption.
        let mut msgs = vec![
            Message::assistant_with_tool_calls("checking", vec![tc("a"), tc("b")]),
            Message::tool("a", "ra"),
            Message::assistant("done?"),
        ];
        sanitize_tool_call_pairs(&mut msgs);
        // [assistant+tc, tool(a), synthetic(b), assistant]
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("a"));
        assert_eq!(msgs[2].role, Role::Tool);
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("b"));
    }

    #[test]
    fn sanitize_repairs_trailing_orphaned_assistant() {
        // assistant with tool_calls at the very end, no results at all.
        let mut msgs = vec![
            Message::user("do it"),
            Message::assistant_with_tool_calls("running", vec![tc("x")]),
        ];
        sanitize_tool_call_pairs(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2].role, Role::Tool);
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("x"));
    }
}
