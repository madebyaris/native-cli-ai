use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
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
    /// Persisted compact summary for resume and memory surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_summary: Option<String>,
    /// External orchestration metadata for headless worker runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration: Option<OrchestrationContext>,
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

/// Lightweight session summary for machine-readable orchestration surfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub workspace: PathBuf,
    pub model: String,
    pub status: SessionStatus,
    pub pid: Option<u32>,
    pub socket_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_session_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration: Option<OrchestrationContext>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub estimated_cost_usd: f64,
}

/// Optional metadata injected by an external orchestrator for headless runs.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct OrchestrationContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl SessionState {
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            id: self.meta.id.clone(),
            created_at: self.meta.created_at,
            updated_at: self.meta.updated_at,
            workspace: self.meta.workspace.clone(),
            model: self.meta.model.clone(),
            status: self.meta.status.clone(),
            pid: self.meta.pid,
            socket_path: self.meta.socket_path.clone(),
            worktree_path: self.meta.worktree_path.clone(),
            branch: self.meta.branch.clone(),
            base_branch: self.meta.base_branch.clone(),
            parent_session_id: self.meta.parent_session_id.clone(),
            child_session_ids: self.meta.child_session_ids.clone(),
            inherited_summary: self.meta.inherited_summary.clone(),
            spawn_reason: self.meta.spawn_reason.clone(),
            session_summary: self.meta.session_summary.clone(),
            orchestration: self.meta.orchestration.clone(),
            total_input_tokens: self.total_input_tokens,
            total_output_tokens: self.total_output_tokens,
            estimated_cost_usd: self.estimated_cost_usd,
        }
    }
}

impl OrchestrationContext {
    pub fn from_env() -> Option<Self> {
        let mut metadata = BTreeMap::new();
        for (key, value) in env::vars() {
            if let Some(meta_key) = key.strip_prefix("NCA_ORCH_META_") {
                if !value.trim().is_empty() {
                    metadata.insert(meta_key.to_ascii_lowercase(), value);
                }
            }
        }

        let ctx = Self {
            orchestrator: non_empty_env("NCA_ORCH_NAME"),
            run_id: non_empty_env("NCA_ORCH_RUN_ID"),
            task_id: non_empty_env("NCA_ORCH_TASK_ID"),
            task_ref: non_empty_env("NCA_ORCH_TASK_REF"),
            parent_run_id: non_empty_env("NCA_ORCH_PARENT_RUN_ID"),
            callback_url: non_empty_env("NCA_ORCH_CALLBACK_URL"),
            metadata,
        };

        if ctx.is_empty() { None } else { Some(ctx) }
    }

    pub fn is_empty(&self) -> bool {
        self.orchestrator.is_none()
            && self.run_id.is_none()
            && self.task_id.is_none()
            && self.task_ref.is_none()
            && self.parent_run_id.is_none()
            && self.callback_url.is_none()
            && self.metadata.is_empty()
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Completed,
    Error,
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::OrchestrationContext;
    use std::env;

    #[test]
    fn orchestration_context_reads_env_contract() {
        let vars = [
            ("NCA_ORCH_NAME", "paperclip-wrapper"),
            ("NCA_ORCH_RUN_ID", "run-123"),
            ("NCA_ORCH_TASK_ID", "task-456"),
            ("NCA_ORCH_TASK_REF", "issue/99"),
            ("NCA_ORCH_PARENT_RUN_ID", "run-122"),
            ("NCA_ORCH_CALLBACK_URL", "http://localhost/callback"),
            ("NCA_ORCH_META_CHANNEL", "ticket"),
        ];

        for (key, value) in vars {
            unsafe { env::set_var(key, value) };
        }

        let ctx = OrchestrationContext::from_env().expect("context from env");
        assert_eq!(ctx.orchestrator.as_deref(), Some("paperclip-wrapper"));
        assert_eq!(ctx.run_id.as_deref(), Some("run-123"));
        assert_eq!(ctx.task_id.as_deref(), Some("task-456"));
        assert_eq!(ctx.task_ref.as_deref(), Some("issue/99"));
        assert_eq!(ctx.parent_run_id.as_deref(), Some("run-122"));
        assert_eq!(
            ctx.callback_url.as_deref(),
            Some("http://localhost/callback")
        );
        assert_eq!(ctx.metadata.get("channel").map(String::as_str), Some("ticket"));

        for (key, _) in vars {
            unsafe { env::remove_var(key) };
        }
    }
}
