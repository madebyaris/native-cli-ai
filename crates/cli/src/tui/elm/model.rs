//! Top-level Elm model: tick/update/view loop.

#![allow(clippy::collapsible_if)]

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use nca_common::config::ProviderKind;
use nca_common::event::{AgentEvent, BusyState, InteractiveQuestionPayload, QuestionSelection};
use nca_common::message::ImageAttachment;

use super::comp_id::CompId;
use super::components::transcript::TranscriptAction;
use super::components::{
    Components, agent_picker, api_key_modal, branch_picker, command_palette, connect_modal,
    info_modal, model_picker, permission_picker, provider_picker, session_picker,
};
use super::feedback::TuiFeedbackMsg;
use super::msg::Msg;
use crate::tui::app::TuiCmd;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc::UnboundedSender;

// ── Side-effect channels ─────────────────────────────────────────

/// External channels for side-effects from the TUI to the runtime.
pub(crate) struct SideEffectChannels {
    pub question_answer_tx: Option<UnboundedSender<(String, QuestionSelection)>>,
    pub approval_answer_tx: Option<UnboundedSender<crate::tui::app::ApprovalAnswer>>,
    pub cancel_flag: Option<Arc<AtomicBool>>,
    /// Shared active question ID for synchronous reads by the runtime.
    pub active_question_id: Arc<StdMutex<Option<String>>>,
    /// Shared active question payload for answer parsing.
    pub active_question_payload: Arc<StdMutex<Option<InteractiveQuestionPayload>>>,
    /// Shared active approval request for answer routing.
    pub active_approval_payload: Arc<StdMutex<Option<crate::tui::state::ApprovalRequest>>>,
    /// Shared staged images for synchronous reads by the runtime.
    pub staged_images: Arc<StdMutex<Vec<ImageAttachment>>>,
}

// ── NcaModel ─────────────────────────────────────────────────────

/// Top-level application model for the Elm architecture.
///
/// Runs inside `spawn_blocking` on a dedicated thread.
/// Receives events from two sources:
/// - crossterm terminal events (polled every tick)
/// - TuiFeedbackMsg messages (from runtime via unbounded channel)
pub(crate) struct NcaModel {
    /// All concrete component instances.
    pub(crate) components: Components,
    /// Currently focused component.
    pub(crate) focus: CompId,
    /// Channel to receive messages from the runtime.
    pub(crate) bridge_rx: tokio::sync::mpsc::UnboundedReceiver<TuiFeedbackMsg>,
    /// Channel to send commands to the external runtime.
    pub(crate) cmd_tx: tokio::sync::mpsc::UnboundedSender<Msg>,
    /// External side-effect channels.
    pub(crate) side_effects: SideEffectChannels,
    /// Whether a redraw is needed.
    pub(crate) redraw: bool,
    /// Whether the application should exit.
    pub(crate) quit: bool,
    /// Whether any popup is currently open (affects global key interception).
    pub(crate) popup_open: bool,
    /// Terminal size (updated on Resize events).
    pub(crate) size: (u16, u16),
}

impl NcaModel {
    pub(crate) fn new(
        bridge_rx: tokio::sync::mpsc::UnboundedReceiver<TuiFeedbackMsg>,
        cmd_tx: tokio::sync::mpsc::UnboundedSender<Msg>,
        side_effects: SideEffectChannels,
    ) -> Self {
        Self {
            components: Components::new(),
            focus: CompId::Composer,
            bridge_rx,
            cmd_tx,
            side_effects,
            redraw: true,
            quit: false,
            popup_open: false,
            size: (80, 24),
        }
    }

    /// Main tick: poll crossterm, drain bridge, redraw if dirty.
    ///
    /// Returns `Ok(())` normally, error on terminal I/O failure.
    pub(crate) fn tick(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> anyhow::Result<()> {
        // 1. Poll crossterm events (40ms timeout)
        if crossterm::event::poll(std::time::Duration::from_millis(40))?
            && let Ok(event) = crossterm::event::read()
        {
            match event {
                crossterm::event::Event::Key(key) => {
                    // Transcript scroll keys need layout area — handle directly
                    if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown | KeyCode::End) {
                        let areas = self.compute_layout(Rect::new(0, 0, self.size.0, self.size.1));
                        self.components.transcript.handle_key(key, areas.transcript);
                        self.redraw = true;
                    } else {
                        self.handle_key(key);
                    }
                }
                crossterm::event::Event::Mouse(mouse) => {
                    self.handle_mouse(mouse);
                    self.redraw = true;
                }
                crossterm::event::Event::Paste(text) => {
                    self.handle_paste(&text);
                }
                crossterm::event::Event::Resize(w, h) => {
                    self.size = (w, h);
                    self.redraw = true;
                }
                _ => {}
            }
        }

        // 2. Drain bridge events (non-blocking)
        while let Ok(msg) = self.bridge_rx.try_recv() {
            self.update_feedback(msg);
        }

        // 3. Sync popup_open flag
        self.popup_open = self.components.sync_popup_open();

        // 4. Check quit
        if self.quit {
            return Ok(());
        }

        // 5. Render if dirty
        if self.redraw {
            self.view(terminal);
        }

        Ok(())
    }

    // ── Event handlers ──────────────────────────────────────────

    /// Full key event routing with popup priority.
    fn handle_key(&mut self, key: KeyEvent) {
        // 1. Check mounted popups (they intercept all keys when open)
        if self.popup_open {
            if let Some(msg) = self.dispatch_popup_key(key) {
                match msg {
                    Msg::Quit => {
                        self.quit = true;
                    }
                    Msg::Cmd(cmd) => {
                        let _ = self.cmd_tx.send(Msg::Cmd(cmd));
                    }
                    Msg::Redraw => {
                        self.redraw = true;
                    }
                    _ => {}
                }
            }
            return;
        }

        // 2. Global listener (Ctrl+Q, Ctrl+X leader, Ctrl+P, etc.)
        if let Some(msg) = self.components.global_listener.on(&Msg::Key(key)) {
            self.process_global_msg(msg);
            return;
        }

        // 3. Dispatch to focused component
        if self.focus == CompId::Composer {
            if let Some(msg) = self.components.composer.handle_key(key) {
                self.process_component_msg(msg);
            }
            self.redraw = true;
        }
    }

    /// Route a key event to the currently mounted popup. Returns `Some(Msg)` if consumed.
    fn dispatch_popup_key(&mut self, key: KeyEvent) -> Option<Msg> {
        if let Some(ref mut p) = self.components.command_palette {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    command_palette::CommandPaletteAction::None => Msg::Redraw,
                    command_palette::CommandPaletteAction::Close => {
                        self.components.command_palette = None;
                        Msg::Redraw
                    }
                    command_palette::CommandPaletteAction::SubmitCommand(cmd) => {
                        self.components.command_palette = None;
                        Msg::Cmd(TuiCmd::Submit(cmd))
                    }
                };
                return Some(msg);
            }
        }
        if let Some(ref mut p) = self.components.info_modal {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    info_modal::InfoModalAction::None => Msg::Redraw,
                    info_modal::InfoModalAction::Close => {
                        self.components.info_modal = None;
                        Msg::Redraw
                    }
                };
                return Some(msg);
            }
        }
        if let Some(ref mut p) = self.components.model_picker {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    model_picker::ModelPickerPopupAction::None => Msg::Redraw,
                    model_picker::ModelPickerPopupAction::Close => {
                        self.components.model_picker = None;
                        Msg::Redraw
                    }
                    model_picker::ModelPickerPopupAction::ApplyModel(m) => {
                        self.components.model_picker = None;
                        Msg::Cmd(TuiCmd::ApplyModel(m))
                    }
                    model_picker::ModelPickerPopupAction::SwitchProvider(p) => {
                        self.components.model_picker = None;
                        Msg::Cmd(TuiCmd::ApplyModelProvider(p))
                    }
                };
                return Some(msg);
            }
        }
        if let Some(ref mut p) = self.components.connect_modal {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    connect_modal::ConnectModalAction::None => Msg::Redraw,
                    connect_modal::ConnectModalAction::Close => {
                        self.components.connect_modal = None;
                        Msg::Redraw
                    }
                    connect_modal::ConnectModalAction::ConnectProvider(p) => {
                        self.components.connect_modal = None;
                        Msg::Cmd(TuiCmd::ApplyDefaultProvider(p))
                    }
                };
                return Some(msg);
            }
        }
        if let Some(ref mut p) = self.components.api_key_modal {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    api_key_modal::ApiKeyModalAction::None => Msg::Redraw,
                    api_key_modal::ApiKeyModalAction::Close => {
                        self.components.api_key_modal = None;
                        Msg::Redraw
                    }
                    api_key_modal::ApiKeyModalAction::Confirm(key) => {
                        let provider = self
                            .components
                            .api_key_modal
                            .as_ref()
                            .and_then(|p| p.provider())
                            .unwrap_or(ProviderKind::MiniMax);
                        self.components.api_key_modal = None;
                        Msg::Cmd(TuiCmd::ValidateApiKey(provider, key))
                    }
                };
                return Some(msg);
            }
        }
        if let Some(ref mut p) = self.components.provider_picker {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    provider_picker::ProviderPickerAction::None => Msg::Redraw,
                    provider_picker::ProviderPickerAction::Close => {
                        self.components.provider_picker = None;
                        Msg::Redraw
                    }
                    provider_picker::ProviderPickerAction::ApplyDefaultProvider(p) => {
                        self.components.provider_picker = None;
                        Msg::Cmd(TuiCmd::ApplyDefaultProvider(p))
                    }
                    provider_picker::ProviderPickerAction::PromptApiKey(p) => {
                        self.components.provider_picker = None;
                        Msg::Cmd(TuiCmd::PromptApiKey(p, false))
                    }
                };
                return Some(msg);
            }
        }
        if let Some(ref mut p) = self.components.branch_picker {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    branch_picker::BranchPickerAction::None => Msg::Redraw,
                    branch_picker::BranchPickerAction::Close => {
                        self.components.branch_picker = None;
                        Msg::Redraw
                    }
                    branch_picker::BranchPickerAction::SwitchBranch(name) => {
                        self.components.branch_picker = None;
                        Msg::Cmd(TuiCmd::SwitchBranch(name))
                    }
                    branch_picker::BranchPickerAction::CreateBranch(name) => {
                        self.components.branch_picker = None;
                        Msg::Cmd(TuiCmd::CreateBranch(name))
                    }
                };
                return Some(msg);
            }
        }
        if let Some(ref mut p) = self.components.permission_picker {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    permission_picker::PermissionPickerAction::None => Msg::Redraw,
                    permission_picker::PermissionPickerAction::Close => {
                        self.components.permission_picker = None;
                        Msg::Redraw
                    }
                    permission_picker::PermissionPickerAction::ApplyPermission(idx) => {
                        self.components.permission_picker = None;
                        Msg::Cmd(TuiCmd::ApplyPermission(idx))
                    }
                };
                return Some(msg);
            }
        }
        if let Some(ref mut p) = self.components.agent_picker {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    agent_picker::AgentPickerAction::None => Msg::Redraw,
                    agent_picker::AgentPickerAction::Close => {
                        self.components.agent_picker = None;
                        Msg::Redraw
                    }
                    agent_picker::AgentPickerAction::SwitchAgent(idx) => {
                        self.components.agent_picker = None;
                        Msg::Cmd(TuiCmd::SwitchAgent(idx))
                    }
                };
                return Some(msg);
            }
        }
        if let Some(ref mut p) = self.components.session_picker {
            if p.is_open() {
                let action = p.handle_key(key);
                let msg = match action {
                    session_picker::SessionPickerAction::None => Msg::Redraw,
                    session_picker::SessionPickerAction::Close => {
                        self.components.session_picker = None;
                        Msg::Redraw
                    }
                    session_picker::SessionPickerAction::ResumeSession(id) => {
                        self.components.session_picker = None;
                        Msg::Cmd(TuiCmd::ResumeSession(id))
                    }
                };
                return Some(msg);
            }
        }
        None
    }

    /// Handle mouse events (transcript scroll, etc.)
    fn handle_mouse(&mut self, mouse: MouseEvent) {
        // Transcript mouse handling needs layout area
        let areas = self.compute_layout(Rect::new(0, 0, self.size.0, self.size.1));
        let inner_w = areas.transcript.width.saturating_sub(2);
        let total = self.components.transcript.total_line_count(inner_w);
        let action = self
            .components
            .transcript
            .handle_mouse(&mouse, areas.transcript, total);
        self.handle_transcript_action(action);
    }

    /// Handle paste events.
    fn handle_paste(&mut self, text: &str) {
        if self.popup_open {
            return; // Swallow paste when popup is open
        }
        self.components.composer.handle_paste(text);
        self.redraw = true;
    }

    // ── Feedback processing ───────────────────────────────────────

    /// Process a single feedback message from the runtime.
    fn update_feedback(&mut self, msg: TuiFeedbackMsg) {
        self.redraw = true;
        match msg {
            TuiFeedbackMsg::Agent(e) => {
                // Extract metadata for status bar BEFORE passing to transcript
                match &e {
                    AgentEvent::SessionStarted {
                        session_id, model, ..
                    } => {
                        self.components.status_bar.update_session(session_id, model);
                    }
                    AgentEvent::CostUpdated {
                        input_tokens,
                        output_tokens,
                        estimated_cost_usd,
                        ..
                    } => {
                        self.components.status_bar.update_cost(
                            *input_tokens,
                            *output_tokens,
                            *estimated_cost_usd,
                        );
                    }
                    AgentEvent::ContextStatsUpdated {
                        context_window,
                        usage_percent,
                        ..
                    } => {
                        self.components
                            .status_bar
                            .update_context(*context_window, *usage_percent as usize);
                    }
                    AgentEvent::BusyStateChanged { state } => {
                        self.components.status_bar.set_busy(*state);
                    }
                    AgentEvent::QuestionRequested { question } => {
                        self.components.status_bar.set_active_question(true);
                        self.components
                            .transcript
                            .set_active_question(Some(question.clone()));
                        self.components.state_mut().active_question = true;
                        if let Ok(mut guard) = self.side_effects.active_question_id.lock() {
                            *guard = Some(question.question_id.clone());
                        }
                        if let Ok(mut guard) = self.side_effects.active_question_payload.lock() {
                            *guard = Some(question.clone());
                        }
                    }
                    AgentEvent::QuestionResolved { .. } => {
                        self.components.status_bar.set_active_question(false);
                        self.components.transcript.set_active_question(None);
                        self.components.state_mut().active_question = false;
                        if let Ok(mut guard) = self.side_effects.active_question_id.lock() {
                            *guard = None;
                        }
                        if let Ok(mut guard) = self.side_effects.active_question_payload.lock() {
                            *guard = None;
                        }
                    }
                    AgentEvent::ApprovalRequested {
                        call_id,
                        tool,
                        description,
                    } => {
                        let req = crate::tui::state::ApprovalRequest {
                            call_id: call_id.clone(),
                            tool: tool.clone(),
                            description: description.clone(),
                            input: String::new(),
                        };
                        self.components.status_bar.set_active_approval(true);
                        self.components
                            .transcript
                            .set_active_approval(Some(req.clone()));
                        self.components.state_mut().active_approval = true;
                        if let Ok(mut guard) = self.side_effects.active_approval_payload.lock() {
                            *guard = Some(req);
                        }
                    }
                    AgentEvent::ApprovalResolved { .. } => {
                        self.components.status_bar.set_active_approval(false);
                        self.components.transcript.set_active_approval(None);
                        self.components.state_mut().active_approval = false;
                        if let Ok(mut guard) = self.side_effects.active_approval_payload.lock() {
                            *guard = None;
                        }
                    }
                    _ => {}
                }
                // Delegate to transcript for blocks/streaming
                let action = self.components.transcript.apply_event(&e);
                self.handle_transcript_action(action);
            }
            TuiFeedbackMsg::ShouldExit => {
                self.quit = true;
            }
            TuiFeedbackMsg::SetBusyState(state) => {
                self.components.status_bar.set_busy(state);
            }
            TuiFeedbackMsg::PushSystem(msg) => {
                self.components.transcript.push_system(msg);
            }
            TuiFeedbackMsg::PushError(msg) => {
                self.components.transcript.push_error(msg);
            }
            TuiFeedbackMsg::PushBlocks(blocks) => {
                self.components.transcript.push_blocks(blocks);
            }
            TuiFeedbackMsg::SetStreamingAssistant(text) => {
                self.components.transcript.set_streaming_assistant(text);
            }
            TuiFeedbackMsg::SetStreamingReasoning(text) => {
                self.components.transcript.set_streaming_reasoning(text);
            }
            TuiFeedbackMsg::ClearTranscript => {
                self.components.transcript.clear();
            }
            TuiFeedbackMsg::SetActiveApproval(req) => {
                let has_approval = req.is_some();
                self.components.status_bar.set_active_approval(has_approval);
                self.components.transcript.set_active_approval(req.clone());
                self.components.state_mut().active_approval = has_approval;
                // Store approval request for answer routing (need call_id).
                if let Ok(mut guard) = self.side_effects.active_approval_payload.lock() {
                    *guard = req;
                }
            }
            TuiFeedbackMsg::SetActiveQuestion(q) => {
                self.components.status_bar.set_active_question(q.is_some());
                let has_question = q.is_some();
                self.components.transcript.set_active_question(q.clone());
                self.components.state_mut().active_question = has_question;
                // Update shared state for synchronous reads by the runtime.
                if let Ok(mut guard) = self.side_effects.active_question_id.lock() {
                    *guard = q.as_ref().map(|q| q.question_id.clone());
                }
                if let Ok(mut guard) = self.side_effects.active_question_payload.lock() {
                    *guard = q;
                }
            }
            TuiFeedbackMsg::SetModel(model) => {
                self.components.status_bar.update_model(&model);
            }
            TuiFeedbackMsg::SetPermissionMode(mode) => {
                self.components.status_bar.update_permission_mode(&mode);
            }
            TuiFeedbackMsg::SetAgentProfile(label) => {
                self.components.status_bar.update_agent_profile(&label);
            }
            TuiFeedbackMsg::SetCurrentBranch(branch) => {
                self.components.status_bar.update_branch(&branch);
            }
            TuiFeedbackMsg::OpenBranchPicker { branches, current } => {
                self.components
                    .branch_picker
                    .get_or_insert_with(branch_picker::BranchPickerState::new)
                    .open(branches, current);
                self.popup_open = true;
            }
            TuiFeedbackMsg::OpenApiKeyModal {
                provider,
                has_key,
                connect_after,
            } => {
                self.components
                    .api_key_modal
                    .get_or_insert_with(api_key_modal::ApiKeyModalState::new)
                    .open(provider, has_key, connect_after);
                self.popup_open = true;
            }
            // ── Phase 3b-1 additions ──
            TuiFeedbackMsg::OpenInfoModal { title, lines } => {
                self.components
                    .info_modal
                    .get_or_insert_with(info_modal::InfoModalState::new)
                    .open(title, lines);
                self.popup_open = true;
            }
            TuiFeedbackMsg::OpenModelPicker { entries } => {
                let elm_entries: Vec<model_picker::ModelPickerEntry> = entries
                    .into_iter()
                    .map(|e| model_picker::ModelPickerEntry {
                        label: e.label,
                        detail: e.detail,
                        action: match e.action {
                            crate::tui::state::ModelPickerAction::SwitchProvider(p) => {
                                model_picker::ModelPickerAction::SwitchProvider(p)
                            }
                            crate::tui::state::ModelPickerAction::ApplyModel(m) => {
                                model_picker::ModelPickerAction::ApplyModel(m)
                            }
                        },
                        is_header: e.is_header,
                    })
                    .collect();
                self.components
                    .model_picker
                    .get_or_insert_with(model_picker::ModelPickerState::new)
                    .open(elm_entries);
                self.popup_open = true;
            }
            TuiFeedbackMsg::OpenAgentPicker { current_index } => {
                self.components
                    .agent_picker
                    .get_or_insert_with(agent_picker::AgentPickerState::new)
                    .open(current_index);
                self.popup_open = true;
            }
            TuiFeedbackMsg::OpenPermissionPicker { current_index } => {
                self.components
                    .permission_picker
                    .get_or_insert_with(permission_picker::PermissionPickerState::new)
                    .open(current_index);
                self.popup_open = true;
            }
            TuiFeedbackMsg::OpenProviderPicker { for_api_key } => {
                self.components
                    .provider_picker
                    .get_or_insert_with(provider_picker::ProviderPickerState::new)
                    .open(for_api_key);
                self.popup_open = true;
            }
            TuiFeedbackMsg::OpenSessionPicker { entries, current } => {
                self.components
                    .session_picker
                    .get_or_insert_with(session_picker::SessionPickerState::new)
                    .open(entries, current);
                self.popup_open = true;
            }
            TuiFeedbackMsg::OpenConnectModal => {
                self.components
                    .connect_modal
                    .get_or_insert_with(connect_modal::ConnectModalState::new)
                    .open();
                self.popup_open = true;
            }
            TuiFeedbackMsg::CloseApiKeyModal => {
                self.components.api_key_modal = None;
                self.popup_open = self.components.is_any_popup_open();
            }
            TuiFeedbackMsg::CloseConnectModal => {
                self.components.connect_modal = None;
                self.popup_open = self.components.is_any_popup_open();
            }
            TuiFeedbackMsg::SetBusy(busy) => {
                let state = if busy {
                    BusyState::Thinking
                } else {
                    BusyState::Idle
                };
                self.components.status_bar.set_busy(state);
            }
            TuiFeedbackMsg::ResetSessionState { session_id, model } => {
                self.components.transcript.clear();
                self.components
                    .status_bar
                    .update_session(&session_id, &model);
                self.components.status_bar.update_cost(0, 0, 0.0);
            }
            TuiFeedbackMsg::SetOnboardingMode(onboarding) => {
                // Stored for future onboarding flow; no visual effect in Elm yet.
                let _ = onboarding;
            }
            TuiFeedbackMsg::SetValidationStatus { status } => {
                // Stored for api_key_modal rendering; no visual effect in Elm yet.
                let _ = status;
            }
            TuiFeedbackMsg::SetPendingApiKeyProvider { provider } => {
                // Stored for Submit branch logic; no visual effect in Elm yet.
                let _ = provider;
            }
            TuiFeedbackMsg::PushStagedImage { attachment } => {
                // Track staged image count in composer state for display.
                self.components.state_mut().staged_image_count += 1;
                // Update shared state for synchronous reads by the runtime.
                if let Ok(mut guard) = self.side_effects.staged_images.lock() {
                    guard.push(attachment);
                }
            }
            TuiFeedbackMsg::ClearStagedImages => {
                self.components.state_mut().staged_image_count = 0;
                // Update shared state for synchronous reads by the runtime.
                if let Ok(mut guard) = self.side_effects.staged_images.lock() {
                    guard.clear();
                }
            }
            TuiFeedbackMsg::SetInputBuffer { text, cursor } => {
                let s = self.components.state_mut();
                s.input_buffer = text;
                s.cursor_char_idx = cursor;
            }
            TuiFeedbackMsg::ToggleToolOutput { block_index } => {
                self.components.transcript.toggle_tool_output(block_index);
            }
            TuiFeedbackMsg::ToggleAllToolOutput => {
                self.components.transcript.toggle_all_tool_output();
            }
        }
    }

    // ── Transcript action handling ──────────────────────────────

    fn handle_transcript_action(&mut self, action: TranscriptAction) {
        match action {
            TranscriptAction::None => {}
            TranscriptAction::QuestionAnswer(sel) => {
                if let Some(ref tx) = self.side_effects.question_answer_tx
                    && let Some(ref q) = self.components.transcript.active_question_id()
                {
                    let _ = tx.send((q.clone(), sel));
                }
            }
            TranscriptAction::CopyToClipboard(text) => {
                match crate::image_attach::copy_text_to_clipboard(&text) {
                    Ok(()) => {
                        self.components.transcript.push_system(format!(
                            "Copied {} chars to clipboard",
                            text.trim_end_matches('\n').chars().count()
                        ));
                    }
                    Err(e) => {
                        self.components
                            .transcript
                            .push_error(format!("Clipboard failed: {e}"));
                    }
                }
            }
            TranscriptAction::PushSystem(msg) => {
                self.components.transcript.push_system(msg);
            }
            TranscriptAction::PushError(msg) => {
                self.components.transcript.push_error(msg);
            }
        }
    }

    // ── Global message processing ─────────────────────────────────

    fn process_global_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Quit => {
                // Signal agent to stop so run_turn().await completes and cmd_rx can process the exit.
                if let Some(ref flag) = self.side_effects.cancel_flag {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                self.quit = true;
            }
            Msg::Cmd(cmd) => {
                // Immediately signal the agent loop to cancel (bypasses blocked cmd_rx).
                if matches!(cmd, TuiCmd::CancelTurn) {
                    if let Some(ref flag) = self.side_effects.cancel_flag {
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                let _ = self.cmd_tx.send(Msg::Cmd(cmd));
            }
            Msg::Redraw => {
                self.redraw = true;
            }
            _ => {}
        }
    }

    // ── Component message processing ─────────────────────────────

    fn process_component_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Cmd(cmd) => {
                let _ = self.cmd_tx.send(Msg::Cmd(cmd));
            }
            Msg::QuestionSubmit(raw) => {
                let t = raw.trim();
                // /auto-answer shortcut
                if t == "/auto-answer" {
                    self.send_question_answer(QuestionSelection::Suggested);
                    return;
                }
                // Try to parse against the active question payload
                let payload = self
                    .side_effects
                    .active_question_payload
                    .lock()
                    .ok()
                    .and_then(|g| g.clone());
                if let Some(ref q) = payload {
                    if let Some(sel) = Self::parse_question_answer(&raw, q) {
                        self.send_question_answer(sel);
                        return;
                    }
                }
                // Fallback: send as regular command (e.g. slash command)
                let _ = self.cmd_tx.send(Msg::Cmd(TuiCmd::Submit(raw)));
            }
            Msg::QuestionAnswer(sel) => {
                self.send_question_answer(sel);
            }
            Msg::ApprovalSubmit(raw) => {
                let t = raw.trim();
                let approved = t.eq_ignore_ascii_case("y") || t.eq_ignore_ascii_case("yes");
                let denied = t.eq_ignore_ascii_case("n") || t.eq_ignore_ascii_case("no");
                if approved || denied {
                    self.send_approval_answer(approved);
                }
                // If input doesn't match y/yes/n/no, silently ignore (don't submit as command).
            }
            Msg::ApprovalQuickAnswer {
                approved,
                always_allow,
            } => {
                if always_allow {
                    self.send_approval_always_allow();
                } else {
                    self.send_approval_answer(approved);
                }
            }
            _ => {}
        }
    }

    /// Parse raw user input into a `QuestionSelection` based on the active question.
    fn parse_question_answer(
        raw: &str,
        q: &InteractiveQuestionPayload,
    ) -> Option<QuestionSelection> {
        let t = raw.trim();
        if t.is_empty() || t == "0" || t.eq_ignore_ascii_case("s") {
            return Some(QuestionSelection::Suggested);
        }
        if let Ok(n) = t.parse::<usize>() {
            if n >= 1 && n <= q.options.len() {
                return Some(QuestionSelection::Option {
                    option_id: q.options[n - 1].id.clone(),
                });
            }
        }
        if q.allow_custom && !t.is_empty() {
            return Some(QuestionSelection::Custom {
                text: t.to_string(),
            });
        }
        None
    }

    /// Send a question answer through the side channel.
    fn send_question_answer(&mut self, sel: QuestionSelection) {
        if let Some(ref tx) = self.side_effects.question_answer_tx
            && let Some(ref q) = self.components.transcript.active_question_id()
        {
            let _ = tx.send((q.clone(), sel));
        }
    }

    /// Send an approval answer through the side channel.
    fn send_approval_answer(&mut self, approved: bool) {
        if let Some(ref tx) = self.side_effects.approval_answer_tx
            && let Ok(guard) = self.side_effects.active_approval_payload.lock()
            && let Some(ref req) = *guard
        {
            let _ = tx.send(crate::tui::app::ApprovalAnswer::Verdict {
                call_id: req.call_id.clone(),
                approved,
            });
        }
    }

    /// Send an "always allow" approval — generates a wildcard pattern from the tool name and input.
    fn send_approval_always_allow(&mut self) {
        if let Some(ref tx) = self.side_effects.approval_answer_tx
            && let Ok(guard) = self.side_effects.active_approval_payload.lock()
            && let Some(ref req) = *guard
        {
            let pattern = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&req.input) {
                nca_core::approval::suggest_allow_pattern(&req.tool, &json)
            } else {
                format!("{}:*", req.tool)
            };
            let _ = tx.send(crate::tui::app::ApprovalAnswer::AllowPattern {
                call_id: req.call_id.clone(),
                pattern,
            });
        }
    }

    // ── View ─────────────────────────────────────────────────────

    /// Render the full TUI layout.
    pub(crate) fn view(&mut self, terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) {
        let _ = terminal.draw(|frame| {
            let size = frame.area();
            self.size = (size.width, size.height);
            let areas = self.compute_layout(size);

            // Render status bar
            self.components.status_bar.render(frame, areas.status);

            // Transcript: real rendering
            let transcript_para = self.components.transcript.render(areas.transcript);
            frame.render_widget(transcript_para, areas.transcript);

            // Composer: real rendering
            self.components.composer.render(frame, areas.composer);

            // ── Popup overlays (rendered on top of everything) ──────
            if let Some(ref p) = self.components.command_palette {
                p.render(frame);
            }
            if let Some(ref mut p) = self.components.model_picker {
                p.render(frame);
            }
            if let Some(ref p) = self.components.branch_picker {
                p.render(frame);
            }
            if let Some(ref p) = self.components.provider_picker {
                p.render(frame);
            }
            if let Some(ref p) = self.components.agent_picker {
                p.render(frame);
            }
            if let Some(ref p) = self.components.permission_picker {
                p.render(frame);
            }
            if let Some(ref mut p) = self.components.session_picker {
                p.render(frame);
            }
            if let Some(ref p) = self.components.connect_modal {
                p.render(frame);
            }
            if let Some(ref p) = self.components.api_key_modal {
                p.render(frame);
            }
            if let Some(ref mut p) = self.components.info_modal {
                p.render(frame);
            }
        });
        self.redraw = false;
    }

    // ── Layout ───────────────────────────────────────────────────

    /// Compute the main layout areas for the three core components.
    fn compute_layout(&self, size: Rect) -> LayoutAreas {
        // Dynamic composer height based on content
        let composer_cols = size.width.saturating_sub(2) as usize;
        let input_rows = self
            .components
            .composer
            .state()
            .content_rows(composer_cols.max(1)) as u16;
        let chrome_h = self.components.state().chrome_height();
        let composer_h = input_rows.saturating_add(1).saturating_add(2); // rows + hint + border
        let total_composer_h = composer_h.saturating_add(chrome_h);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                // status bar
                Constraint::Min(4),                   // transcript
                Constraint::Length(total_composer_h), // composer (dynamic)
            ])
            .split(size);

        LayoutAreas {
            status: chunks[0],
            transcript: chunks[1],
            composer: chunks[2],
        }
    }
}

struct LayoutAreas {
    status: Rect,
    transcript: Rect,
    composer: Rect,
}

// ── Helper to access composer state from Components ──────────────

impl Components {
    /// Convenience accessor for composer state.
    fn state(&self) -> &super::components::composer::ComposerState {
        self.composer.state()
    }

    /// Convenience accessor for mutable composer state.
    fn state_mut(&mut self) -> &mut super::components::composer::ComposerState {
        self.composer.state_mut()
    }
}
