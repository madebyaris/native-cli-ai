use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::message::Message;

/// Metadata for a persisted session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub workspace: PathBuf,
    pub model: String,
    pub status: SessionStatus,
    pub pid: Option<u32>,
    pub socket_path: Option<PathBuf>,
    /// Git worktree path if the session runs in an isolated worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<PathBuf>,
    /// Branch name the session operates on (e.g. `nca/<session-id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Base branch the worktree was created from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// Parent session id if this is a child/sub-agent session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// IDs of child sessions spawned from this session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_session_ids: Vec<String>,
    /// Summary inherited from parent session for context continuity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_summary: Option<String>,
    /// Why this child session was spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_reason: Option<String>,
}

/// Full session state, including conversation history and cost tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub meta: SessionMeta,
    pub messages: Vec<Message>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub estimated_cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Completed,
    Error,
    Cancelled,
}
