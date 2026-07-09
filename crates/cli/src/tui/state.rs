//! Shared types used by TUI components.

use nca_common::config::ProviderKind;
use nca_common::event::InteractiveQuestionPayload;

#[derive(Debug, Clone)]
pub enum DisplayBlock {
    User(String),
    Assistant(String),
    /// Collapsible reasoning/thinking content from the model.
    Thinking {
        content: String,
        /// Whether this thinking block is expanded (true = show full content).
        expanded: bool,
        /// Elapsed wall-clock time for this thinking block (UI-layer timer).
        duration_ms: Option<u64>,
    },
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
        input: String,
        #[allow(dead_code)] // not yet consumed by rendering; kept for future use
        ok: bool,
        detail: String,
        /// Full output content (before truncation) for collapsible display.
        full_output: String,
        /// Whether this tool block is expanded (true = show full output).
        expanded: bool,
        /// Wall-clock duration of the tool call (from ToolCallCompleted event).
        duration_ms: u64,
    },
    /// Interactive `ask_question` prompt (options + suggested answer).
    Question(InteractiveQuestionPayload),
    System(String),
    ErrorLine(String),
    /// One-line turn-completion summary showing total turn duration.
    TurnInfo {
        duration_ms: u64,
    },
}

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub call_id: String,
    pub tool: String,
    pub description: String,
    pub input: String,
}

/// Status of an API key validation during onboarding.
#[derive(Debug, Clone)]
pub enum OnboardingValidation {
    Validating,
    Valid,
    Failed(String),
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
