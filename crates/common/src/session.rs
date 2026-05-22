use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;

use crate::message::Message;

/// Metadata for a persisted session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Unique session identifier.
    pub id: String,
    /// When the session was first created.
    pub created_at: DateTime<Utc>,
    /// When the session snapshot was last updated.
    pub updated_at: DateTime<Utc>,
    /// Workspace root directory for this session.
    pub workspace: PathBuf,
    /// Active LLM model name.
    pub model: String,
    /// Current lifecycle status.
    pub status: SessionStatus,
    /// OS process id of the runtime supervisor, when running.
    pub pid: Option<u32>,
    /// Unix socket path for IPC attach, when running.
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

/// Current on-disk schema version for [`SessionState`].
pub const SESSION_STATE_SCHEMA_VERSION: u32 = 1;

fn default_session_state_schema_version() -> u32 {
    SESSION_STATE_SCHEMA_VERSION
}

/// Full session state, including conversation history and cost tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// On-disk schema version for forward-compatible loading.
    #[serde(default = "default_session_state_schema_version")]
    pub schema_version: u32,
    /// Session metadata and lineage fields.
    pub meta: SessionMeta,
    /// Full conversation history for resume.
    pub messages: Vec<Message>,
    /// Cumulative input tokens across all turns.
    pub total_input_tokens: u64,
    /// Cumulative output tokens across all turns.
    pub total_output_tokens: u64,
    /// Estimated USD cost at time of last persist.
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
    #[must_use]
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
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let mut metadata = BTreeMap::new();
        for (key, value) in env::vars() {
            if let Some(meta_key) = key.strip_prefix("NCA_ORCH_META_")
                && !value.trim().is_empty()
            {
                metadata.insert(meta_key.to_ascii_lowercase(), value);
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

    #[must_use]
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

/// Lifecycle status of a persisted session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Supervisor is active or session was interrupted mid-run.
    Running,
    /// Session finished normally.
    Completed,
    /// Session ended due to an unrecoverable error.
    Error,
    /// Session was cancelled by the user or orchestrator.
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::{OrchestrationContext, SessionState};
    use std::env;

    #[test]
    fn session_state_v0_fixture_without_schema_version_defaults_to_one() {
        let raw = r#"{
            "meta": {
                "id": "sess-v0",
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "workspace": "/tmp/ws",
                "model": "MiniMax-M2.5",
                "status": "running"
            },
            "messages": [],
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "estimated_cost_usd": 0.0
        }"#;
        let state: SessionState = serde_json::from_str(raw).expect("deserialize v0 session");
        assert_eq!(state.schema_version, 1);
        assert_eq!(state.meta.id, "sess-v0");
    }

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
        assert_eq!(
            ctx.metadata.get("channel").map(String::as_str),
            Some("ticket")
        );

        for (key, _) in vars {
            unsafe { env::remove_var(key) };
        }
    }
}
