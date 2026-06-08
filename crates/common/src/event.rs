use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::tool::ToolResult;

/// Real-time busy state indicator for CLI rendering.
/// Reflects the current processing state of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusyState {
    /// Agent is idle and ready for input.
    Idle,
    /// Agent is thinking/planning (heavy computation, model reasoning).
    Thinking,
    /// Agent is streaming tokens from LLM.
    Streaming,
    /// Agent is executing a tool.
    ToolRunning,
    /// Agent is waiting for user approval on a tool call.
    ApprovalPending,
    /// Error or blocked state.
    Error,
}

impl BusyState {
    /// Human-readable label for this state.
    pub fn label(&self) -> &'static str {
        match self {
            BusyState::Idle => "idle",
            BusyState::Thinking => "thinking",
            BusyState::Streaming => "streaming",
            BusyState::ToolRunning => "tool",
            BusyState::ApprovalPending => "approval",
            BusyState::Error => "error",
        }
    }
}

/// Envelope for events written to disk, with stable id and timestamp for ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub ts: Option<DateTime<Utc>>,
    pub event: AgentEvent,
}

impl EventEnvelope {
    pub fn new(id: u64, event: AgentEvent) -> Self {
        Self {
            id,
            ts: Some(Utc::now()),
            event,
        }
    }
}

/// Events emitted by the agent runtime, broadcast over IPC to the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentEvent {
    SessionStarted {
        session_id: String,
        workspace: PathBuf,
        model: String,
    },
    MessageReceived {
        role: String,
        content: String,
    },
    TokensStreamed {
        delta: String,
    },
    ToolCallStarted {
        call_id: String,
        tool: String,
        input: serde_json::Value,
    },
    ToolCallCompleted {
        call_id: String,
        output: ToolResult,
    },
    ApprovalRequested {
        call_id: String,
        tool: String,
        description: String,
    },
    ApprovalResolved {
        call_id: String,
        approved: bool,
        /// Pattern being persisted when user chose "always allow".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_pattern: Option<String>,
    },
    CostUpdated {
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default)]
        cache_read_tokens: u64,
        estimated_cost_usd: f64,
    },
    Checkpoint {
        phase: String,
        detail: String,
        turn: u32,
    },
    SessionEnded {
        reason: EndReason,
    },
    Error {
        message: String,
    },
    Response {
        response: AgentResponse,
    },
    ChildSessionSpawned {
        parent_session_id: String,
        child_session_id: String,
        task: String,
        workspace: PathBuf,
        branch: Option<String>,
    },
    ChildSessionCompleted {
        parent_session_id: String,
        child_session_id: String,
        status: String,
    },
    /// Live activity from a child session (tools, checkpoints, nested spawns), for parent UI.
    ChildSessionActivity {
        child_session_id: String,
        /// Short label, e.g. tool name or checkpoint phase.
        phase: String,
        /// One-line detail for the sidebar/transcript.
        detail: String,
    },
    /// User must pick an option, use the suggested answer, or enter custom text.
    /// Emitted when the `ask_question` tool runs; answer via `AgentCommand::AnswerQuestion` or local CLI.
    QuestionRequested {
        question: InteractiveQuestionPayload,
    },
    /// Logged after the user (or orchestrator) answers a question.
    QuestionResolved {
        question_id: String,
        selection: QuestionSelection,
    },
    /// Live context usage statistics for the UI (token count + percentage).
    ContextStatsUpdated {
        estimated_tokens: usize,
        context_window: usize,
        usage_percent: u8,
    },
    /// Warning that context is approaching limit.
    ContextWarning {
        message: String,
    },
    /// Context compaction/summarization event.
    ContextCompaction {
        phase: String,
        message: String,
    },
    /// Busy state transition (for animated indicator rendering).
    BusyStateChanged {
        state: BusyState,
    },
}

/// One selectable row shown for an interactive question.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
}

/// Full question payload broadcast on the event bus and shown in the CLI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InteractiveQuestionPayload {
    pub question_id: String,
    pub call_id: String,
    pub prompt: String,
    pub options: Vec<QuestionOption>,
    #[serde(default = "default_allow_custom")]
    pub allow_custom: bool,
    /// Model-provided default; user can accept via suggested / `/auto-answer`.
    pub suggested_answer: String,
}

fn default_allow_custom() -> bool {
    true
}

/// How the user answered an interactive question.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionSelection {
    Option { option_id: String },
    Custom { text: String },
    Suggested,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EndReason {
    UserExit,
    Completed,
    Error,
    Cancelled,
}

/// Commands sent over IPC to the agent runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentCommand {
    SendMessage {
        content: String,
    },
    ApproveToolCall {
        call_id: String,
    },
    DenyToolCall {
        call_id: String,
    },
    /// Submit an answer for the pending `ask_question` with matching `question_id`.
    AnswerQuestion {
        question_id: String,
        selection: QuestionSelection,
    },
    Cancel,
    Shutdown,
}

/// Responses to query-style IPC messages, sent back as `AgentEvent::Response`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentResponse {
    SessionState {
        session: Box<crate::session::SessionState>,
    },
    SessionList {
        sessions: Vec<crate::session::SessionMeta>,
    },
    Error {
        message: String,
    },
    Ok,
}

#[cfg(test)]
mod interactive_question_serde_tests {
    use super::*;

    #[test]
    fn question_requested_roundtrip() {
        let q = InteractiveQuestionPayload {
            question_id: "q-1".into(),
            call_id: "call_1".into(),
            prompt: "Pick one".into(),
            options: vec![
                QuestionOption {
                    id: "a".into(),
                    label: "Alpha".into(),
                },
                QuestionOption {
                    id: "b".into(),
                    label: "Beta".into(),
                },
            ],
            allow_custom: true,
            suggested_answer: "Alpha".into(),
        };
        let ev = AgentEvent::QuestionRequested {
            question: q.clone(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        match back {
            AgentEvent::QuestionRequested { question } => assert_eq!(question, q),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn answer_question_command_roundtrip() {
        let cmd = AgentCommand::AnswerQuestion {
            question_id: "q-1".into(),
            selection: QuestionSelection::Suggested,
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        let back: AgentCommand = serde_json::from_str(&json).expect("deserialize");
        match back {
            AgentCommand::AnswerQuestion {
                question_id,
                selection,
            } => {
                assert_eq!(question_id, "q-1");
                assert!(matches!(selection, QuestionSelection::Suggested));
            }
            _ => panic!("wrong variant"),
        }
    }
}
