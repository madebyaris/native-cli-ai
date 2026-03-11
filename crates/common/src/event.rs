use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::tool::ToolResult;

/// Envelope for events written to disk, with stable id and timestamp for ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub ts: Option<DateTime<Utc>>,
    pub event: AgentEvent,
}

/// Events emitted by the agent runtime, broadcast over IPC to CLI and monitor.
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
    },
    CostUpdated {
        input_tokens: u64,
        output_tokens: u64,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EndReason {
    UserExit,
    Completed,
    Error,
    Cancelled,
}

/// Commands sent from CLI or monitor to the agent runtime.
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
    Cancel,
    Shutdown,
    /// Desktop: request the runtime to start a new session in a workspace.
    StartSession {
        workspace: PathBuf,
        prompt: String,
        model: Option<String>,
        safe_mode: bool,
    },
    /// Desktop: resume an existing session with an optional follow-up prompt.
    ResumeSession {
        session_id: String,
        prompt: Option<String>,
    },
    /// Desktop: query current session state snapshot.
    QueryState {
        session_id: String,
    },
    /// Desktop: list all sessions in a workspace.
    ListSessions {
        workspace: PathBuf,
    },
}

/// Responses to query commands, sent back over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentResponse {
    SessionState {
        session: crate::session::SessionState,
    },
    SessionList {
        sessions: Vec<crate::session::SessionMeta>,
    },
    Error {
        message: String,
    },
    Ok,
}
