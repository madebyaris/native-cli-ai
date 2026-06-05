//! Transcript + status driven by `AgentEvent`.

use nca_common::config::ProviderKind;
use nca_common::event::{AgentEvent, BusyState, InteractiveQuestionPayload};
use nca_common::message::ImageAttachment;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone)]
pub enum DisplayBlock {
    User(String),
    Assistant(String),
    ToolRunning {
        name: String,
        call_id: String,
        input: String,
    },
    ApprovalPending(ApprovalRequest),
    ApprovalResolved {
        tool: String,
        approved: bool,
    },
    ToolDone {
        name: String,
        ok: bool,
        detail: String,
    },
    /// Interactive `ask_question` prompt (options + suggested answer).
    Question(InteractiveQuestionPayload),
    System(String),
    ErrorLine(String),
}

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub call_id: String,
    pub tool: String,
    pub description: String,
    pub input: String,
}

/// One row in the sidebar for a child / sub-agent session.
#[derive(Debug, Clone)]
pub struct SubagentRow {
    pub id: String,
    pub task: String,
    pub phase: String,
    pub detail: String,
    pub running: bool,
    pub skill: Option<String>,
}

/// Status of an API key validation during onboarding.
#[derive(Debug, Clone)]
pub enum OnboardingValidation {
    Validating,
    Valid,
    Failed(String),
}

pub struct TuiSessionState {
    pub blocks: Vec<DisplayBlock>,
    /// In-progress assistant text (shown below committed blocks until finalized).
    pub streaming_assistant: Option<String>,
    pub input_buffer: String,
    pub cursor_char_idx: usize,
    /// Scroll offset in *lines* (flattened transcript).
    pub scroll_lines: usize,
    /// When true, transcript stays pinned to the bottom as new output arrives.
    pub transcript_follow_tail: bool,
    pub session_id: String,
    /// Workspace root for resolving attachment paths and clipboard import.
    pub workspace_root: PathBuf,
    /// Workspace root (from `SessionStarted`), for sidebar context.
    pub workspace_display: String,
    /// Images to send on the next user message (TUI only).
    pub staged_image_attachments: Vec<ImageAttachment>,
    /// Live view of spawned sub-agents (updated from child activity events).
    pub subagents: Vec<SubagentRow>,
    pub model: String,
    pub agent_profile: String,
    pub permission_mode: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub started: Instant,
    pub busy: bool,
    /// Current busy state (for animated indicator).
    pub current_busy_state: BusyState,
    /// When the current busy state started (for animation frame selection).
    pub busy_state_since: Instant,
    pub should_exit: bool,
    /// Selected row in slash-command popup (↑↓ or click).
    pub slash_menu_index: usize,
    /// Centered command palette opened via Ctrl+P.
    pub command_palette_open: bool,
    /// Filter text for the command palette.
    pub command_palette_query: String,
    /// Approval request currently waiting for a local TUI answer.
    pub active_approval: Option<ApprovalRequest>,
    /// When set, the composer answers this question (see status hint).
    pub active_question: Option<InteractiveQuestionPayload>,
    /// Current git branch name (updated on branch switch).
    pub current_branch: String,
    /// Branch picker popup state.
    pub branch_picker_open: bool,
    /// Filter text in the branch picker.
    pub branch_picker_query: String,
    /// Selected index in the branch picker list.
    pub branch_picker_index: usize,
    /// List of branches for the picker (refreshed on open).
    pub branch_picker_branches: Vec<String>,
    /// Bounding rect of the branch chip in the status bar (for click hit-testing).
    pub branch_chip_bounds: Option<ratatui::layout::Rect>,
    /// Pick default LLM provider (or provider for API key) — TUI overlay.
    pub provider_picker_open: bool,
    pub provider_picker_index: usize,
    /// When true, picking a row sets `pending_api_key_provider` instead of applying provider.
    pub provider_picker_for_api_key: bool,
    /// After choosing a provider for API key, next non-command line is the secret.
    pub pending_api_key_provider: Option<ProviderKind>,
    /// Selected row when `@` file completion panel is visible.
    pub at_menu_index: usize,
    /// OpenCode-style "Connect a provider" modal (`/connect`).
    pub connect_modal_open: bool,
    pub connect_search: String,
    /// Index among selectable provider rows (not section headers).
    pub connect_menu_index: usize,
    /// Scroll offset for the connect modal viewport.
    pub connect_modal_scroll: usize,
    /// API key entry modal (used by `/connect` and `/apikey` TUI flows).
    pub api_key_modal_open: bool,
    pub api_key_target_provider: Option<ProviderKind>,
    pub api_key_input: String,
    pub api_key_target_has_existing: bool,
    /// When true, Enter should connect to this provider after saving/confirming the key.
    pub api_key_connect_after_save: bool,
    /// Generic info popup (read-only scrollable lines).
    pub info_modal_open: bool,
    pub info_modal_title: String,
    pub info_modal_lines: Vec<String>,
    pub info_modal_scroll: usize,
    /// Model picker popup (searchable model/provider list).
    pub model_picker_open: bool,
    pub model_picker_search: String,
    pub model_picker_index: usize,
    pub model_picker_entries: Vec<ModelPickerEntry>,
    /// Scroll offset (first visible row) in the model picker viewport.
    pub model_picker_scroll: usize,
    /// Ctrl+X leader key pending (next keypress is dispatched as shortcut).
    pub leader_pending: bool,
    /// Permission mode picker popup.
    pub permission_picker_open: bool,
    pub permission_picker_index: usize,
    /// Agent profile picker popup.
    pub agent_picker_open: bool,
    pub agent_picker_index: usize,
    /// Question modal popup (arrow-key option picker).
    pub question_modal_open: bool,
    pub question_modal_index: usize,
    pub question_modal_scroll: usize,
    /// Command palette selection index (separate from slash_menu_index).
    pub palette_index: usize,
    /// Session picker popup (interactive list with resume).
    pub session_picker_open: bool,
    pub session_picker_search: String,
    pub session_picker_index: usize,
    pub session_picker_entries: Vec<nca_common::session::SessionSnapshot>,
    /// Scroll offset for the session picker viewport.
    pub session_picker_scroll: usize,
    /// When true, the onboarding gate is active — connect modal is locked open.
    pub onboarding_mode: bool,
    /// Result of the most recent API key validation attempt (None = no attempt yet).
    pub validation_status: Option<OnboardingValidation>,
    // ── Text selection in transcript ─────────────────────────────────────
    /// When `Some`, a range of flattened transcript lines + columns is selected (inclusive).
    /// Inner tuples are `(line, col)`. `None` means no selection is active.
    pub transcript_selection: Option<((usize, usize), (usize, usize))>,
    /// True while a mouse drag is in progress (started inside the transcript area).
    pub transcript_dragging: bool,
    /// The (line, col) where the current drag started. Cleared on mouse-up.
    pub transcript_drag_anchor: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub enum ModelPickerAction {
    SwitchProvider(ProviderKind),
    ApplyModel(String),
}

#[derive(Debug, Clone)]
pub struct ModelPickerEntry {
    pub label: String,
    pub detail: String,
    pub action: ModelPickerAction,
    pub is_header: bool,
}

impl TuiSessionState {
    pub fn new(
        session_id: String,
        model: String,
        agent_profile: String,
        permission_mode: String,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            blocks: Vec::new(),
            streaming_assistant: None,
            input_buffer: String::new(),
            cursor_char_idx: 0,
            scroll_lines: 0,
            transcript_follow_tail: true,
            session_id,
            workspace_root,
            workspace_display: String::new(),
            staged_image_attachments: Vec::new(),
            subagents: Vec::new(),
            model,
            agent_profile,
            permission_mode,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            started: Instant::now(),
            busy: false,
            current_busy_state: BusyState::Idle,
            busy_state_since: Instant::now(),
            should_exit: false,
            slash_menu_index: 0,
            command_palette_open: false,
            command_palette_query: String::new(),
            active_approval: None,
            active_question: None,
            current_branch: String::new(),
            branch_picker_open: false,
            branch_picker_query: String::new(),
            branch_picker_index: 0,
            branch_picker_branches: Vec::new(),
            branch_chip_bounds: None,
            provider_picker_open: false,
            provider_picker_index: 0,
            provider_picker_for_api_key: false,
            pending_api_key_provider: None,
            at_menu_index: 0,
            connect_modal_open: false,
            connect_search: String::new(),
            connect_menu_index: 0,
            connect_modal_scroll: 0,
            api_key_modal_open: false,
            api_key_target_provider: None,
            api_key_input: String::new(),
            api_key_target_has_existing: false,
            api_key_connect_after_save: false,
            info_modal_open: false,
            info_modal_title: String::new(),
            info_modal_lines: Vec::new(),
            info_modal_scroll: 0,
            model_picker_open: false,
            model_picker_search: String::new(),
            model_picker_index: 0,
            model_picker_entries: Vec::new(),
            model_picker_scroll: 0,
            leader_pending: false,
            permission_picker_open: false,
            permission_picker_index: 0,
            agent_picker_open: false,
            agent_picker_index: 0,
            question_modal_open: false,
            question_modal_index: 0,
            question_modal_scroll: 0,
            palette_index: 0,
            session_picker_open: false,
            session_picker_search: String::new(),
            session_picker_index: 0,
            session_picker_entries: Vec::new(),
            session_picker_scroll: 0,
            onboarding_mode: false,
            validation_status: None,
            transcript_selection: None,
            transcript_dragging: false,
            transcript_drag_anchor: None,
        }
    }

    pub fn open_connect_modal(&mut self) {
        self.connect_modal_open = true;
        self.connect_search.clear();
        self.connect_menu_index = 0;
        self.connect_modal_scroll = 0;
    }

    pub fn close_connect_modal(&mut self) {
        self.connect_modal_open = false;
        self.connect_search.clear();
        self.connect_menu_index = 0;
        self.connect_modal_scroll = 0;
    }

    pub fn open_api_key_modal(
        &mut self,
        provider: ProviderKind,
        has_existing: bool,
        connect_after_save: bool,
    ) {
        self.api_key_modal_open = true;
        self.api_key_target_provider = Some(provider);
        self.api_key_input.clear();
        self.api_key_target_has_existing = has_existing;
        self.api_key_connect_after_save = connect_after_save;
        self.validation_status = None;
    }

    pub fn close_api_key_modal(&mut self) {
        self.api_key_modal_open = false;
        self.api_key_target_provider = None;
        self.api_key_input.clear();
        self.api_key_target_has_existing = false;
        self.api_key_connect_after_save = false;
        self.validation_status = None;
    }

    pub fn open_info_modal(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.info_modal_open = true;
        self.info_modal_title = title.into();
        self.info_modal_lines = lines;
        self.info_modal_scroll = 0;
    }

    pub fn close_info_modal(&mut self) {
        self.info_modal_open = false;
        self.info_modal_title.clear();
        self.info_modal_lines.clear();
        self.info_modal_scroll = 0;
    }

    pub fn open_model_picker(&mut self, entries: Vec<ModelPickerEntry>) {
        self.model_picker_open = true;
        self.model_picker_search.clear();
        self.model_picker_index = 0;
        self.model_picker_scroll = 0;
        self.model_picker_entries = entries;
    }

    pub fn close_model_picker(&mut self) {
        self.model_picker_open = false;
        self.model_picker_search.clear();
        self.model_picker_index = 0;
        self.model_picker_scroll = 0;
        self.model_picker_entries.clear();
    }

    pub fn open_permission_picker(&mut self, current_index: usize) {
        self.permission_picker_open = true;
        self.permission_picker_index = current_index;
    }

    pub fn close_permission_picker(&mut self) {
        self.permission_picker_open = false;
        self.permission_picker_index = 0;
    }

    pub fn open_agent_picker(&mut self, current_index: usize) {
        self.agent_picker_open = true;
        self.agent_picker_index = current_index;
    }

    pub fn close_agent_picker(&mut self) {
        self.agent_picker_open = false;
        self.agent_picker_index = 0;
    }

    pub fn open_question_modal(&mut self) {
        self.question_modal_open = true;
        self.question_modal_index = 0;
        self.question_modal_scroll = 0;
    }

    pub fn close_question_modal(&mut self) {
        self.question_modal_open = false;
        self.question_modal_index = 0;
        self.question_modal_scroll = 0;
    }

    pub fn open_session_picker(
        &mut self,
        entries: Vec<nca_common::session::SessionSnapshot>,
        current: &str,
    ) {
        self.session_picker_open = true;
        self.session_picker_search.clear();
        self.session_picker_index = entries.iter().position(|e| e.id == current).unwrap_or(0);
        self.session_picker_entries = entries;
        self.session_picker_scroll = 0;
    }

    pub fn close_session_picker(&mut self) {
        self.session_picker_open = false;
        self.session_picker_search.clear();
        self.session_picker_index = 0;
        self.session_picker_entries.clear();
        self.session_picker_scroll = 0;
    }

    pub fn open_provider_picker(&mut self, current: ProviderKind, for_api_key: bool) {
        self.provider_picker_open = true;
        self.provider_picker_for_api_key = for_api_key;
        self.provider_picker_index = ProviderKind::ALL
            .iter()
            .position(|p| *p == current)
            .unwrap_or(0);
    }

    pub fn close_provider_picker(&mut self) {
        self.provider_picker_open = false;
        self.provider_picker_for_api_key = false;
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    pub fn set_busy_state(&mut self, state: BusyState) {
        if self.current_busy_state != state {
            self.current_busy_state = state;
            self.busy_state_since = Instant::now();
        }
    }

    pub fn push_error(&mut self, msg: String) {
        self.blocks.push(DisplayBlock::ErrorLine(msg));
    }

    /// Approval/question prompts from replayed history are transcript only.
    /// The live pending channels are not restored on resume, so these must not
    /// keep the input box in approval/answer mode.
    pub fn clear_replayed_interaction_state(&mut self) {
        self.active_approval = None;
        self.active_question = None;
        self.close_question_modal();
    }

    pub fn clear_active_approval_if_matches(&mut self, call_id: &str) {
        if self
            .active_approval
            .as_ref()
            .is_some_and(|req| req.call_id == call_id)
        {
            self.active_approval = None;
        }
    }

    pub fn set_agent_profile(&mut self, label: &str) {
        self.agent_profile = label.to_string();
    }

    pub fn set_current_branch(&mut self, branch: &str) {
        self.current_branch = branch.to_string();
    }

    pub fn open_branch_picker(&mut self, branches: Vec<String>, current: &str) {
        self.branch_picker_branches = branches;
        self.branch_picker_query.clear();
        self.branch_picker_index = self
            .branch_picker_branches
            .iter()
            .position(|b| b == current)
            .unwrap_or(0);
        self.branch_picker_open = true;
    }

    pub fn close_branch_picker(&mut self) {
        self.branch_picker_open = false;
        self.branch_picker_query.clear();
        self.branch_picker_branches.clear();
        self.branch_picker_index = 0;
    }

    pub fn set_permission_mode(&mut self, mode: &str) {
        self.permission_mode = mode.to_string();
    }

    fn flush_stream_before_tool(&mut self) {
        if let Some(s) = self.streaming_assistant.take()
            && !s.trim().is_empty()
        {
            self.blocks.push(DisplayBlock::Assistant(s));
        }
    }

    pub fn apply_event(&mut self, e: &AgentEvent) {
        match e {
            AgentEvent::SessionStarted {
                session_id,
                model,
                workspace,
            } => {
                self.session_id = session_id.clone();
                self.model = model.clone();
                self.workspace_root = workspace.clone();
                self.workspace_display = workspace.display().to_string();
            }
            AgentEvent::MessageReceived { role, content } => {
                if role == "user" {
                    self.streaming_assistant = None;
                    self.blocks.push(DisplayBlock::User(content.clone()));
                    self.set_busy_state(BusyState::Thinking);
                } else if role == "assistant" {
                    self.streaming_assistant = None;
                    self.blocks.push(DisplayBlock::Assistant(content.clone()));
                    self.set_busy_state(BusyState::Idle);
                }
            }
            AgentEvent::TokensStreamed { delta } => {
                self.streaming_assistant
                    .get_or_insert_with(String::new)
                    .push_str(delta);
                self.set_busy_state(BusyState::Streaming);
            }
            AgentEvent::ToolCallStarted {
                call_id,
                tool,
                input,
            } => {
                self.flush_stream_before_tool();
                self.blocks.push(DisplayBlock::ToolRunning {
                    name: tool.clone(),
                    call_id: call_id.clone(),
                    input: format_tool_input_for_display(tool, input),
                });
                self.set_busy_state(BusyState::ToolRunning);
            }
            AgentEvent::ToolCallCompleted { call_id, output } => {
                let ok = output.success;
                self.active_approval = self
                    .active_approval
                    .take()
                    .filter(|req| req.call_id != *call_id);
                self.set_busy_state(BusyState::Thinking);
                let detail = if ok {
                    truncate(&output.output, 120)
                } else {
                    output.error.clone().unwrap_or_else(|| "failed".into())
                };
                if let Some(idx) = self.blocks.iter().rposition(
                    |b| {
                        matches!(b, DisplayBlock::ToolRunning { call_id: id, .. } if id == call_id)
                            || matches!(b, DisplayBlock::ApprovalPending(req) if req.call_id == *call_id)
                    },
                ) {
                    let name = match &self.blocks[idx] {
                        DisplayBlock::ToolRunning { name, .. } => name.clone(),
                        DisplayBlock::ApprovalPending(req) => req.tool.clone(),
                        _ => "?".into(),
                    };
                    self.blocks[idx] = DisplayBlock::ToolDone { name, ok, detail };
                } else {
                    self.blocks.push(DisplayBlock::ToolDone {
                        name: "?".into(),
                        ok,
                        detail,
                    });
                }
            }
            AgentEvent::ApprovalRequested {
                call_id,
                tool,
                description,
            } => {
                let input = self
                    .blocks
                    .iter()
                    .rev()
                    .find_map(|block| match block {
                        DisplayBlock::ToolRunning {
                            call_id: id, input, ..
                        } if id == call_id => Some(input.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "{}".into());
                let req = ApprovalRequest {
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    description: description.clone(),
                    input,
                };
                self.active_approval = Some(req.clone());
                self.set_busy_state(BusyState::ApprovalPending);
                if let Some(idx) = self.blocks.iter().rposition(
                    |b| matches!(b, DisplayBlock::ToolRunning { call_id: id, .. } if id == call_id),
                ) {
                    self.blocks[idx] = DisplayBlock::ApprovalPending(req);
                } else {
                    self.blocks.push(DisplayBlock::ApprovalPending(req));
                }
            }
            AgentEvent::ApprovalResolved { call_id, approved } => {
                let tool = self
                    .active_approval
                    .as_ref()
                    .filter(|req| req.call_id == *call_id)
                    .map(|req| req.tool.clone())
                    .or_else(|| {
                        self.blocks.iter().rev().find_map(|block| match block {
                            DisplayBlock::ApprovalPending(req) if req.call_id == *call_id => {
                                Some(req.tool.clone())
                            }
                            _ => None,
                        })
                    })
                    .unwrap_or_else(|| "tool".into());
                self.active_approval = self
                    .active_approval
                    .take()
                    .filter(|req| req.call_id != *call_id);
                self.blocks.push(DisplayBlock::ApprovalResolved {
                    tool,
                    approved: *approved,
                });
            }
            AgentEvent::QuestionRequested { question } => {
                self.active_question = Some(question.clone());
                self.blocks.push(DisplayBlock::Question(question.clone()));
                // Bring the prompt into view when follow-tail is on (default).
                self.transcript_follow_tail = true;
                self.open_question_modal();
            }
            AgentEvent::QuestionResolved {
                question_id,
                selection,
            } => {
                self.active_question = None;
                self.close_question_modal();
                self.blocks.push(DisplayBlock::System(format!(
                    "Answered question {question_id}: {selection:?}"
                )));
            }
            AgentEvent::CostUpdated {
                input_tokens,
                output_tokens,
                cache_read_tokens: _,
                estimated_cost_usd,
            } => {
                self.input_tokens = *input_tokens;
                self.output_tokens = *output_tokens;
                self.cost_usd = *estimated_cost_usd;
            }
            AgentEvent::Error { message } => {
                self.blocks.push(DisplayBlock::ErrorLine(message.clone()));
                if message.to_ascii_lowercase().contains("run cancelled") {
                    self.set_busy_state(BusyState::Idle);
                } else {
                    self.set_busy_state(BusyState::Error);
                }
            }
            AgentEvent::Checkpoint { .. } => {}
            AgentEvent::ChildSessionSpawned {
                child_session_id,
                task,
                ..
            } => {
                let short = short_session_prefix(child_session_id);
                let task_s = truncate(task, 200);
                if let Some(row) = self
                    .subagents
                    .iter_mut()
                    .find(|r| r.id == *child_session_id)
                {
                    row.task = task_s.clone();
                    row.running = true;
                } else {
                    self.subagents.push(SubagentRow {
                        id: child_session_id.clone(),
                        task: task_s.clone(),
                        phase: String::new(),
                        detail: String::new(),
                        running: true,
                        skill: None,
                    });
                }
                self.blocks.push(DisplayBlock::System(format!(
                    "Sub-agent {short}… — {}",
                    truncate(task, 80)
                )));
            }
            AgentEvent::ChildSessionActivity {
                child_session_id,
                phase,
                detail,
            } => {
                let short = short_session_prefix(child_session_id);
                let d = truncate(detail, 120);
                if let Some(row) = self
                    .subagents
                    .iter_mut()
                    .find(|r| r.id == *child_session_id)
                {
                    row.phase = phase.clone();
                    row.detail = d.clone();
                    row.running = true;
                    if phase == "skill" || phase == "invoke_skill" {
                        row.skill = Some(detail.clone());
                    }
                } else {
                    self.subagents.push(SubagentRow {
                        id: child_session_id.clone(),
                        task: "(sub-agent)".into(),
                        phase: phase.clone(),
                        detail: d.clone(),
                        running: true,
                        skill: if phase == "skill" || phase == "invoke_skill" {
                            Some(detail.clone())
                        } else {
                            None
                        },
                    });
                }
                self.blocks
                    .push(DisplayBlock::System(format!("↳ {short}… · {phase} · {d}")));
            }
            AgentEvent::ChildSessionCompleted {
                child_session_id,
                status,
                ..
            } => {
                let short = short_session_prefix(child_session_id);
                if let Some(row) = self
                    .subagents
                    .iter_mut()
                    .find(|r| r.id == *child_session_id)
                {
                    row.running = false;
                    row.phase = "done".into();
                    row.detail = status.clone();
                }
                self.blocks.push(DisplayBlock::System(format!(
                    "Sub-agent {short}… done: {status}"
                )));
            }
            AgentEvent::BusyStateChanged { state } => {
                self.set_busy_state(*state);
            }
            _ => {}
        }
    }
}

fn short_session_prefix(id: &str) -> &str {
    if id.len() > 8 { &id[..8] } else { id }
}

fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        format!(
            "{}…",
            t.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

fn format_tool_input_for_display(tool: &str, value: &Value) -> String {
    if tool == "spawn_subagent" {
        format_spawn_subagent_input(value)
    } else {
        format_tool_input(value)
    }
}

fn format_spawn_subagent_input(v: &Value) -> String {
    let task = v.get("task").and_then(|t| t.as_str()).unwrap_or("").trim();
    let wt = v
        .get("use_worktree")
        .and_then(|b| b.as_bool())
        .unwrap_or(true);
    let n_focus = v
        .get("focus_files")
        .and_then(|a| a.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    format!(
        "task:\n{}\nworktree: {} · focus_files: {}",
        truncate(task, 500),
        wt,
        n_focus
    )
}

fn format_tool_input(value: &Value) -> String {
    if let Some(raw) = value.as_str()
        && let Ok(parsed) = serde_json::from_str::<Value>(raw)
    {
        return serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| raw.to_string());
    }
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::event::{
        AgentEvent, InteractiveQuestionPayload, QuestionOption, QuestionSelection,
    };

    #[test]
    fn question_requested_sets_active_question() {
        let mut st = TuiSessionState::new(
            "session-x".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        let q = InteractiveQuestionPayload {
            question_id: "q-1".into(),
            call_id: "c1".into(),
            prompt: "Pick".into(),
            options: vec![QuestionOption {
                id: "a".into(),
                label: "A".into(),
            }],
            allow_custom: true,
            suggested_answer: "A".into(),
        };
        st.apply_event(&AgentEvent::QuestionRequested {
            question: q.clone(),
        });
        assert_eq!(
            st.active_question.as_ref().map(|x| x.question_id.as_str()),
            Some("q-1")
        );
        assert!(matches!(st.blocks.last(), Some(DisplayBlock::Question(_))));

        st.apply_event(&AgentEvent::QuestionResolved {
            question_id: "q-1".into(),
            selection: QuestionSelection::Suggested,
        });
        assert!(st.active_question.is_none());
    }

    #[test]
    fn child_session_activity_updates_subagent_row() {
        let mut st = TuiSessionState::new(
            "session-x".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.apply_event(&AgentEvent::ChildSessionSpawned {
            parent_session_id: "session-x".into(),
            child_session_id: "child-abc".into(),
            task: "do the thing".into(),
            workspace: std::path::PathBuf::from("/tmp"),
            branch: None,
        });
        assert_eq!(st.subagents.len(), 1);
        st.apply_event(&AgentEvent::ChildSessionActivity {
            child_session_id: "child-abc".into(),
            phase: "read_file".into(),
            detail: "src/lib.rs".into(),
        });
        assert_eq!(st.subagents[0].phase, "read_file");
        assert_eq!(st.subagents[0].detail, "src/lib.rs");
    }

    #[test]
    fn approval_requested_promotes_running_tool_with_input() {
        let mut st = TuiSessionState::new(
            "session-x".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.apply_event(&AgentEvent::ToolCallStarted {
            call_id: "call-1".into(),
            tool: "execute_bash".into(),
            input: serde_json::json!({"command":"ls -la"}),
        });
        st.apply_event(&AgentEvent::ApprovalRequested {
            call_id: "call-1".into(),
            tool: "execute_bash".into(),
            description: "Tool `execute_bash` requires approval".into(),
        });

        assert!(st.active_approval.is_some());
        match st.blocks.last() {
            Some(DisplayBlock::ApprovalPending(req)) => {
                assert_eq!(req.tool, "execute_bash");
                assert!(req.input.contains("command"));
                assert!(req.input.contains("ls -la"));
            }
            other => panic!("expected approval block, got {other:?}"),
        }
    }

    #[test]
    fn clear_replayed_interaction_state_drops_stale_prompts() {
        let mut st = TuiSessionState::new(
            "session-x".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.active_approval = Some(ApprovalRequest {
            call_id: "call-1".into(),
            tool: "execute_bash".into(),
            description: "approve".into(),
            input: "{}".into(),
        });
        st.active_question = Some(InteractiveQuestionPayload {
            question_id: "q-1".into(),
            call_id: "call-2".into(),
            prompt: "Pick".into(),
            options: vec![],
            allow_custom: true,
            suggested_answer: String::new(),
        });

        st.clear_replayed_interaction_state();

        assert!(st.active_approval.is_none());
        assert!(st.active_question.is_none());
    }

    #[test]
    fn open_close_question_modal() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        assert!(!st.question_modal_open);
        assert_eq!(st.question_modal_index, 0);

        st.open_question_modal();
        assert!(st.question_modal_open);
        assert_eq!(st.question_modal_index, 0);
        assert_eq!(st.question_modal_scroll, 0);

        st.question_modal_index = 3;
        st.close_question_modal();
        assert!(!st.question_modal_open);
        assert_eq!(st.question_modal_index, 0);
        assert_eq!(st.question_modal_scroll, 0);
    }

    #[test]
    fn question_requested_opens_modal() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        let q = InteractiveQuestionPayload {
            question_id: "q-1".into(),
            call_id: "c1".into(),
            prompt: "Pick".into(),
            options: vec![QuestionOption {
                id: "a".into(),
                label: "A".into(),
            }],
            allow_custom: true,
            suggested_answer: "A".into(),
        };
        st.apply_event(&AgentEvent::QuestionRequested {
            question: q.clone(),
        });
        assert!(st.question_modal_open);
        assert_eq!(st.question_modal_index, 0);
        assert!(st.active_question.is_some());
    }

    #[test]
    fn question_resolved_closes_modal() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.question_modal_open = true;
        st.question_modal_index = 2;
        st.apply_event(&AgentEvent::QuestionResolved {
            question_id: "q-1".into(),
            selection: QuestionSelection::Suggested,
        });
        assert!(st.active_question.is_none());
        assert!(!st.question_modal_open);
        assert_eq!(st.question_modal_index, 0);
    }
}
