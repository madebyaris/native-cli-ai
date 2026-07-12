//! Transcript + status driven by `AgentEvent`.

use super::overlay::{UiOverlay, UiOverlayKind};
use nca_common::config::ProviderKind;
use nca_common::event::{AgentEvent, BusyState, InteractiveQuestionPayload, QuestionSelection};
use nca_common::message::ImageAttachment;
use nca_common::todo::{AgentTodo, TodoStatus};
use ratatui::text::Line;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Instant;

/// Cached wrapped + styled transcript lines used by the draw path.
///
/// Invalidation is driven by `transcript_version` (bumped on any mutation that
/// affects `transcript_lines_and_hits` output) and by viewport `width`. Rebuilding
/// styled `ratatui::text::Line` values for every frame over a long transcript is
/// the hottest CPU path in the TUI (see docs/research/rust-ratatui-optimization.md).
pub struct TranscriptCache {
    pub built_for_version: u64,
    pub built_for_width: u16,
    pub lines: Vec<Line<'static>>,
    pub hits: Vec<Option<(String, QuestionSelection)>>,
}

impl TranscriptCache {
    pub fn is_valid(&self, version: u64, width: u16) -> bool {
        self.built_for_version == version && self.built_for_width == width
    }
}

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

/// Steps for the in-TUI “add custom provider” wizard (`/provider` → Add custom provider…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomProviderSetupStep {
    Compatibility,
    BaseUrl,
    ApiKey,
    Model,
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
    /// Cumulative input tokens reported by the child's latest `CostUpdated`.
    pub tokens_in: u64,
    /// Cumulative output tokens reported by the child's latest `CostUpdated`.
    pub tokens_out: u64,
}

/// Latest smart-context / compaction diagnostics for `/status`.
#[derive(Debug, Clone, Default)]
pub struct ContextCompactionReport {
    pub phase: String,
    pub message: String,
    pub tokens_before: Option<usize>,
    pub tokens_after: Option<usize>,
    pub retained_groups: Option<usize>,
    pub dropped_groups: Option<usize>,
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
    /// Session todo list (last `TodosUpdated` wins).
    pub todos: Vec<AgentTodo>,
    /// Latest context compaction diagnostics (if any).
    pub context_report: Option<ContextCompactionReport>,
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
    /// Active modal overlay (at most one).
    pub overlay: UiOverlay,
    /// Approval request currently waiting for a local TUI answer.
    pub active_approval: Option<ApprovalRequest>,
    /// When set, the composer answers this question (see status hint).
    pub active_question: Option<InteractiveQuestionPayload>,
    /// Current git branch name (updated on branch switch).
    pub current_branch: String,
    /// Bounding rect of the branch chip in the status bar (for click hit-testing).
    pub branch_chip_bounds: Option<ratatui::layout::Rect>,
    /// After choosing a provider for API key, next non-command line is the secret.
    pub pending_api_key_provider: Option<ProviderKind>,
    /// Selected row when `@` file completion panel is visible.
    pub at_menu_index: usize,
    /// Ctrl+X leader key pending (next keypress is dispatched as shortcut).
    pub leader_pending: bool,
    /// When true, the onboarding gate is active — connect modal is locked open.
    pub onboarding_mode: bool,
    /// Result of the most recent API key validation attempt (None = no attempt yet).
    pub validation_status: Option<OnboardingValidation>,
    /// Monotonically increasing version bumped on any state change. Used by the
    /// render loop to decide whether a redraw is needed (dirty-flag pattern).
    pub state_version: u64,
    /// Bumped specifically when `blocks` / `streaming_assistant` / interactive
    /// state that the transcript depends on changes. Used by the transcript cache.
    pub transcript_version: u64,
    /// Cached wrapped transcript lines for the current width + version.
    pub transcript_cache: Option<TranscriptCache>,
    /// Chars appended since the last transcript-cache invalidation during streaming.
    pub stream_chars_since_dirty: usize,
    /// When we last invalidated the transcript cache for streaming output.
    pub last_stream_transcript_dirty: Option<Instant>,
}

/// Minimum streamed chars before invalidating the transcript cache again.
pub const STREAM_TRANSCRIPT_DIRTY_CHARS: usize = 64;
/// Minimum interval between streaming transcript-cache invalidations.
pub const STREAM_TRANSCRIPT_DIRTY_MS: u128 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPickerAction {
    SwitchProvider(ProviderKind),
    ApplyModel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
            todos: Vec::new(),
            context_report: None,
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
            overlay: UiOverlay::None,
            active_approval: None,
            active_question: None,
            current_branch: String::new(),
            branch_chip_bounds: None,
            pending_api_key_provider: None,
            at_menu_index: 0,
            leader_pending: false,
            onboarding_mode: false,
            validation_status: None,
            state_version: 1,
            transcript_version: 1,
            transcript_cache: None,
            stream_chars_since_dirty: 0,
            last_stream_transcript_dirty: None,
        }
    }

    /// Replace the overlay after validating the FSM transition table.
    pub fn set_overlay(&mut self, next: UiOverlay) {
        let current = std::mem::replace(&mut self.overlay, UiOverlay::None);
        self.overlay = UiOverlay::transition(current, next);
        self.mark_dirty();
    }

    pub fn close_overlay(&mut self) {
        if self.overlay.is_open() {
            self.overlay = UiOverlay::None;
            self.mark_dirty();
        }
    }

    pub fn overlay_kind(&self) -> UiOverlayKind {
        self.overlay.kind()
    }

    pub fn command_palette_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::CommandPalette { .. })
    }

    pub fn branch_picker_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::BranchPicker { .. })
    }

    pub fn connect_modal_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::ConnectModal { .. })
    }

    pub fn api_key_modal_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::ApiKeyModal { .. })
    }

    pub fn info_modal_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::InfoModal { .. })
    }

    pub fn model_picker_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::ModelPicker { .. })
    }

    pub fn permission_picker_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::PermissionPicker { .. })
    }

    pub fn agent_picker_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::AgentPicker { .. })
    }

    pub fn question_modal_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::QuestionModal { .. })
    }

    pub fn session_picker_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::SessionPicker { .. })
    }

    pub fn provider_picker_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::ProviderPicker { .. })
    }

    pub fn custom_provider_setup_open(&self) -> bool {
        matches!(self.overlay, UiOverlay::CustomProviderSetup { .. })
    }

    pub fn command_palette_query(&self) -> &str {
        match &self.overlay {
            UiOverlay::CommandPalette { query, .. } => query.as_str(),
            _ => "",
        }
    }

    pub fn palette_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::CommandPalette { palette_index, .. } => *palette_index,
            _ => 0,
        }
    }

    pub fn branch_picker_query(&self) -> &str {
        match &self.overlay {
            UiOverlay::BranchPicker { query, .. } => query.as_str(),
            _ => "",
        }
    }

    pub fn branch_picker_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::BranchPicker { index, .. } => *index,
            _ => 0,
        }
    }

    pub fn branch_picker_branches(&self) -> &[String] {
        match &self.overlay {
            UiOverlay::BranchPicker { branches, .. } => branches.as_slice(),
            _ => &[],
        }
    }

    pub fn connect_search(&self) -> &str {
        match &self.overlay {
            UiOverlay::ConnectModal { search, .. } => search.as_str(),
            _ => "",
        }
    }

    pub fn connect_menu_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::ConnectModal { menu_index, .. } => *menu_index,
            _ => 0,
        }
    }

    pub fn connect_modal_scroll(&self) -> usize {
        match &self.overlay {
            UiOverlay::ConnectModal { scroll, .. } => *scroll,
            _ => 0,
        }
    }

    pub fn api_key_target_provider(&self) -> Option<ProviderKind> {
        match &self.overlay {
            UiOverlay::ApiKeyModal { provider, .. } => Some(*provider),
            _ => None,
        }
    }

    pub fn api_key_input(&self) -> &str {
        match &self.overlay {
            UiOverlay::ApiKeyModal { input, .. } => input.as_str(),
            _ => "",
        }
    }

    pub fn api_key_target_has_existing(&self) -> bool {
        matches!(
            self.overlay,
            UiOverlay::ApiKeyModal {
                has_existing: true,
                ..
            }
        )
    }

    pub fn api_key_connect_after_save(&self) -> bool {
        matches!(
            self.overlay,
            UiOverlay::ApiKeyModal {
                connect_after_save: true,
                ..
            }
        )
    }

    pub fn info_modal_title(&self) -> &str {
        match &self.overlay {
            UiOverlay::InfoModal { title, .. } => title.as_str(),
            _ => "",
        }
    }

    pub fn info_modal_lines(&self) -> &[String] {
        match &self.overlay {
            UiOverlay::InfoModal { lines, .. } => lines.as_slice(),
            _ => &[],
        }
    }

    pub fn info_modal_scroll(&self) -> usize {
        match &self.overlay {
            UiOverlay::InfoModal { scroll, .. } => *scroll,
            _ => 0,
        }
    }

    pub fn model_picker_search(&self) -> &str {
        match &self.overlay {
            UiOverlay::ModelPicker { search, .. } => search.as_str(),
            _ => "",
        }
    }

    pub fn model_picker_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::ModelPicker { index, .. } => *index,
            _ => 0,
        }
    }

    pub fn model_picker_entries(&self) -> &[ModelPickerEntry] {
        match &self.overlay {
            UiOverlay::ModelPicker { entries, .. } => entries.as_slice(),
            _ => &[],
        }
    }

    pub fn model_picker_scroll(&self) -> usize {
        match &self.overlay {
            UiOverlay::ModelPicker { scroll, .. } => *scroll,
            _ => 0,
        }
    }

    pub fn permission_picker_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::PermissionPicker { index } => *index,
            _ => 0,
        }
    }

    pub fn agent_picker_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::AgentPicker { index } => *index,
            _ => 0,
        }
    }

    pub fn question_modal_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::QuestionModal { index, .. } => *index,
            _ => 0,
        }
    }

    pub fn question_modal_scroll(&self) -> usize {
        match &self.overlay {
            UiOverlay::QuestionModal { scroll, .. } => *scroll,
            _ => 0,
        }
    }

    pub fn session_picker_search(&self) -> &str {
        match &self.overlay {
            UiOverlay::SessionPicker { search, .. } => search.as_str(),
            _ => "",
        }
    }

    pub fn session_picker_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::SessionPicker { index, .. } => *index,
            _ => 0,
        }
    }

    pub fn session_picker_entries(&self) -> &[String] {
        match &self.overlay {
            UiOverlay::SessionPicker { entries, .. } => entries.as_slice(),
            _ => &[],
        }
    }

    pub fn session_picker_scroll(&self) -> usize {
        match &self.overlay {
            UiOverlay::SessionPicker { scroll, .. } => *scroll,
            _ => 0,
        }
    }

    pub fn provider_picker_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::ProviderPicker { index, .. } => *index,
            _ => 0,
        }
    }

    pub fn provider_picker_scroll(&self) -> usize {
        match &self.overlay {
            UiOverlay::ProviderPicker { scroll, .. } => *scroll,
            _ => 0,
        }
    }

    pub fn custom_provider_setup_step(&self) -> CustomProviderSetupStep {
        match &self.overlay {
            UiOverlay::CustomProviderSetup { step, .. } => *step,
            _ => CustomProviderSetupStep::Compatibility,
        }
    }

    pub fn custom_setup_compat_index(&self) -> usize {
        match &self.overlay {
            UiOverlay::CustomProviderSetup { compat_index, .. } => *compat_index,
            _ => 0,
        }
    }

    pub fn custom_setup_input(&self) -> &str {
        match &self.overlay {
            UiOverlay::CustomProviderSetup { input, .. } => input.as_str(),
            _ => "",
        }
    }

    pub fn custom_setup_base_url(&self) -> &str {
        match &self.overlay {
            UiOverlay::CustomProviderSetup { base_url, .. } => base_url.as_str(),
            _ => "",
        }
    }

    pub fn custom_setup_api_key(&self) -> &str {
        match &self.overlay {
            UiOverlay::CustomProviderSetup { api_key, .. } => api_key.as_str(),
            _ => "",
        }
    }

    pub fn custom_setup_model_hint(&self) -> &str {
        match &self.overlay {
            UiOverlay::CustomProviderSetup { model_hint, .. } => model_hint.as_str(),
            _ => "",
        }
    }

    pub fn command_palette_query_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::CommandPalette { query, .. } => Some(query),
            _ => None,
        }
    }

    pub fn palette_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::CommandPalette { palette_index, .. } => Some(palette_index),
            _ => None,
        }
    }

    pub fn branch_picker_query_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::BranchPicker { query, .. } => Some(query),
            _ => None,
        }
    }

    pub fn branch_picker_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::BranchPicker { index, .. } => Some(index),
            _ => None,
        }
    }

    pub fn branch_picker_branches_mut(&mut self) -> Option<&mut Vec<String>> {
        match &mut self.overlay {
            UiOverlay::BranchPicker { branches, .. } => Some(branches),
            _ => None,
        }
    }

    pub fn connect_search_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::ConnectModal { search, .. } => Some(search),
            _ => None,
        }
    }

    pub fn connect_menu_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::ConnectModal { menu_index, .. } => Some(menu_index),
            _ => None,
        }
    }

    pub fn connect_modal_scroll_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::ConnectModal { scroll, .. } => Some(scroll),
            _ => None,
        }
    }

    pub fn api_key_input_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::ApiKeyModal { input, .. } => Some(input),
            _ => None,
        }
    }

    pub fn info_modal_scroll_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::InfoModal { scroll, .. } => Some(scroll),
            _ => None,
        }
    }

    pub fn model_picker_search_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::ModelPicker { search, .. } => Some(search),
            _ => None,
        }
    }

    pub fn model_picker_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::ModelPicker { index, .. } => Some(index),
            _ => None,
        }
    }

    pub fn model_picker_scroll_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::ModelPicker { scroll, .. } => Some(scroll),
            _ => None,
        }
    }

    pub fn model_picker_entries_mut(&mut self) -> Option<&mut Vec<ModelPickerEntry>> {
        match &mut self.overlay {
            UiOverlay::ModelPicker { entries, .. } => Some(entries),
            _ => None,
        }
    }

    pub fn permission_picker_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::PermissionPicker { index } => Some(index),
            _ => None,
        }
    }

    pub fn agent_picker_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::AgentPicker { index } => Some(index),
            _ => None,
        }
    }

    pub fn question_modal_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::QuestionModal { index, .. } => Some(index),
            _ => None,
        }
    }

    pub fn question_modal_scroll_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::QuestionModal { scroll, .. } => Some(scroll),
            _ => None,
        }
    }

    pub fn session_picker_search_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::SessionPicker { search, .. } => Some(search),
            _ => None,
        }
    }

    pub fn session_picker_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::SessionPicker { index, .. } => Some(index),
            _ => None,
        }
    }

    pub fn session_picker_scroll_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::SessionPicker { scroll, .. } => Some(scroll),
            _ => None,
        }
    }

    pub fn provider_picker_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::ProviderPicker { index, .. } => Some(index),
            _ => None,
        }
    }

    pub fn provider_picker_scroll_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::ProviderPicker { scroll, .. } => Some(scroll),
            _ => None,
        }
    }

    pub fn custom_setup_compat_index_mut(&mut self) -> Option<&mut usize> {
        match &mut self.overlay {
            UiOverlay::CustomProviderSetup { compat_index, .. } => Some(compat_index),
            _ => None,
        }
    }

    pub fn custom_setup_input_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::CustomProviderSetup { input, .. } => Some(input),
            _ => None,
        }
    }

    pub fn custom_provider_setup_step_mut(&mut self) -> Option<&mut CustomProviderSetupStep> {
        match &mut self.overlay {
            UiOverlay::CustomProviderSetup { step, .. } => Some(step),
            _ => None,
        }
    }

    pub fn custom_setup_base_url_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::CustomProviderSetup { base_url, .. } => Some(base_url),
            _ => None,
        }
    }

    pub fn custom_setup_api_key_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::CustomProviderSetup { api_key, .. } => Some(api_key),
            _ => None,
        }
    }

    pub fn custom_setup_model_hint_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            UiOverlay::CustomProviderSetup { model_hint, .. } => Some(model_hint),
            _ => None,
        }
    }

    pub fn provider_picker_for_api_key(&self) -> bool {
        matches!(
            self.overlay,
            UiOverlay::ProviderPicker {
                for_api_key: true,
                ..
            }
        )
    }

    pub fn provider_picker_include_add_row(&self) -> bool {
        matches!(
            self.overlay,
            UiOverlay::ProviderPicker {
                include_add_row: true,
                ..
            }
        )
    }

    /// Bump the UI state version (forces a redraw on the next render tick).
    #[inline]
    pub fn mark_dirty(&mut self) {
        self.state_version = self.state_version.wrapping_add(1);
    }

    /// Bump both the state version and the transcript version (invalidates
    /// cached wrapped lines).
    #[inline]
    pub fn mark_transcript_dirty(&mut self) {
        self.state_version = self.state_version.wrapping_add(1);
        self.transcript_version = self.transcript_version.wrapping_add(1);
    }

    /// Throttled transcript invalidation while tokens stream in.
    pub fn mark_streaming_update(&mut self, delta_len: usize) {
        self.stream_chars_since_dirty += delta_len;
        let now = Instant::now();
        let elapsed = self
            .last_stream_transcript_dirty
            .map(|t| now.duration_since(t).as_millis())
            .unwrap_or(u128::MAX);
        if self.stream_chars_since_dirty >= STREAM_TRANSCRIPT_DIRTY_CHARS
            || elapsed >= STREAM_TRANSCRIPT_DIRTY_MS
        {
            self.stream_chars_since_dirty = 0;
            self.last_stream_transcript_dirty = Some(now);
            self.mark_transcript_dirty();
        } else {
            self.mark_dirty();
        }
    }

    pub fn flush_streaming_dirty(&mut self) {
        if self.stream_chars_since_dirty > 0 {
            self.stream_chars_since_dirty = 0;
            self.mark_transcript_dirty();
        }
    }

    pub fn open_connect_modal(&mut self) {
        self.set_overlay(UiOverlay::ConnectModal {
            search: String::new(),
            menu_index: 0,
            scroll: 0,
        });
    }

    pub fn close_connect_modal(&mut self) {
        if self.connect_modal_open() {
            self.close_overlay();
        }
    }

    pub fn open_api_key_modal(
        &mut self,
        provider: ProviderKind,
        has_existing: bool,
        connect_after_save: bool,
    ) {
        self.validation_status = None;
        self.set_overlay(UiOverlay::ApiKeyModal {
            provider,
            input: String::new(),
            has_existing,
            connect_after_save,
        });
    }

    pub fn close_api_key_modal(&mut self) {
        if self.api_key_modal_open() {
            self.validation_status = None;
            self.close_overlay();
        }
    }

    pub fn open_info_modal(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.set_overlay(UiOverlay::InfoModal {
            title: title.into(),
            lines,
            scroll: 0,
        });
    }

    pub fn close_info_modal(&mut self) {
        if self.info_modal_open() {
            self.close_overlay();
        }
    }

    pub fn open_model_picker(&mut self, entries: Vec<ModelPickerEntry>) {
        self.set_overlay(UiOverlay::ModelPicker {
            search: String::new(),
            index: 0,
            entries,
            scroll: 0,
        });
    }

    pub fn close_model_picker(&mut self) {
        if self.model_picker_open() {
            self.close_overlay();
        }
    }

    pub fn open_permission_picker(&mut self, current_index: usize) {
        self.set_overlay(UiOverlay::PermissionPicker {
            index: current_index,
        });
    }

    pub fn close_permission_picker(&mut self) {
        if self.permission_picker_open() {
            self.close_overlay();
        }
    }

    pub fn open_agent_picker(&mut self, current_index: usize) {
        self.set_overlay(UiOverlay::AgentPicker {
            index: current_index,
        });
    }

    pub fn close_agent_picker(&mut self) {
        if self.agent_picker_open() {
            self.close_overlay();
        }
    }

    pub fn open_question_modal(&mut self) {
        self.set_overlay(UiOverlay::QuestionModal {
            index: 0,
            scroll: 0,
        });
    }

    pub fn close_question_modal(&mut self) {
        if self.question_modal_open() {
            self.close_overlay();
        }
    }

    pub fn open_session_picker(&mut self, entries: Vec<String>, current: &str) {
        let index = entries.iter().position(|e| e == current).unwrap_or(0);
        self.set_overlay(UiOverlay::SessionPicker {
            search: String::new(),
            index,
            entries,
            scroll: 0,
        });
    }

    pub fn close_session_picker(&mut self) {
        if self.session_picker_open() {
            self.close_overlay();
        }
    }

    pub fn open_provider_picker(&mut self, current: ProviderKind, for_api_key: bool) {
        let index = ProviderKind::ALL
            .iter()
            .position(|p| *p == current)
            .unwrap_or(0);
        self.set_overlay(UiOverlay::ProviderPicker {
            index,
            scroll: 0,
            for_api_key,
            include_add_row: !for_api_key,
        });
        self.sync_provider_picker_scroll();
    }

    pub fn close_provider_picker(&mut self) {
        if self.provider_picker_open() {
            self.close_overlay();
        }
    }

    /// Row count for the open provider picker (built-ins plus optional “Add custom…” row).
    pub fn provider_picker_visible_row_count(&self) -> usize {
        ProviderKind::ALL.len() + usize::from(self.provider_picker_include_add_row())
    }

    /// Max provider rows shown at once (smaller than [`ProviderKind::ALL`] so short terminals can scroll).
    pub const PROVIDER_PICKER_VISIBLE_ROWS: usize = 4;

    /// Keep provider picker index inside the visible window.
    pub fn sync_provider_picker_scroll(&mut self) {
        let n = self.provider_picker_visible_row_count();
        let cap = Self::PROVIDER_PICKER_VISIBLE_ROWS.min(n.max(1));
        let UiOverlay::ProviderPicker { index, scroll, .. } = &mut self.overlay else {
            return;
        };
        if *index < *scroll {
            *scroll = *index;
        }
        while *index >= *scroll + cap {
            *scroll += 1;
        }
        let max_scroll = n.saturating_sub(cap);
        if *scroll > max_scroll {
            *scroll = max_scroll;
        }
    }

    pub fn open_custom_provider_setup(&mut self, model_hint: impl Into<String>) {
        self.set_overlay(UiOverlay::CustomProviderSetup {
            step: CustomProviderSetupStep::Compatibility,
            compat_index: 0,
            input: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            model_hint: model_hint.into(),
        });
    }

    pub fn close_custom_provider_setup(&mut self) {
        if self.custom_provider_setup_open() {
            self.close_overlay();
        }
    }

    pub fn set_busy(&mut self, busy: bool) {
        if self.busy != busy {
            self.busy = busy;
            self.mark_dirty();
        }
    }

    pub fn set_busy_state(&mut self, state: BusyState) {
        if self.current_busy_state != state {
            self.current_busy_state = state;
            self.busy_state_since = Instant::now();
            self.mark_dirty();
        }
    }

    pub fn push_error(&mut self, msg: String) {
        self.blocks.push(DisplayBlock::ErrorLine(msg));
        self.mark_transcript_dirty();
    }

    /// Newest committed assistant response, or non-empty streaming text when
    /// nothing has been committed yet.
    pub fn last_assistant_text(&self) -> Option<&str> {
        for block in self.blocks.iter().rev() {
            if let DisplayBlock::Assistant(text) = block {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(text.as_str());
                }
            }
        }
        self.streaming_assistant
            .as_deref()
            .filter(|s| !s.trim().is_empty())
    }

    /// Seed or replace the todo list (e.g. from a resumed session snapshot).
    pub fn set_todos(&mut self, todos: Vec<AgentTodo>) {
        self.todos = todos;
        self.mark_dirty();
    }

    /// Progress summary like `2/5 done`.
    pub fn todo_progress(&self) -> (usize, usize) {
        let done = self
            .todos
            .iter()
            .filter(|t| matches!(t.status, TodoStatus::Completed | TodoStatus::Cancelled))
            .count();
        (done, self.todos.len())
    }

    /// Compact sidebar rows: up to `limit` items, with `+N more` overflow.
    pub fn todo_sidebar_lines(&self, limit: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let (done, total) = self.todo_progress();
        if total == 0 {
            lines.push("none yet".into());
            return lines;
        }
        lines.push(format!("{done}/{total} done"));
        for todo in self.todos.iter().take(limit) {
            lines.push(format!(
                "{} {}",
                todo.status.glyph(),
                truncate(&todo.content, 24)
            ));
        }
        if self.todos.len() > limit {
            lines.push(format!("+{} more", self.todos.len() - limit));
        }
        lines
    }

    /// Full list for the `/todos` info modal.
    pub fn todo_modal_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let (done, total) = self.todo_progress();
        lines.push(format!("Session todos ({done}/{total} done)"));
        lines.push(String::new());
        if self.todos.is_empty() {
            lines.push("No todos yet. The agent updates them via `update_todos`.".into());
            return lines;
        }
        for todo in &self.todos {
            let source = match todo.source {
                Some(nca_common::todo::TodoSource::Agent) => " [agent]",
                Some(nca_common::todo::TodoSource::Plan) => " [plan]",
                Some(nca_common::todo::TodoSource::User) => " [user]",
                None => "",
            };
            lines.push(format!(
                "{} [{}] {}{}",
                todo.status.glyph(),
                todo.id,
                todo.content,
                source
            ));
        }
        lines
    }

    /// Drop live approval/question prompts so cancel returns the composer to normal input.
    pub fn dismiss_interactive_prompts(&mut self) {
        let had = self.active_approval.is_some() || self.active_question.is_some();
        self.active_approval = None;
        self.active_question = None;
        self.close_question_modal();
        if had {
            self.blocks.push(DisplayBlock::System(
                "Dismissed pending approval/question (cancelled)".into(),
            ));
            self.mark_transcript_dirty();
            self.mark_dirty();
        }
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
            self.mark_dirty();
        }
    }

    pub fn set_agent_profile(&mut self, label: &str) {
        if self.agent_profile != label {
            self.agent_profile = label.to_string();
            self.mark_dirty();
        }
    }

    pub fn set_current_branch(&mut self, branch: &str) {
        if self.current_branch != branch {
            self.current_branch = branch.to_string();
            self.mark_dirty();
        }
    }

    pub fn open_branch_picker(&mut self, branches: Vec<String>, current: &str) {
        let index = branches.iter().position(|b| b == current).unwrap_or(0);
        self.set_overlay(UiOverlay::BranchPicker {
            query: String::new(),
            index,
            branches,
        });
    }

    pub fn close_branch_picker(&mut self) {
        if self.branch_picker_open() {
            self.close_overlay();
        }
    }

    pub fn open_command_palette(&mut self) {
        self.set_overlay(UiOverlay::CommandPalette {
            query: String::new(),
            palette_index: 0,
        });
    }

    pub fn close_command_palette(&mut self) {
        if self.command_palette_open() {
            self.close_overlay();
        }
    }

    pub fn set_permission_mode(&mut self, mode: &str) {
        if self.permission_mode != mode {
            self.permission_mode = mode.to_string();
            self.mark_dirty();
        }
    }

    fn flush_stream_before_tool(&mut self) {
        if let Some(s) = self.streaming_assistant.take()
            && !s.trim().is_empty()
        {
            self.blocks.push(DisplayBlock::Assistant(s));
        }
    }

    pub fn apply_event(&mut self, e: &AgentEvent) {
        // Cheap events (cost/checkpoint) only dirty the status surface; everything
        // else also invalidates the wrapped transcript cache.
        match e {
            AgentEvent::CostUpdated { .. } | AgentEvent::Checkpoint { .. } => self.mark_dirty(),
            AgentEvent::TokensStreamed { .. } => {}
            _ => self.mark_transcript_dirty(),
        }
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
                self.flush_streaming_dirty();
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
                self.mark_streaming_update(delta.len());
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
                        tokens_in: 0,
                        tokens_out: 0,
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
                let (parsed_in, parsed_out) = if phase == "tokens" {
                    let mut parts = detail.splitn(2, '/');
                    let i = parts
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    let o = parts
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    (Some(i), Some(o))
                } else {
                    (None, None)
                };
                if let Some(row) = self
                    .subagents
                    .iter_mut()
                    .find(|r| r.id == *child_session_id)
                {
                    if phase != "tokens" {
                        row.phase = phase.clone();
                        row.detail = d.clone();
                    }
                    row.running = true;
                    if phase == "skill" || phase == "invoke_skill" {
                        row.skill = Some(detail.clone());
                    }
                    if let (Some(i), Some(o)) = (parsed_in, parsed_out) {
                        row.tokens_in = i;
                        row.tokens_out = o;
                    }
                } else {
                    self.subagents.push(SubagentRow {
                        id: child_session_id.clone(),
                        task: "(sub-agent)".into(),
                        phase: if phase == "tokens" {
                            String::new()
                        } else {
                            phase.clone()
                        },
                        detail: if phase == "tokens" {
                            String::new()
                        } else {
                            d.clone()
                        },
                        running: true,
                        skill: if phase == "skill" || phase == "invoke_skill" {
                            Some(detail.clone())
                        } else {
                            None
                        },
                        tokens_in: parsed_in.unwrap_or(0),
                        tokens_out: parsed_out.unwrap_or(0),
                    });
                }
                if phase != "tokens" {
                    self.blocks
                        .push(DisplayBlock::System(format!("↳ {short}… · {phase} · {d}")));
                }
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
            AgentEvent::TodosUpdated { todos } => {
                self.todos = todos.clone();
                self.mark_dirty();
            }
            AgentEvent::ContextCompaction {
                phase,
                message,
                tokens_before,
                tokens_after,
                retained_groups,
                dropped_groups,
            } => {
                self.context_report = Some(ContextCompactionReport {
                    phase: phase.clone(),
                    message: message.clone(),
                    tokens_before: *tokens_before,
                    tokens_after: *tokens_after,
                    retained_groups: *retained_groups,
                    dropped_groups: *dropped_groups,
                });
                // Only surface a concise system line for completed phases to avoid spam.
                if phase == "completed" || phase == "dry_run" {
                    self.blocks
                        .push(DisplayBlock::System(format!("[context] {message}")));
                }
                self.mark_dirty();
            }
            AgentEvent::ContextWarning { message } => {
                self.blocks
                    .push(DisplayBlock::System(format!("[context] {message}")));
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
        assert!(!st.question_modal_open());
        assert_eq!(st.question_modal_index(), 0);

        st.open_question_modal();
        assert!(st.question_modal_open());
        assert_eq!(st.question_modal_index(), 0);
        assert_eq!(st.question_modal_scroll(), 0);

        st.set_overlay(UiOverlay::QuestionModal {
            index: 3,
            scroll: 0,
        });
        st.close_question_modal();
        assert!(!st.question_modal_open());
        assert_eq!(st.overlay, UiOverlay::None);
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
        assert!(st.question_modal_open());
        assert_eq!(st.question_modal_index(), 0);
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
        st.set_overlay(UiOverlay::QuestionModal {
            index: 2,
            scroll: 0,
        });
        st.apply_event(&AgentEvent::QuestionResolved {
            question_id: "q-1".into(),
            selection: QuestionSelection::Suggested,
        });
        assert!(st.active_question.is_none());
        assert!(!st.question_modal_open());
        assert_eq!(st.question_modal_index(), 0);
    }

    #[test]
    fn last_assistant_text_prefers_newest_committed() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        assert!(st.last_assistant_text().is_none());

        st.blocks
            .push(DisplayBlock::Assistant("first response".into()));
        st.blocks
            .push(DisplayBlock::Assistant("second response".into()));
        st.streaming_assistant = Some("partial".into());
        assert_eq!(st.last_assistant_text(), Some("second response"));
    }

    #[test]
    fn last_assistant_text_falls_back_to_streaming() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.streaming_assistant = Some("still streaming".into());
        assert_eq!(st.last_assistant_text(), Some("still streaming"));

        st.blocks.push(DisplayBlock::Assistant("   ".into()));
        assert_eq!(st.last_assistant_text(), Some("still streaming"));
    }

    #[test]
    fn last_assistant_text_empty_transcript() {
        let st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        assert!(st.last_assistant_text().is_none());
    }

    #[test]
    fn todos_updated_replaces_list() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.apply_event(&AgentEvent::TodosUpdated {
            todos: vec![
                AgentTodo {
                    id: "1".into(),
                    content: "First".into(),
                    status: TodoStatus::Completed,
                    source: None,
                },
                AgentTodo {
                    id: "2".into(),
                    content: "Second longer task name here".into(),
                    status: TodoStatus::InProgress,
                    source: None,
                },
            ],
        });
        assert_eq!(st.todos.len(), 2);
        assert_eq!(st.todo_progress(), (1, 2));
        let sidebar = st.todo_sidebar_lines(6);
        assert!(sidebar[0].contains("1/2"));
        assert!(sidebar.iter().any(|l| l.contains("◉")));

        st.apply_event(&AgentEvent::TodosUpdated { todos: vec![] });
        assert!(st.todos.is_empty());
        assert_eq!(st.todo_modal_lines()[0], "Session todos (0/0 done)");
    }

    #[test]
    fn streaming_stress_many_tokens_throttles_transcript_version() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "@build".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        let start_tv = st.transcript_version;
        for i in 0..500 {
            st.apply_event(&AgentEvent::TokensStreamed {
                delta: format!("tok{i} "),
            });
        }
        let bumps = st.transcript_version - start_tv;
        assert!(
            bumps < 50,
            "expected throttled transcript invalidation, got {bumps} bumps"
        );
        assert!(st.state_version > start_tv + 100);
    }
}
