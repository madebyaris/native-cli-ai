use futures_util::future::join_all;
use nca_common::config::SmartCompactionMode;
use nca_common::event::{AgentEvent, BusyState};
use nca_common::message::{
    ContentPart, ImageAttachment, Message, MessageContent, MessageToolCall, Role,
};
use nca_common::tool::{PermissionTier, ToolCall, ToolDefinition, ToolResult};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::approval::{ApprovalPolicy, ApprovalVerdict};
use crate::context_view::{is_context_summary, plan_context_view_fitted};
use crate::cost::CostTracker;
use crate::hooks::{HookEventKind, HookRunner};
use crate::provider::{Provider, ProviderError, StreamChunk};
use crate::tools::ToolRegistry;

/// Attempts per model request: the first try plus three retries.
const MAX_PROVIDER_ATTEMPTS: u32 = 4;
const DEFAULT_RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const MAX_EMPTY_RETRIES: u32 = 2;
const MAX_CONSECUTIVE_TOOL_FAILURES: u32 = 3;
/// Identical tool calls in a row: nudge the model at the first limit, stop at the second.
const DOOM_LOOP_NUDGE_STREAK: u32 = 3;
const DOOM_LOOP_STOP_STREAK: u32 = 6;
/// Tools whose success means files changed and validation is owed.
const EDIT_TOOLS: &[&str] = &[
    "edit_file",
    "apply_patch",
    "write_file",
    "replace_match",
    "delete_path",
    "rename_path",
    "move_path",
    "copy_path",
];
const VALIDATION_TOOL: &str = "run_validation";
const MAX_RAW_ARGUMENTS_IN_ERROR: usize = 500;
const LENGTH_LIMIT_MESSAGE: &str = "Model stopped at its output token limit before producing a visible answer. Reasoning models can spend the whole budget on hidden reasoning; raise `model.max_tokens` in config or choose a non-reasoning model.";
const EMPTY_RESPONSE_NUDGE: &str = "Your last reply was empty. Continue the task: call a tool if you still need information or an action, otherwise give the final answer now.";
const VERIFICATION_NUDGE: &str = "You changed files in this turn but have not run a validation check since. Run the most relevant check with run_validation, such as the build or test command, then give the final answer. If no check can run here, say exactly why.";

fn is_interactive_tool(name: &str) -> bool {
    matches!(name, "ask_question")
}

/// A tool call whose arguments did not parse. It is answered with an error result, never executed.
#[derive(Debug, Clone)]
struct InvalidToolCall {
    call_id: String,
    name: String,
    raw_arguments: String,
    error: String,
}

/// Everything one model request produced once its stream finished.
#[derive(Debug, Default)]
struct ModelTurn {
    text: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    invalid_calls: Vec<InvalidToolCall>,
    finish_reason: Option<String>,
    /// Set when the stream failed before it completed.
    failure: Option<String>,
}

impl ModelTurn {
    /// True once anything reached the transcript, so a retry would repeat visible output.
    fn produced_visible_output(&self) -> bool {
        !self.text.is_empty() || !self.tool_calls.is_empty() || !self.invalid_calls.is_empty()
    }

    fn has_no_visible_output(&self) -> bool {
        self.text.trim().is_empty() && self.tool_calls.is_empty() && self.invalid_calls.is_empty()
    }
}

/// Harness-authored guidance. It reaches the model on the next request only and is never persisted.
fn harness_note(text: impl AsRef<str>) -> Message {
    Message::user(format!("[nca harness] {}", text.as_ref()))
}

/// What an approval prompt says. A dangerous shell command names the danger
/// and shows the command, so the user can decide without opening details.
fn approval_description(call: &ToolCall) -> String {
    let command = call.input.get("command").and_then(|c| c.as_str());
    if call.name == "execute_bash"
        && let Some(command) = command
        && let Some(reason) = crate::approval::dangerous_command_reason(command)
    {
        let shown: String = command.chars().take(200).collect();
        let more = if command.chars().count() > 200 {
            "…"
        } else {
            ""
        };
        return format!("Dangerous command ({reason}): {shown}{more}");
    }
    format!("Tool `{}` requires approval", call.name)
}

/// Error for a tool call cut off by the output token limit.
fn truncated_call_error(max_tokens: usize) -> String {
    let limit = if max_tokens > 0 {
        format!(" (`model.max_tokens` = {max_tokens})")
    } else {
        String::new()
    };
    format!(
        "the response reached the output token limit{limit}, so the arguments were cut off mid-stream. Do not repeat the same call: split the content into smaller pieces, e.g. write the first part with `write_file`, then add the rest with further edits"
    )
}

fn invalid_tool_result(call: &InvalidToolCall) -> ToolResult {
    let raw: String = call
        .raw_arguments
        .chars()
        .take(MAX_RAW_ARGUMENTS_IN_ERROR)
        .collect();
    ToolResult {
        call_id: call.call_id.clone(),
        success: false,
        output: String::new(),
        error: Some(format!(
            "Invalid arguments for tool `{}`: {}. The arguments received were: {raw}. Call the tool again with a JSON object that matches its parameter schema.",
            call.name, call.error
        )),
    }
}

/// Backoff for a retry: honor the provider's Retry-After, otherwise exponential with jitter.
fn retry_delay(base: Duration, err: &ProviderError, attempt: u32) -> Duration {
    match err {
        ProviderError::RateLimited { retry_after_ms } => {
            Duration::from_millis(*retry_after_ms).min(MAX_RETRY_DELAY)
        }
        _ => {
            let backoff = base.saturating_mul(2_u32.pow(attempt.saturating_sub(1).min(5)));
            let jitter = Duration::from_millis(retry_jitter_ms()).min(base / 4);
            (backoff + jitter).min(MAX_RETRY_DELAY)
        }
    }
}

/// Sub-second wall-clock noise, enough to spread retries without a random-number dependency.
fn retry_jitter_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::from(elapsed.subsec_millis()))
        .unwrap_or(0)
}

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
    /// Provider-request smart compaction (canonical history always kept).
    smart_compaction_mode: SmartCompactionMode,
    /// Hard ceiling on the provider request view, derived from the model window.
    /// `None` disables the cap; the canonical history is never modified.
    request_token_budget: Option<usize>,
    /// Leave this many tokens for the response when deriving the budget.
    reserved_output_tokens: usize,
    retry_base_delay: Duration,
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
            smart_compaction_mode: SmartCompactionMode::Off,
            request_token_budget: None,
            reserved_output_tokens: 0,
            retry_base_delay: DEFAULT_RETRY_BASE_DELAY,
        }
    }

    pub fn set_smart_compaction_mode(&mut self, mode: SmartCompactionMode) {
        self.smart_compaction_mode = mode;
    }

    pub fn smart_compaction_mode(&self) -> SmartCompactionMode {
        self.smart_compaction_mode
    }

    /// Set the hard input ceiling for provider requests.
    ///
    /// `None` disables the cap. The budget applies on every request even when
    /// smart compaction is off: overflowing the window is a hard provider error,
    /// so some eviction always beats failing the turn.
    pub fn set_request_token_budget(&mut self, budget: Option<usize>) {
        self.request_token_budget = budget;
    }

    pub fn request_token_budget(&self) -> Option<usize> {
        self.request_token_budget
    }

    /// Tokens held back from the budget for the model's reply.
    pub fn set_reserved_output_tokens(&mut self, tokens: usize) {
        self.reserved_output_tokens = tokens;
    }

    /// Set the system prompt. The leading system message is replaced, so
    /// refreshing the harness each turn (or resuming a session) cannot append a
    /// second prompt at the end of the history. System messages later in the
    /// history are preserved: auto-summarization stores its summary as one.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        let prompt = prompt.into();
        let replace_first = self
            .messages
            .first()
            .is_some_and(|message| message.role == Role::System && !is_context_summary(message));
        if replace_first {
            self.messages[0].content = MessageContent::Text(prompt);
        } else {
            self.messages.insert(0, Message::system(prompt));
        }
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

        let mut turn = 0_u32;
        let mut empty_retries = 0_u32;
        let mut attachments_cleaned = attachments.is_empty();
        // Consecutive failures of the same tool — stops infinite retry loops.
        let mut consecutive_tool_failures: u32 = 0;
        let mut last_failed_tool: String = String::new();
        // Identical tool calls in a row, for doom-loop detection.
        let mut last_call_key: Option<String> = None;
        let mut identical_streak: u32 = 0;
        // Set by a successful edit, cleared by a successful validation run.
        let mut edited_since_validation = false;
        let mut verification_nudged = false;
        // Harness note for the next request only. It reaches the model but never enters history.
        let mut pending_nudge: Option<Message> = None;

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
                break self.wrap_up_turn_budget(workspace_root, turn).await?;
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

            let mut request_messages = self.request_messages().await;
            if let Some(note) = pending_nudge.take() {
                request_messages.push(note);
            }
            let tools = self.tool_definitions();
            let mut model_turn = self
                .request_model_turn(&request_messages, &tools, workspace_root, turn)
                .await?;

            if !attachments_cleaned {
                cleanup_processed_attachments(&mut self.messages, workspace_root, attachments);
                attachments_cleaned = true;
            }

            if model_turn.has_no_visible_output() {
                // A length cutoff will repeat on an identical request, so fail now.
                if model_turn.finish_reason.as_deref() == Some("length") {
                    self.emit(AgentEvent::Error {
                        message: LENGTH_LIMIT_MESSAGE.into(),
                    })
                    .await;
                    return Err(ProviderError::Other(LENGTH_LIMIT_MESSAGE.into()));
                }
                empty_retries += 1;
                if empty_retries <= MAX_EMPTY_RETRIES {
                    self.emit(AgentEvent::Checkpoint {
                        phase: "provider_retry".into(),
                        detail: format!(
                            "Empty response — retry {empty_retries}/{MAX_EMPTY_RETRIES}"
                        ),
                        turn,
                    })
                    .await;
                    pending_nudge = Some(harness_note(EMPTY_RESPONSE_NUDGE));
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

            if model_turn.tool_calls.is_empty() && model_turn.invalid_calls.is_empty() {
                self.messages
                    .push(Message::assistant(model_turn.text.clone()));
                if edited_since_validation && !verification_nudged {
                    verification_nudged = true;
                    self.emit(AgentEvent::Checkpoint {
                        phase: "verification".into(),
                        detail:
                            "Files changed without a validation run; asking the model to verify"
                                .into(),
                        turn,
                    })
                    .await;
                    pending_nudge = Some(harness_note(VERIFICATION_NUDGE));
                    continue;
                }
                self.emit(AgentEvent::MessageReceived {
                    role: "assistant".into(),
                    content: model_turn.text.clone(),
                })
                .await;
                break model_turn.text;
            }

            let total_calls = model_turn.tool_calls.len() + model_turn.invalid_calls.len();
            if total_calls as u32 > self.max_tool_calls_per_turn {
                return Err(ProviderError::Other(format!(
                    "tool-call budget exceeded in turn {turn} ({total_calls} > {})",
                    self.max_tool_calls_per_turn
                )));
            }

            if model_turn.finish_reason.as_deref() == Some("length") {
                // The arguments did not fail to parse because the model wrote
                // bad JSON: the stream was cut at the output limit. Say so, or
                // the model retries the same oversized call.
                for call in &mut model_turn.invalid_calls {
                    call.error = truncated_call_error(self.reserved_output_tokens);
                }
            }

            let mut replay_tool_calls: Vec<MessageToolCall> = model_turn
                .tool_calls
                .iter()
                .map(|call| MessageToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.input.clone(),
                })
                .collect();
            replay_tool_calls.extend(model_turn.invalid_calls.iter().map(|call| {
                MessageToolCall {
                    id: call.call_id.clone(),
                    name: call.name.clone(),
                    // The raw text is not valid JSON, so replay an empty object; the error result explains.
                    arguments: json!({}),
                }
            }));
            let mut assistant =
                Message::assistant_with_tool_calls(model_turn.text.clone(), replay_tool_calls);
            // DeepSeek-style thinking mode requires reasoning to come back on tool-call turns.
            if !model_turn.reasoning.trim().is_empty() {
                assistant.reasoning_content = Some(model_turn.reasoning.clone());
            }
            self.messages.push(assistant);

            let call_names: HashMap<String, String> = model_turn
                .tool_calls
                .iter()
                .map(|call| (call.id.clone(), call.name.clone()))
                .chain(
                    model_turn
                        .invalid_calls
                        .iter()
                        .map(|call| (call.call_id.clone(), call.name.clone())),
                )
                .collect();

            let final_results = self
                .execute_tool_calls(&model_turn.tool_calls, &model_turn.invalid_calls)
                .await?;

            if self.checkpoint_interval > 0 && total_calls as u32 >= self.checkpoint_interval {
                self.emit(AgentEvent::Checkpoint {
                    phase: "tool_execution".into(),
                    detail: format!("Executed {total_calls} tool calls in turn {turn}"),
                    turn,
                })
                .await;
            }

            let edited_now = final_results.iter().any(|result| {
                result.success
                    && call_names
                        .get(&result.call_id)
                        .is_some_and(|name| EDIT_TOOLS.contains(&name.as_str()))
            });
            let validated_now = final_results.iter().any(|result| {
                result.success
                    && call_names
                        .get(&result.call_id)
                        .is_some_and(|name| name == VALIDATION_TOOL)
            });
            if edited_now {
                edited_since_validation = true;
            } else if validated_now {
                edited_since_validation = false;
            }

            // Track consecutive failures of the same tool to detect infinite retry loops.
            let all_failed_same_tool = !final_results.is_empty()
                && final_results.iter().all(|r| !r.success)
                && total_calls == 1;
            if all_failed_same_tool {
                let tool_name = call_names.values().next().cloned().unwrap_or_default();
                if tool_name == last_failed_tool {
                    consecutive_tool_failures += 1;
                } else {
                    last_failed_tool = tool_name;
                    consecutive_tool_failures = 1;
                }
            } else {
                consecutive_tool_failures = 0;
                last_failed_tool.clear();
            }

            for result in final_results {
                self.messages.push(Message::tool(
                    result.call_id.clone(),
                    format_tool_result(&result),
                ));
                self.emit(AgentEvent::ToolCallCompleted {
                    call_id: result.call_id.clone(),
                    output: result,
                })
                .await;
            }

            // Doom-loop check runs after results are recorded, so history stays well-formed on stop.
            for call in &model_turn.tool_calls {
                let key = format!("{}:{}", call.name, call.input);
                if last_call_key.as_deref() == Some(key.as_str()) {
                    identical_streak += 1;
                } else {
                    last_call_key = Some(key);
                    identical_streak = 1;
                }
                if identical_streak == DOOM_LOOP_NUDGE_STREAK {
                    let note = format!(
                        "You have called `{}` with identical arguments {identical_streak} times in a row. Repeating it will not change the result. Use the result you already have, try a different approach, or give the final answer.",
                        call.name
                    );
                    pending_nudge = Some(harness_note(note));
                    self.emit(AgentEvent::Checkpoint {
                        phase: "doom_loop".into(),
                        detail: format!("`{}` repeated with identical arguments", call.name),
                        turn,
                    })
                    .await;
                }
                if identical_streak >= DOOM_LOOP_STOP_STREAK {
                    let message = format!(
                        "Stopped: `{}` was called {identical_streak} times in a row with identical arguments.",
                        call.name
                    );
                    self.emit(AgentEvent::Error {
                        message: message.clone(),
                    })
                    .await;
                    return Err(ProviderError::Other(message));
                }
            }

            if consecutive_tool_failures >= MAX_CONSECUTIVE_TOOL_FAILURES {
                let msg = format!(
                    "Tool `{}` failed {} times consecutively — stopping to avoid infinite loop.",
                    last_failed_tool, consecutive_tool_failures
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
            self.cost_tracker.add(estimated_input, estimated_output);
            self.emit(AgentEvent::CostUpdated {
                input_tokens: self.cost_tracker.input_tokens,
                output_tokens: self.cost_tracker.output_tokens,
                estimated_cost_usd: self.cost_tracker.estimated_cost_usd(),
            })
            .await;
        }

        self.emit(AgentEvent::BusyStateChanged {
            state: BusyState::Idle,
        })
        .await;
        Ok(final_text)
    }

    /// The request view: canonical history, or a compacted view that always fits
    /// the model window.
    async fn request_messages(&self) -> Vec<Message> {
        if !self.smart_compaction_mode.is_enabled() && self.request_token_budget.is_none() {
            return self.messages.clone();
        }
        // A zero window means "unknown" (auto-detect off, no target set), not
        // "send nothing". The reply reservation is capped at half the window
        // so a `max_tokens` at or above the window cannot starve the request.
        let budget = self
            .request_token_budget
            .filter(|budget| *budget > 0)
            .map(|budget| budget - self.reserved_output_tokens.min(budget / 2));
        let plan = plan_context_view_fitted(
            &self.messages,
            self.smart_compaction_mode,
            budget.and_then(std::num::NonZeroUsize::new),
        );
        let report = &plan.report;
        if report.tokens_after < report.tokens_before
            || matches!(self.smart_compaction_mode, SmartCompactionMode::DryRun)
        {
            let phase = match self.smart_compaction_mode {
                SmartCompactionMode::DryRun => "dry_run",
                SmartCompactionMode::On => "completed",
                SmartCompactionMode::Off => "off",
            };
            self.emit(AgentEvent::ContextCompaction {
                phase: phase.into(),
                message: report.summary_line(),
                tokens_before: Some(report.tokens_before),
                tokens_after: Some(report.tokens_after),
                retained_groups: Some(report.retained_groups),
                dropped_groups: Some(report.dropped_groups),
            })
            .await;
        }
        // A hard budget is a correctness requirement, not a preference: send the
        // fitted view even when smart compaction is off or only in dry-run mode.
        if budget.is_some() || matches!(self.smart_compaction_mode, SmartCompactionMode::On) {
            plan.messages
        } else {
            self.messages.clone()
        }
    }

    /// One model request, retried while nothing visible has been produced yet.
    async fn request_model_turn(
        &mut self,
        request_messages: &[Message],
        tools: &[ToolDefinition],
        workspace_root: &Path,
        turn: u32,
    ) -> Result<ModelTurn, ProviderError> {
        let mut attempt = 1_u32;
        loop {
            let result = match self
                .open_stream(request_messages, tools, workspace_root)
                .await
            {
                Ok(stream) => self.consume_stream(stream).await,
                Err(err) => Err(err),
            };
            let err = match result {
                Ok(model_turn) => match model_turn.failure.clone() {
                    None => return Ok(model_turn),
                    Some(message) if model_turn.produced_visible_output() => {
                        return Err(ProviderError::RequestFailed(format!(
                            "stream interrupted after partial output: {message}"
                        )));
                    }
                    Some(message) => {
                        ProviderError::Transient(format!("stream interrupted: {message}"))
                    }
                },
                Err(err) => err,
            };
            if !err.is_retryable() || attempt >= MAX_PROVIDER_ATTEMPTS {
                return Err(err);
            }
            let delay = retry_delay(self.retry_base_delay, &err, attempt);
            self.emit(AgentEvent::Checkpoint {
                phase: "provider_retry".into(),
                detail: format!(
                    "Provider request failed ({err}); retry {attempt}/{} in {:.1}s",
                    MAX_PROVIDER_ATTEMPTS - 1,
                    delay.as_secs_f32()
                ),
                turn,
            })
            .await;
            if !self.wait_or_cancel(delay).await {
                return Err(ProviderError::Other("run cancelled".into()));
            }
            attempt += 1;
        }
    }

    /// Start a provider request. Cancel is honored while waiting for the response headers, so a
    /// stalled endpoint cannot keep the turn running after the user presses Esc.
    async fn open_stream(
        &self,
        request_messages: &[Message],
        tools: &[ToolDefinition],
        workspace_root: &Path,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, ProviderError> {
        let request = self
            .provider
            .chat(request_messages, tools, &self.model, workspace_root);
        tokio::pin!(request);
        let mut cancel_poll = tokio::time::interval(Duration::from_millis(25));
        cancel_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                result = &mut request => return result,
                _ = cancel_poll.tick() => {
                    if self.is_cancelled() {
                        self.emit(AgentEvent::Error {
                            message: "Run cancelled while waiting for the provider".into(),
                        })
                        .await;
                        return Err(ProviderError::Other("run cancelled".into()));
                    }
                }
            }
        }
    }

    /// Collect one provider stream into a `ModelTurn`, streaming text to the UI as it arrives.
    async fn consume_stream(
        &mut self,
        mut stream: tokio::sync::mpsc::Receiver<StreamChunk>,
    ) -> Result<ModelTurn, ProviderError> {
        let mut out = ModelTurn::default();
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
                    if out.text.is_empty() {
                        self.emit(AgentEvent::BusyStateChanged {
                            state: BusyState::Streaming,
                        })
                        .await;
                    }
                    out.text.push_str(&delta);
                    self.emit(AgentEvent::TokensStreamed { delta }).await;
                }
                StreamChunk::ReasoningDelta(delta) => out.reasoning.push_str(&delta),
                StreamChunk::ToolUse(call) => {
                    self.emit(AgentEvent::ToolCallStarted {
                        call_id: call.id.clone(),
                        tool: call.name.clone(),
                        input: call.input.clone(),
                    })
                    .await;
                    out.tool_calls.push(call);
                }
                StreamChunk::InvalidToolUse {
                    call_id,
                    name,
                    raw_arguments,
                    error,
                } => {
                    self.emit(AgentEvent::ToolCallStarted {
                        call_id: call_id.clone(),
                        tool: name.clone(),
                        input: json!({ "raw_arguments": raw_arguments }),
                    })
                    .await;
                    out.invalid_calls.push(InvalidToolCall {
                        call_id,
                        name,
                        raw_arguments,
                        error,
                    });
                }
                StreamChunk::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    self.cost_tracker.add(input_tokens, output_tokens);
                    self.emit(AgentEvent::CostUpdated {
                        input_tokens: self.cost_tracker.input_tokens,
                        output_tokens: self.cost_tracker.output_tokens,
                        estimated_cost_usd: self.cost_tracker.estimated_cost_usd(),
                    })
                    .await;
                }
                StreamChunk::Finish { reason } => out.finish_reason = Some(reason),
                StreamChunk::Failed(message) => out.failure = Some(message),
                StreamChunk::Done => break,
            }
        }
        Ok(out)
    }

    /// Sleep for `duration`, returning false early if the run is cancelled.
    async fn wait_or_cancel(&self, duration: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            if self.is_cancelled() {
                return false;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return true;
            }
            tokio::time::sleep((deadline - now).min(Duration::from_millis(25))).await;
        }
    }

    /// The turn budget is spent: make one tool-less request for a summary instead of failing.
    async fn wrap_up_turn_budget(
        &mut self,
        workspace_root: &Path,
        turn: u32,
    ) -> Result<String, ProviderError> {
        let note = format!(
            "The turn budget ({} turns) is used up. Do not call any tools. Summarize what you completed, what remains, and the next step for the user.",
            self.max_turns
        );
        self.emit(AgentEvent::Checkpoint {
            phase: "turn_budget".into(),
            detail: format!(
                "Turn budget of {} reached; requesting a summary",
                self.max_turns
            ),
            turn,
        })
        .await;
        let mut request_messages = self.request_messages().await;
        request_messages.push(harness_note(note));
        let model_turn = self
            .request_model_turn(&request_messages, &[], workspace_root, turn)
            .await?;
        let summary = model_turn.text.trim().to_string();
        if summary.is_empty() {
            return Err(ProviderError::Other(format!(
                "turn budget exceeded (max {})",
                self.max_turns
            )));
        }
        let answer = format!(
            "Stopped at the turn budget ({} turns). {summary}",
            self.max_turns
        );
        self.messages.push(Message::assistant(answer.clone()));
        self.emit(AgentEvent::MessageReceived {
            role: "assistant".into(),
            content: answer.clone(),
        })
        .await;
        Ok(answer)
    }

    /// Permission checks and execution for one model turn. Invalid calls get an error result
    /// and are never executed.
    async fn execute_tool_calls(
        &mut self,
        tool_calls: &[ToolCall],
        invalid_calls: &[InvalidToolCall],
    ) -> Result<Vec<ToolResult>, ProviderError> {
        // ── Phase 1: permission checks (sequential — approvals may be interactive) ──
        //
        // Produces, in original order, either a pre-resolved result (denied /
        // approval-denied) or a ticket to execute concurrently in phase 2.
        enum Ticket {
            Resolved(ToolResult),
            Execute(ToolCall),
        }

        if self.is_cancelled() {
            self.emit(AgentEvent::Error {
                message: "Run cancelled before tool execution".into(),
            })
            .await;
            return Err(ProviderError::Other("run cancelled".into()));
        }

        let mut tickets: Vec<Ticket> = Vec::with_capacity(tool_calls.len());

        for call in tool_calls {
            let tier = self.approval.check(&call.name, &call.input.to_string());

            match tier {
                PermissionTier::Denied => {
                    tickets.push(Ticket::Resolved(ToolResult {
                        call_id: call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(format!("tool `{}` denied by policy", call.name)),
                    }));
                }

                PermissionTier::Ask => {
                    let description = approval_description(call);
                    self.emit(AgentEvent::ApprovalRequested {
                        call_id: call.id.clone(),
                        tool: call.name.clone(),
                        description: description.clone(),
                    })
                    .await;
                    if let Some(hooks) = &self.hooks {
                        hooks
                            .run_best_effort(
                                HookEventKind::ApprovalRequested,
                                Some(&call.name),
                                &json!({
                                    "call_id": call.id.clone(),
                                    "tool": call.name.clone(),
                                    "input": call.input.clone(),
                                    "description": description,
                                }),
                            )
                            .await;
                    }
                    let verdict = self.approval.resolve(call, &description).await;
                    let approved = verdict.is_approved();
                    self.emit(AgentEvent::ApprovalResolved {
                        call_id: call.id.clone(),
                        approved,
                    })
                    .await;
                    if let ApprovalVerdict::AllowPattern(pattern) = &verdict {
                        self.approval.add_session_allow(pattern.clone());
                    }

                    if approved {
                        if let Some(hooks) = &self.hooks
                            && let Err(reason) = hooks
                                .run(
                                    HookEventKind::PreToolUse,
                                    Some(&call.name),
                                    &json!({
                                        "call_id": call.id.clone(),
                                        "tool": call.name.clone(),
                                        "input": call.input.clone(),
                                    }),
                                )
                                .await
                        {
                            tickets.push(Ticket::Resolved(ToolResult {
                                call_id: call.id.clone(),
                                success: false,
                                output: String::new(),
                                error: Some(reason),
                            }));
                            continue;
                        }
                        tickets.push(Ticket::Execute(call.clone()));
                    } else {
                        if self.approval.should_fail_on_ask() {
                            let message = format!(
                                "tool `{}` requires approval in headless mode; rerun with a non-interactive permission mode such as `dont-ask` or `bypass-permissions`",
                                call.name
                            );
                            self.emit(AgentEvent::Error {
                                message: message.clone(),
                            })
                            .await;
                            return Err(ProviderError::Other(message));
                        }
                        tickets.push(Ticket::Resolved(ToolResult {
                            call_id: call.id.clone(),
                            success: false,
                            output: String::new(),
                            error: Some(format!(
                                "tool `{}` requires approval; request was denied",
                                call.name
                            )),
                        }));
                    }
                }

                PermissionTier::Allowed => {
                    if let Some(hooks) = &self.hooks
                        && let Err(reason) = hooks
                            .run(
                                HookEventKind::PreToolUse,
                                Some(&call.name),
                                &json!({
                                    "call_id": call.id.clone(),
                                    "tool": call.name.clone(),
                                    "input": call.input.clone(),
                                }),
                            )
                            .await
                    {
                        tickets.push(Ticket::Resolved(ToolResult {
                            call_id: call.id.clone(),
                            success: false,
                            output: String::new(),
                            error: Some(reason),
                        }));
                        continue;
                    }
                    tickets.push(Ticket::Execute(call.clone()));
                }
            }
        }

        // ── Phase 2: execution ───────────────────────────────────────────────────
        //
        // Non-interactive tools run concurrently. `ask_question` is always
        // sequential so the UI can show one prompt at a time; parallel asks
        // would leave unanswered ones pending forever after the user answers
        // only the latest modal.
        let n = tickets.len();
        let mut results: Vec<Option<ToolResult>> = (0..n).map(|_| None).collect();

        let to_execute: Vec<(usize, ToolCall)> = tickets
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                if let Ticket::Execute(call) = t {
                    Some((i, call.clone()))
                } else {
                    None
                }
            })
            .collect();

        let (interactive, parallel): (Vec<_>, Vec<_>) = to_execute
            .into_iter()
            .partition(|(_, call)| is_interactive_tool(&call.name));

        if !parallel.is_empty() {
            let futs = parallel.iter().map(|(i, call)| {
                let fut = self.tools.execute(call);
                async move { (*i, fut.await) }
            });
            let executed: Vec<(usize, ToolResult)> = join_all(futs).await;
            for (i, result) in executed {
                results[i] = Some(result);
            }
        }

        for (i, call) in interactive {
            if self.is_cancelled() {
                results[i] = Some(ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some("run cancelled while waiting for interactive tool".into()),
                });
                continue;
            }
            results[i] = Some(self.tools.execute(&call).await);
        }

        // Fill pre-resolved slots
        for (i, ticket) in tickets.into_iter().enumerate() {
            if let Ticket::Resolved(result) = ticket {
                results[i] = Some(result);
            }
        }

        let mut final_results: Vec<ToolResult> = results.into_iter().flatten().collect();
        final_results.extend(invalid_calls.iter().map(invalid_tool_result));

        if let Some(hooks) = &self.hooks {
            for result in &final_results {
                let hook_event = if result.success {
                    HookEventKind::PostToolUse
                } else {
                    HookEventKind::PostToolFailure
                };
                hooks
                    .run_best_effort(
                        hook_event,
                        None,
                        &json!({
                            "call_id": result.call_id,
                            "success": result.success,
                            "output": result.output,
                            "error": result.error,
                        }),
                    )
                    .await;
            }
        }

        Ok(final_results)
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
    if result.success {
        result.output.clone()
    } else {
        result
            .error
            .clone()
            .unwrap_or_else(|| "tool failed".to_string())
    }
}

#[cfg(test)]
mod interactive_tool_tests {
    use super::is_interactive_tool;

    #[test]
    fn only_ask_question_is_interactive() {
        assert!(is_interactive_tool("ask_question"));
        assert!(!is_interactive_tool("list_directory"));
        assert!(!is_interactive_tool("invoke_skill"));
        assert!(!is_interactive_tool("update_todos"));
    }
}

#[cfg(test)]
mod loop_tests {
    use super::*;
    use crate::tools::ToolExecutor;
    use async_trait::async_trait;
    use nca_common::config::{PermissionConfig, PermissionMode};
    use nca_common::message::Role;
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// What the mock saw on one provider request.
    #[derive(Clone)]
    struct RecordedRequest {
        messages: Vec<Message>,
        tool_count: usize,
    }

    /// Replays one scripted outcome per `chat` call.
    struct ScriptedProvider {
        script: Mutex<VecDeque<Result<Vec<StreamChunk>, ProviderError>>>,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        async fn chat(
            &self,
            messages: &[Message],
            tools: &[ToolDefinition],
            _model: &str,
            _workspace_root: &Path,
        ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, ProviderError> {
            self.requests
                .lock()
                .expect("requests")
                .push(RecordedRequest {
                    messages: messages.to_vec(),
                    tool_count: tools.len(),
                });
            let next = self
                .script
                .lock()
                .expect("script")
                .pop_front()
                .unwrap_or_else(|| Err(ProviderError::Other("script exhausted".into())))?;
            let (tx, rx) = tokio::sync::mpsc::channel(next.len() + 1);
            for chunk in next {
                let _ = tx.try_send(chunk);
            }
            Ok(rx)
        }
    }

    /// Succeeds with output "ok" for any call.
    struct FakeTool {
        name: &'static str,
    }

    #[async_trait]
    impl ToolExecutor for FakeTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.into(),
                description: "test tool".into(),
                parameters: json!({ "type": "object" }),
            }
        }

        async fn execute(&self, call: &ToolCall) -> ToolResult {
            ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: "ok".into(),
                error: None,
            }
        }
    }

    fn call(id: &str, name: &str, input: Value) -> StreamChunk {
        StreamChunk::ToolUse(ToolCall {
            id: id.into(),
            name: name.into(),
            input,
        })
    }

    fn finished(text: &str) -> Result<Vec<StreamChunk>, ProviderError> {
        Ok(vec![
            StreamChunk::TextDelta(text.into()),
            StreamChunk::Finish {
                reason: "stop".into(),
            },
            StreamChunk::Done,
        ])
    }

    fn agent_with(
        script: Vec<Result<Vec<StreamChunk>, ProviderError>>,
        max_turns: u32,
        tools: Vec<Box<dyn ToolExecutor>>,
    ) -> (AgentLoop, Arc<Mutex<Vec<RecordedRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider {
            script: Mutex::new(script.into()),
            requests: requests.clone(),
        };
        let mut registry = ToolRegistry::new();
        for tool in tools {
            registry.register(tool);
        }
        let approval = ApprovalPolicy::new(PermissionConfig {
            mode: PermissionMode::BypassPermissions,
            ..Default::default()
        });
        let (tx, _rx) = tokio::sync::mpsc::channel(4096);
        let mut agent = AgentLoop::new(
            Box::new(provider),
            registry,
            approval,
            "test-model".into(),
            tx,
            max_turns,
            200,
            0,
            None,
        );
        agent.retry_base_delay = Duration::from_millis(1);
        (agent, requests)
    }

    fn has_harness_note(messages: &[Message], needle: &str) -> bool {
        messages
            .iter()
            .any(|m| m.role == Role::User && m.content.to_summary_text().contains(needle))
    }

    #[tokio::test]
    async fn retries_rate_limit_then_succeeds() {
        let (mut agent, requests) = agent_with(
            vec![
                Err(ProviderError::RateLimited { retry_after_ms: 1 }),
                finished("hi"),
            ],
            8,
            vec![],
        );
        let answer = agent
            .run_turn("hello", Path::new("."), &[])
            .await
            .expect("turn");
        assert_eq!(answer, "hi");
        assert_eq!(requests.lock().expect("requests").len(), 2);
    }

    #[tokio::test]
    async fn transient_failure_before_output_is_retried() {
        let (mut agent, requests) = agent_with(
            vec![
                Ok(vec![
                    StreamChunk::Failed("connection reset".into()),
                    StreamChunk::Done,
                ]),
                finished("ok"),
            ],
            8,
            vec![],
        );
        let answer = agent
            .run_turn("hello", Path::new("."), &[])
            .await
            .expect("turn");
        assert_eq!(answer, "ok");
        assert_eq!(requests.lock().expect("requests").len(), 2);
    }

    #[tokio::test]
    async fn stream_failure_after_partial_output_is_not_retried() {
        let (mut agent, requests) = agent_with(
            vec![Ok(vec![
                StreamChunk::TextDelta("partial".into()),
                StreamChunk::Failed("connection reset".into()),
                StreamChunk::Done,
            ])],
            8,
            vec![],
        );
        let err = agent
            .run_turn("hello", Path::new("."), &[])
            .await
            .expect_err("partial output must not be retried");
        assert!(err.to_string().contains("partial output"), "{err}");
        assert_eq!(requests.lock().expect("requests").len(), 1);
    }

    #[tokio::test]
    async fn length_cutoff_without_visible_output_fails_once() {
        let (mut agent, requests) = agent_with(
            vec![Ok(vec![
                StreamChunk::Finish {
                    reason: "length".into(),
                },
                StreamChunk::Done,
            ])],
            8,
            vec![],
        );
        let err = agent
            .run_turn("hello", Path::new("."), &[])
            .await
            .expect_err("length cutoff must fail");
        assert!(err.to_string().contains("output token limit"), "{err}");
        assert_eq!(requests.lock().expect("requests").len(), 1);
    }

    #[tokio::test]
    async fn invalid_tool_arguments_go_back_to_the_model() {
        let (mut agent, requests) = agent_with(
            vec![
                Ok(vec![
                    StreamChunk::InvalidToolUse {
                        call_id: "c1".into(),
                        name: "read_file".into(),
                        raw_arguments: "{\"path\":".into(),
                        error: "EOF while parsing".into(),
                    },
                    StreamChunk::Done,
                ]),
                finished("recovered"),
            ],
            8,
            vec![],
        );
        let answer = agent
            .run_turn("read it", Path::new("."), &[])
            .await
            .expect("turn");
        assert_eq!(answer, "recovered");
        let recorded = requests.lock().expect("requests");
        let last = recorded[1].messages.last().expect("tool message");
        assert_eq!(last.role, Role::Tool);
        assert_eq!(last.tool_call_id.as_deref(), Some("c1"));
        assert!(
            last.content
                .to_summary_text()
                .contains("Invalid arguments for tool `read_file`")
        );
    }

    #[tokio::test]
    async fn a_tool_call_cut_by_the_output_limit_says_so() {
        let (mut agent, requests) = agent_with(
            vec![
                Ok(vec![
                    StreamChunk::InvalidToolUse {
                        call_id: "c1".into(),
                        name: "write_file".into(),
                        raw_arguments: "{\"path\":\"index.html\",\"content\":\"<html>".into(),
                        error: "EOF while parsing a string".into(),
                    },
                    StreamChunk::Finish {
                        reason: "length".into(),
                    },
                    StreamChunk::Done,
                ]),
                finished("split it up"),
            ],
            8,
            vec![],
        );
        agent.set_reserved_output_tokens(8_192);
        agent
            .run_turn("write a long page", Path::new("."), &[])
            .await
            .expect("turn");
        let recorded = requests.lock().expect("requests");
        let text = recorded[1]
            .messages
            .last()
            .expect("tool message")
            .content
            .to_summary_text();
        assert!(text.contains("output token limit"), "{text}");
        assert!(text.contains("`model.max_tokens` = 8192"), "{text}");
        assert!(!text.contains("EOF while parsing"), "{text}");
    }

    #[tokio::test]
    async fn reasoning_is_replayed_on_tool_call_turns() {
        let (mut agent, requests) = agent_with(
            vec![
                Ok(vec![
                    StreamChunk::ReasoningDelta("plan it".into()),
                    call("c2", "echo", json!({ "x": 1 })),
                    StreamChunk::Done,
                ]),
                finished("done"),
            ],
            8,
            vec![Box::new(FakeTool { name: "echo" })],
        );
        agent
            .run_turn("go", Path::new("."), &[])
            .await
            .expect("turn");
        let recorded = requests.lock().expect("requests");
        let replayed = recorded[1]
            .messages
            .iter()
            .find(|m| m.role == Role::Assistant && m.tool_calls.is_some())
            .expect("assistant tool-call message");
        assert_eq!(replayed.reasoning_content.as_deref(), Some("plan it"));
    }

    #[tokio::test]
    async fn doom_loop_nudges_then_stops() {
        let script = (0..6)
            .map(|_| {
                Ok(vec![
                    call("c", "echo", json!({ "x": 1 })),
                    StreamChunk::Done,
                ])
            })
            .collect();
        let (mut agent, requests) =
            agent_with(script, 32, vec![Box::new(FakeTool { name: "echo" })]);
        let err = agent
            .run_turn("loop", Path::new("."), &[])
            .await
            .expect_err("identical calls must stop the turn");
        assert!(err.to_string().contains("6 times"), "{err}");
        let recorded = requests.lock().expect("requests");
        assert_eq!(recorded.len(), 6);
        assert!(has_harness_note(
            &recorded[3].messages,
            "identical arguments 3 times"
        ));
        assert!(!has_harness_note(
            &recorded[2].messages,
            "identical arguments"
        ));
    }

    #[tokio::test]
    async fn verification_nudge_fires_once_after_unvalidated_edit() {
        let (mut agent, requests) = agent_with(
            vec![
                Ok(vec![
                    call("e1", "edit_file", json!({ "path": "a.txt" })),
                    StreamChunk::Done,
                ]),
                finished("first answer"),
                finished("final answer"),
            ],
            8,
            vec![Box::new(FakeTool { name: "edit_file" })],
        );
        let answer = agent
            .run_turn("edit a", Path::new("."), &[])
            .await
            .expect("turn");
        assert_eq!(answer, "final answer");
        let recorded = requests.lock().expect("requests");
        assert_eq!(recorded.len(), 3);
        assert!(has_harness_note(&recorded[2].messages, "run_validation"));
    }

    #[tokio::test]
    async fn turn_budget_wraps_up_without_tools() {
        let (mut agent, requests) = agent_with(
            vec![
                Ok(vec![
                    call("c1", "echo", json!({ "x": 2 })),
                    StreamChunk::Done,
                ]),
                finished("summary text"),
            ],
            1,
            vec![Box::new(FakeTool { name: "echo" })],
        );
        let answer = agent
            .run_turn("big task", Path::new("."), &[])
            .await
            .expect("wrap-up is a success");
        assert!(answer.contains("summary text"), "{answer}");
        assert!(answer.contains("turn budget"), "{answer}");
        let recorded = requests.lock().expect("requests");
        assert_eq!(recorded[1].tool_count, 0);
    }

    #[tokio::test]
    async fn empty_responses_retry_then_fail_loudly() {
        let empty = || {
            Ok(vec![
                StreamChunk::Finish {
                    reason: "stop".into(),
                },
                StreamChunk::Done,
            ])
        };
        let (mut agent, requests) = agent_with(vec![empty(), empty(), empty()], 8, vec![]);
        let err = agent
            .run_turn("hello", Path::new("."), &[])
            .await
            .expect_err("empty output must fail");
        assert!(err.to_string().contains("empty response"), "{err}");
        assert_eq!(requests.lock().expect("requests").len(), 3);
    }

    /// Never answers, like an endpoint that accepted the connection and then stalled.
    struct StalledProvider;

    #[async_trait]
    impl Provider for StalledProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model: &str,
            _workspace_root: &Path,
        ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, ProviderError> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cancel_interrupts_a_stalled_provider_request() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let mut agent = AgentLoop::new(
            Box::new(StalledProvider),
            ToolRegistry::new(),
            ApprovalPolicy::new(PermissionConfig::default()),
            "test-model".into(),
            tx,
            8,
            200,
            0,
            None,
        );
        let flag = agent.cancel_handle();
        let canceller = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            flag.store(true, Ordering::SeqCst);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(agent.run_turn("hello", Path::new("."), &[]), canceller)
        })
        .await
        .expect("cancel must end a stalled request");
        let err = result.expect_err("cancelled turn must fail");
        assert!(err.to_string().contains("run cancelled"), "{err}");
    }

    #[test]
    fn retry_delay_honors_retry_after_and_stays_bounded() {
        let base = Duration::from_secs(1);
        let transient = ProviderError::Transient("503".into());
        let first = retry_delay(base, &transient, 1);
        assert!(first >= Duration::from_secs(1) && first <= Duration::from_millis(1250));
        let third = retry_delay(base, &transient, 3);
        assert!(third >= Duration::from_secs(4) && third <= Duration::from_millis(4250));
        let long = ProviderError::RateLimited {
            retry_after_ms: 999_999,
        };
        assert_eq!(retry_delay(base, &long, 1), MAX_RETRY_DELAY);
        let short = ProviderError::RateLimited {
            retry_after_ms: 3000,
        };
        assert_eq!(retry_delay(base, &short, 1), Duration::from_secs(3));
    }

    #[test]
    fn system_prompt_is_replaced_not_appended() {
        let (mut agent, _requests) = agent_with(vec![finished("hi")], 4, vec![]);
        agent.set_system_prompt("first");
        agent.set_system_prompt("second");

        let systems: Vec<&Message> = agent
            .messages
            .iter()
            .filter(|message| message.role == Role::System)
            .collect();
        assert_eq!(systems.len(), 1, "only one system message may exist");
        assert_eq!(systems[0].content.to_summary_text(), "second");
        assert_eq!(agent.messages[0].role, Role::System);
    }

    #[test]
    fn system_prompt_refresh_keeps_an_auto_summary() {
        let (mut agent, _requests) = agent_with(vec![finished("hi")], 4, vec![]);
        agent.set_system_prompt("harness");
        agent.messages.push(Message::system(
            "## Conversation Summary (Earlier Context)\n\nwe fixed the parser",
        ));
        agent.messages.push(Message::user("continue"));

        agent.set_system_prompt("harness v2");

        assert_eq!(agent.messages[0].content.to_summary_text(), "harness v2");
        assert!(
            agent
                .messages
                .iter()
                .any(|message| message.role == Role::System
                    && message
                        .content
                        .to_summary_text()
                        .contains("we fixed the parser")),
            "the summary must survive a prompt refresh"
        );
    }

    #[test]
    fn system_prompt_refresh_keeps_a_summary_at_index_zero() {
        let (mut agent, _requests) = agent_with(vec![finished("hi")], 4, vec![]);
        agent.messages.clear();
        agent.messages.push(Message::system(
            "## Conversation Summary (Earlier Context)\n\nwe fixed the parser",
        ));

        agent.set_system_prompt("harness");

        assert_eq!(agent.messages[0].content.to_summary_text(), "harness");
        assert!(
            agent.messages[1]
                .content
                .to_summary_text()
                .contains("we fixed the parser")
        );
    }

    #[tokio::test]
    async fn an_unknown_window_does_not_clamp_the_request() {
        let (mut agent, _) = agent_with(Vec::new(), 1, Vec::new());
        agent.set_system_prompt("sys");
        for i in 0..40 {
            agent
                .messages
                .push(Message::user(format!("q{i} {}", "x".repeat(200))));
            agent.messages.push(Message::assistant(format!("a{i}")));
        }
        // Zero means "window unknown": send the history untouched.
        agent.set_request_token_budget(Some(0));
        agent.set_reserved_output_tokens(8_192);
        assert_eq!(agent.request_messages().await.len(), agent.messages.len());

        // `max_tokens` at or above the window must not starve the request.
        agent.set_request_token_budget(Some(8_000));
        assert_eq!(agent.request_messages().await.len(), agent.messages.len());
    }
}
