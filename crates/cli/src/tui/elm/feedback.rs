//! TuiFeedback trait: decouples repl.rs from TUI state.
//!
//! Write operations go through `TuiFeedbackMsg` → channel → `NcaModel::update()`.
//! Read operations use oneshot channels for synchronous responses.

use nca_common::config::ProviderKind;
use nca_common::event::{AgentEvent, BusyState, InteractiveQuestionPayload};
use nca_common::message::ImageAttachment;
use nca_common::session::SessionSnapshot;

use std::sync::Arc;

use crate::tui::state::{ApprovalRequest, DisplayBlock, ModelPickerEntry, OnboardingValidation};

/// Messages sent TO the TUI from the runtime (write operations).
///
/// These are produced by `TuiFeedbackChannel` and consumed by `NcaModel::update()`.
#[derive(Debug)]
pub enum TuiFeedbackMsg {
    /// Full agent event forwarding to NcaModel (from bridge).
    Agent(AgentEvent),
    PushSystem(String),
    PushError(String),
    PushBlocks(Vec<DisplayBlock>),
    OpenBranchPicker {
        branches: Vec<String>,
        current: String,
    },
    SetCurrentBranch(String),
    OpenApiKeyModal {
        provider: ProviderKind,
        has_key: bool,
        connect_after: bool,
    },
    SetModel(String),
    SetPermissionMode(String),
    SetAgentProfile(String),
    SetStreamingAssistant(Option<String>),
    SetStreamingReasoning(Option<String>),
    SetBusyState(BusyState),
    SetActiveApproval(Option<ApprovalRequest>),
    SetActiveQuestion(Option<InteractiveQuestionPayload>),
    ClearTranscript,
    ShouldExit,

    // ── Phase 3b-1 additions ──
    /// Open the info/help modal with title and scrollable lines.
    OpenInfoModal {
        title: String,
        lines: Vec<String>,
    },
    /// Open the model picker popup.
    OpenModelPicker {
        entries: Vec<ModelPickerEntry>,
    },
    /// Open the agent profile picker popup, pre-selecting `current_index`.
    OpenAgentPicker {
        labels: Vec<(String, String)>,
        current_index: usize,
    },
    /// Open the permission mode picker popup, pre-selecting `current_index`.
    OpenPermissionPicker {
        current_index: usize,
    },
    /// Open the provider picker popup.
    OpenProviderPicker {
        for_api_key: bool,
    },
    /// Open the session picker popup.
    OpenSessionPicker {
        entries: Vec<SessionSnapshot>,
        current: String,
    },
    /// Open the connect-provider modal.
    OpenConnectModal,
    /// Close the API key modal (used after key is saved).
    CloseApiKeyModal,
    /// Close the connect-provider modal.
    CloseConnectModal,
    /// Set simple busy flag (true = show spinner in status bar).
    SetBusy(bool),
    /// Reset transcript, session id, model, and token counters (used by /new and session switch).
    ResetSessionState {
        session_id: String,
        model: String,
    },
    /// Toggle onboarding mode on the TUI.
    SetOnboardingMode(bool),
    /// Set API key validation status (during onboarding).
    SetValidationStatus {
        status: Option<OnboardingValidation>,
    },
    SetPendingApiKeyProvider {
        provider: Option<ProviderKind>,
    },
    /// Stage an image attachment for the next user message.
    PushStagedImage {
        attachment: ImageAttachment,
    },
    /// Clear all staged image attachments.
    ClearStagedImages,
    /// Set the composer input buffer and cursor position (used by /editor).
    SetInputBuffer {
        text: String,
        cursor: usize,
    },
    /// Toggle tool output expand/collapse for a specific block index.
    ToggleToolOutput {
        block_index: usize,
    },
    /// Toggle all tool output blocks (expand if any collapsed, collapse all otherwise).
    ToggleAllToolOutput,
}

/// Trait that decouples repl.rs from TUI internal state.
///
/// The primary implementation is `TuiFeedbackChannel` (channel-based).
pub trait TuiFeedback: Send + Sync {
    // ── Write operations (one-way to TUI) ──
    fn push_agent_event(&self, event: AgentEvent);
    fn push_system(&self, msg: String);
    fn push_error(&self, msg: String);
    fn push_blocks(&self, blocks: Vec<DisplayBlock>);
    fn open_branch_picker(&self, branches: Vec<String>, current: &str);
    fn set_current_branch(&self, branch: &str);
    fn open_api_key_modal(&self, provider: ProviderKind, has_key: bool, connect_after: bool);
    fn set_model(&self, model: String);
    fn set_permission_mode(&self, mode: String);
    fn set_agent_profile(&self, label: String);
    fn set_streaming_assistant(&self, text: Option<String>);
    fn set_streaming_reasoning(&self, text: Option<String>);
    fn set_busy_state(&self, state: BusyState);
    fn set_active_approval(&self, req: Option<ApprovalRequest>);
    fn set_active_question(&self, q: Option<InteractiveQuestionPayload>);
    fn clear_transcript(&self);
    fn should_exit(&self);
    // ── Phase 3b-1 additions ──
    fn open_info_modal(&self, title: String, lines: Vec<String>);
    fn open_model_picker(&self, entries: Vec<ModelPickerEntry>);
    fn open_agent_picker(&self, labels: Vec<(String, String)>, current_index: usize);
    fn open_permission_picker(&self, current_index: usize);
    fn open_provider_picker(&self, for_api_key: bool);
    fn open_session_picker(&self, entries: Vec<SessionSnapshot>, current: String);
    fn open_connect_modal(&self);
    fn close_api_key_modal(&self);
    fn close_connect_modal(&self);
    fn set_busy(&self, busy: bool);
    fn reset_session_state(&self, session_id: String, model: String);
    fn set_onboarding_mode(&self, onboarding: bool);
    fn set_validation_status(&self, status: Option<OnboardingValidation>);
    fn set_pending_api_key_provider(&self, provider: Option<ProviderKind>);
    fn push_staged_image(&self, attachment: ImageAttachment);
    fn clear_staged_images(&self);
    fn set_input_buffer(&self, text: String, cursor: usize);
    // ── Read operations (needed by cmd_rx loop for conditional logic) ──
    fn get_active_question_id(&self) -> Option<String>;
    fn get_active_question_payload(&self) -> Option<InteractiveQuestionPayload>;
    fn take_staged_images(&self) -> Vec<ImageAttachment>;
    // ── Phase 3b-2 additions ──
    fn toggle_tool_output(&self, block_index: usize);
    fn toggle_all_tool_output(&self);
}

/// Channel-based implementation of TuiFeedback.
///
/// Each write method sends a `TuiFeedbackMsg` to the Elm update loop.
/// Read methods use shared `Arc<Mutex<...>>` state for synchronous responses.
pub struct TuiFeedbackChannel {
    tx: tokio::sync::mpsc::UnboundedSender<TuiFeedbackMsg>,
    /// Shared active question ID for synchronous reads (set by NcaModel).
    active_question_id: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// Shared active question payload for synchronous reads (set by NcaModel).
    active_question_payload: std::sync::Arc<std::sync::Mutex<Option<InteractiveQuestionPayload>>>,
    /// Shared staged images for synchronous reads (set by NcaModel).
    staged_images: std::sync::Arc<std::sync::Mutex<Vec<ImageAttachment>>>,
}

impl TuiFeedbackChannel {
    pub(crate) fn new(tx: tokio::sync::mpsc::UnboundedSender<TuiFeedbackMsg>) -> Self {
        Self {
            tx,
            active_question_id: std::sync::Arc::new(std::sync::Mutex::new(None)),
            active_question_payload: std::sync::Arc::new(std::sync::Mutex::new(None)),
            staged_images: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Clone the shared active question ID handle (for passing to NcaModel).
    pub(crate) fn active_question_id_handle(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<Option<String>>> {
        Arc::clone(&self.active_question_id)
    }

    /// Clone the shared active question payload handle (for passing to NcaModel).
    pub(crate) fn active_question_payload_handle(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<Option<InteractiveQuestionPayload>>> {
        Arc::clone(&self.active_question_payload)
    }

    /// Clone the shared staged images handle (for passing to NcaModel).
    pub(crate) fn staged_images_handle(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<ImageAttachment>>> {
        Arc::clone(&self.staged_images)
    }

    /// Update the shared active question ID (called by NcaModel on SetActiveQuestion).
    pub(crate) fn set_active_question_id(&self, id: Option<String>) {
        if let Ok(mut guard) = self.active_question_id.lock() {
            *guard = id;
        }
    }

    /// Update the shared active question payload (called by NcaModel on SetActiveQuestion).
    pub(crate) fn set_active_question_payload(&self, q: Option<InteractiveQuestionPayload>) {
        if let Ok(mut guard) = self.active_question_payload.lock() {
            *guard = q;
        }
    }

    /// Add a staged image (called by NcaModel on PushStagedImage).
    pub(crate) fn add_staged_image(&self, attachment: ImageAttachment) {
        if let Ok(mut guard) = self.staged_images.lock() {
            guard.push(attachment);
        }
    }

    /// Clear staged images (called by NcaModel on ClearStagedImages).
    pub(crate) fn clear_staged_images_shared(&self) {
        if let Ok(mut guard) = self.staged_images.lock() {
            guard.clear();
        }
    }
}

impl TuiFeedback for TuiFeedbackChannel {
    fn push_agent_event(&self, event: AgentEvent) {
        let _ = self.tx.send(TuiFeedbackMsg::Agent(event));
    }
    fn push_system(&self, msg: String) {
        let _ = self.tx.send(TuiFeedbackMsg::PushSystem(msg));
    }
    fn push_error(&self, msg: String) {
        let _ = self.tx.send(TuiFeedbackMsg::PushError(msg));
    }
    fn push_blocks(&self, blocks: Vec<DisplayBlock>) {
        let _ = self.tx.send(TuiFeedbackMsg::PushBlocks(blocks));
    }
    fn open_branch_picker(&self, branches: Vec<String>, current: &str) {
        let _ = self.tx.send(TuiFeedbackMsg::OpenBranchPicker {
            branches,
            current: current.to_string(),
        });
    }
    fn set_current_branch(&self, branch: &str) {
        let _ = self
            .tx
            .send(TuiFeedbackMsg::SetCurrentBranch(branch.to_string()));
    }
    fn open_api_key_modal(&self, provider: ProviderKind, has_key: bool, connect_after: bool) {
        let _ = self.tx.send(TuiFeedbackMsg::OpenApiKeyModal {
            provider,
            has_key,
            connect_after,
        });
    }
    fn set_model(&self, model: String) {
        let _ = self.tx.send(TuiFeedbackMsg::SetModel(model));
    }
    fn set_permission_mode(&self, mode: String) {
        let _ = self.tx.send(TuiFeedbackMsg::SetPermissionMode(mode));
    }
    fn set_agent_profile(&self, label: String) {
        let _ = self.tx.send(TuiFeedbackMsg::SetAgentProfile(label));
    }
    fn set_streaming_assistant(&self, text: Option<String>) {
        let _ = self.tx.send(TuiFeedbackMsg::SetStreamingAssistant(text));
    }
    fn set_streaming_reasoning(&self, text: Option<String>) {
        let _ = self.tx.send(TuiFeedbackMsg::SetStreamingReasoning(text));
    }
    fn set_busy_state(&self, state: BusyState) {
        let _ = self.tx.send(TuiFeedbackMsg::SetBusyState(state));
    }
    fn set_active_approval(&self, req: Option<ApprovalRequest>) {
        let _ = self.tx.send(TuiFeedbackMsg::SetActiveApproval(req));
    }
    fn set_active_question(&self, q: Option<InteractiveQuestionPayload>) {
        let _ = self.tx.send(TuiFeedbackMsg::SetActiveQuestion(q));
    }
    fn clear_transcript(&self) {
        let _ = self.tx.send(TuiFeedbackMsg::ClearTranscript);
    }
    fn should_exit(&self) {
        let _ = self.tx.send(TuiFeedbackMsg::ShouldExit);
    }
    // ── Phase 3b-1 additions ──
    fn open_info_modal(&self, title: String, lines: Vec<String>) {
        let _ = self.tx.send(TuiFeedbackMsg::OpenInfoModal { title, lines });
    }
    fn open_model_picker(&self, entries: Vec<ModelPickerEntry>) {
        let _ = self.tx.send(TuiFeedbackMsg::OpenModelPicker { entries });
    }
    fn open_agent_picker(&self, labels: Vec<(String, String)>, current_index: usize) {
        let _ = self.tx.send(TuiFeedbackMsg::OpenAgentPicker {
            labels,
            current_index,
        });
    }
    fn open_permission_picker(&self, current_index: usize) {
        let _ = self
            .tx
            .send(TuiFeedbackMsg::OpenPermissionPicker { current_index });
    }
    fn open_provider_picker(&self, for_api_key: bool) {
        let _ = self
            .tx
            .send(TuiFeedbackMsg::OpenProviderPicker { for_api_key });
    }
    fn open_session_picker(&self, entries: Vec<SessionSnapshot>, current: String) {
        let _ = self
            .tx
            .send(TuiFeedbackMsg::OpenSessionPicker { entries, current });
    }
    fn open_connect_modal(&self) {
        let _ = self.tx.send(TuiFeedbackMsg::OpenConnectModal);
    }
    fn close_api_key_modal(&self) {
        let _ = self.tx.send(TuiFeedbackMsg::CloseApiKeyModal);
    }
    fn close_connect_modal(&self) {
        let _ = self.tx.send(TuiFeedbackMsg::CloseConnectModal);
    }
    fn set_busy(&self, busy: bool) {
        let _ = self.tx.send(TuiFeedbackMsg::SetBusy(busy));
    }
    fn reset_session_state(&self, session_id: String, model: String) {
        let _ = self
            .tx
            .send(TuiFeedbackMsg::ResetSessionState { session_id, model });
    }
    fn set_onboarding_mode(&self, onboarding: bool) {
        let _ = self.tx.send(TuiFeedbackMsg::SetOnboardingMode(onboarding));
    }
    fn set_validation_status(&self, status: Option<OnboardingValidation>) {
        let _ = self.tx.send(TuiFeedbackMsg::SetValidationStatus { status });
    }
    fn set_pending_api_key_provider(&self, provider: Option<ProviderKind>) {
        let _ = self
            .tx
            .send(TuiFeedbackMsg::SetPendingApiKeyProvider { provider });
    }
    fn push_staged_image(&self, attachment: ImageAttachment) {
        let _ = self.tx.send(TuiFeedbackMsg::PushStagedImage { attachment });
    }
    fn clear_staged_images(&self) {
        let _ = self.tx.send(TuiFeedbackMsg::ClearStagedImages);
    }
    fn set_input_buffer(&self, text: String, cursor: usize) {
        let _ = self
            .tx
            .send(TuiFeedbackMsg::SetInputBuffer { text, cursor });
    }
    // Read operations — use shared state for synchronous responses
    fn get_active_question_id(&self) -> Option<String> {
        self.active_question_id.lock().ok().and_then(|g| g.clone())
    }
    fn get_active_question_payload(&self) -> Option<InteractiveQuestionPayload> {
        self.active_question_payload
            .lock()
            .ok()
            .and_then(|g| g.clone())
    }
    fn take_staged_images(&self) -> Vec<ImageAttachment> {
        self.staged_images
            .lock()
            .ok()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    }
    fn toggle_tool_output(&self, block_index: usize) {
        let _ = self
            .tx
            .send(TuiFeedbackMsg::ToggleToolOutput { block_index });
    }
    fn toggle_all_tool_output(&self) {
        let _ = self.tx.send(TuiFeedbackMsg::ToggleAllToolOutput);
    }
}
