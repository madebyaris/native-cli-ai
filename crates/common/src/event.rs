use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::tool::ToolResult;

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
    SendMessage { content: String },
    ApproveToolCall { call_id: String },
    DenyToolCall { call_id: String },
    Cancel,
    Shutdown,
}
