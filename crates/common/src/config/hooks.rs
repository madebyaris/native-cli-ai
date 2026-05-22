//! Hook (lifecycle command) configuration.

use serde::{Deserialize, Serialize};

/// Lifecycle hook command lists keyed by event kind.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookConfig {
    /// Commands run when a session starts.
    #[serde(default)]
    pub session_start: Vec<HookCommand>,
    /// Commands run when a session ends.
    #[serde(default)]
    pub session_end: Vec<HookCommand>,
    /// Commands run before a tool executes.
    #[serde(default)]
    pub pre_tool_use: Vec<HookCommand>,
    /// Commands run after a successful tool execution.
    #[serde(default)]
    pub post_tool_use: Vec<HookCommand>,
    /// Commands run after a tool failure.
    #[serde(default)]
    pub post_tool_failure: Vec<HookCommand>,
    /// Commands run when user approval is requested.
    #[serde(default)]
    pub approval_requested: Vec<HookCommand>,
    /// Commands run when a sub-agent session starts.
    #[serde(default)]
    pub subagent_start: Vec<HookCommand>,
    /// Commands run when a sub-agent session stops.
    #[serde(default)]
    pub subagent_stop: Vec<HookCommand>,
}

/// A shell command invoked by the hook runner for a lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCommand {
    /// Shell command to execute.
    pub command: String,
    /// Optional tool-name matcher; when set, only matching tools trigger this hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// When true, the agent waits for this hook to finish before continuing.
    #[serde(default)]
    pub blocking: bool,
}

impl HookConfig {
    pub(super) fn merge(&mut self, partial: PartialHookConfig) {
        if let Some(session_start) = partial.session_start {
            self.session_start = session_start;
        }
        if let Some(session_end) = partial.session_end {
            self.session_end = session_end;
        }
        if let Some(pre_tool_use) = partial.pre_tool_use {
            self.pre_tool_use = pre_tool_use;
        }
        if let Some(post_tool_use) = partial.post_tool_use {
            self.post_tool_use = post_tool_use;
        }
        if let Some(post_tool_failure) = partial.post_tool_failure {
            self.post_tool_failure = post_tool_failure;
        }
        if let Some(approval_requested) = partial.approval_requested {
            self.approval_requested = approval_requested;
        }
        if let Some(subagent_start) = partial.subagent_start {
            self.subagent_start = subagent_start;
        }
        if let Some(subagent_stop) = partial.subagent_stop {
            self.subagent_stop = subagent_stop;
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialHookConfig {
    pub(super) session_start: Option<Vec<HookCommand>>,
    pub(super) session_end: Option<Vec<HookCommand>>,
    pub(super) pre_tool_use: Option<Vec<HookCommand>>,
    pub(super) post_tool_use: Option<Vec<HookCommand>>,
    pub(super) post_tool_failure: Option<Vec<HookCommand>>,
    pub(super) approval_requested: Option<Vec<HookCommand>>,
    pub(super) subagent_start: Option<Vec<HookCommand>>,
    pub(super) subagent_stop: Option<Vec<HookCommand>>,
}
