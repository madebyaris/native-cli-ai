//! Monitor app state, SessionVm, and derived indexes for UI rendering.

use nca_common::event::EndReason;
use nca_common::tool::ToolResult;
use std::collections::HashMap;

/// A single entry in the session timeline for display.
#[derive(Debug, Clone)]
pub enum TimelineEntry {
    SessionStarted {
        session_id: String,
        model: String,
        workspace: String,
    },
    Message {
        role: String,
        content: String,
    },
    Tokens {
        delta: String,
    },
    ToolStart {
        call_id: String,
        tool: String,
        input: serde_json::Value,
    },
    ToolComplete {
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
    Cost {
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
    ChildSpawned {
        child_session_id: String,
        task: String,
    },
    ChildCompleted {
        child_session_id: String,
        status: String,
    },
}

/// State of a tool call for the inspector.
#[derive(Debug, Clone)]
pub enum ToolCallState {
    Started {
        tool: String,
        input: serde_json::Value,
        output: Option<ToolResult>,
    },
    Completed {
        output: ToolResult,
    },
}

/// Pending approval awaiting user action.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub call_id: String,
    pub tool: String,
    pub description: String,
}

/// Reduced view model for a single session, built from replayed and live events.
#[derive(Debug, Default)]
pub struct SessionVm {
    pub timeline: Vec<TimelineEntry>,
    pub pending_approvals: Vec<PendingApproval>,
    pub tool_calls: HashMap<String, ToolCallState>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
    pub errors: Vec<String>,
    pub end_reason: Option<EndReason>,
    pub model: Option<String>,
}

impl SessionVm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a reduced event update. Call resolve_approval separately when needed.
    pub fn apply(
        &mut self,
        timeline_entry: Option<TimelineEntry>,
        pending_approval: Option<(String, String, String)>,
        tool_update: Option<(String, ToolCallState)>,
        cost_update: Option<(u64, u64, f64)>,
        error: Option<String>,
        session_ended: Option<EndReason>,
    ) {
        if let Some(e) = timeline_entry {
            self.timeline.push(e);
        }
        if let Some((call_id, tool, description)) = pending_approval {
            self.pending_approvals.push(PendingApproval {
                call_id,
                tool,
                description,
            });
        }
        if let Some((call_id, state)) = tool_update {
            self.tool_calls.insert(call_id, state);
        }
        if let Some((i, o, c)) = cost_update {
            self.input_tokens = i;
            self.output_tokens = o;
            self.estimated_cost_usd = c;
        }
        if let Some(e) = error {
            self.errors.push(e);
        }
        if let Some(r) = session_ended {
            self.end_reason = Some(r);
        }
    }

    pub fn has_pending_approvals(&self) -> bool {
        !self.pending_approvals.is_empty()
    }

    pub fn resolve_approval(&mut self, call_id: &str) {
        self.pending_approvals
            .retain(|a| a.call_id != call_id);
    }
}

/// A task card representing an agent run in the multi-run dashboard.
#[derive(Debug, Clone)]
pub struct TaskCard {
    pub session_id: String,
    pub workspace_name: String,
    pub workspace_path: std::path::PathBuf,
    pub model: String,
    pub status: TaskStatus,
    pub branch: Option<String>,
    pub pending_approvals: usize,
    pub error_count: usize,
    pub updated_at: String,
    pub parent_session_id: Option<String>,
    pub child_session_ids: Vec<String>,
    pub spawn_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    WaitingApproval,
    Completed,
    Error,
    Cancelled,
    Queued,
}

impl TaskStatus {
    pub fn label(&self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::WaitingApproval => "waiting",
            TaskStatus::Completed => "done",
            TaskStatus::Error => "error",
            TaskStatus::Cancelled => "cancelled",
            TaskStatus::Queued => "queued",
        }
    }
}

/// Top-level app state: session index, selected session, and its view model.
#[derive(Debug)]
pub struct AppState {
    pub selected_session_id: Option<String>,
    pub loaded_session_id: Option<String>,
    pub session_vm: Option<SessionVm>,
    pub filter_timeline: Option<String>,
    pub show_tools: bool,
    pub show_stats: bool,
    pub show_log: bool,
    pub task_cards: Vec<TaskCard>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            selected_session_id: None,
            loaded_session_id: None,
            session_vm: None,
            filter_timeline: None,
            show_tools: true,
            show_stats: true,
            show_log: true,
            task_cards: Vec::new(),
        }
    }

    pub fn select_session(&mut self, id: Option<String>) {
        self.selected_session_id = id;
        self.loaded_session_id = None;
        self.session_vm = None;
    }

    pub fn set_session_vm(&mut self, vm: SessionVm) {
        self.session_vm = Some(vm);
    }

    pub fn clear_session_vm(&mut self) {
        self.loaded_session_id = None;
        self.session_vm = None;
    }
}
