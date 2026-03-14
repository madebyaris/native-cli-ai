use nca_common::config::{HookCommand, HookConfig};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone)]
pub struct HookRunner {
    config: HookConfig,
}

#[derive(Debug, Clone, Copy)]
pub enum HookEventKind {
    SessionStart,
    SessionEnd,
    PreToolUse,
    PostToolUse,
    PostToolFailure,
    ApprovalRequested,
    SubagentStart,
    SubagentStop,
}

impl HookRunner {
    pub fn new(config: HookConfig) -> Self {
        Self { config }
    }

    pub fn has_any(&self) -> bool {
        !self.config.session_start.is_empty()
            || !self.config.session_end.is_empty()
            || !self.config.pre_tool_use.is_empty()
            || !self.config.post_tool_use.is_empty()
            || !self.config.post_tool_failure.is_empty()
            || !self.config.approval_requested.is_empty()
            || !self.config.subagent_start.is_empty()
            || !self.config.subagent_stop.is_empty()
    }

    pub async fn run(
        &self,
        event: HookEventKind,
        matcher_value: Option<&str>,
        payload: &Value,
    ) -> Result<(), String> {
        for hook in self.matching_hooks(event, matcher_value) {
            run_hook_command(hook, payload).await?;
        }
        Ok(())
    }

    pub async fn run_best_effort(
        &self,
        event: HookEventKind,
        matcher_value: Option<&str>,
        payload: &Value,
    ) {
        if let Err(error) = self.run(event, matcher_value, payload).await {
            tracing::warn!("hook {:?} failed: {}", event, error);
        }
    }

    fn matching_hooks(
        &self,
        event: HookEventKind,
        matcher_value: Option<&str>,
    ) -> Vec<&HookCommand> {
        hooks_for_event(&self.config, event)
            .iter()
            .filter(|hook| hook_matches(hook.matcher.as_deref(), matcher_value))
            .collect()
    }
}

fn hooks_for_event(config: &HookConfig, event: HookEventKind) -> &[HookCommand] {
    match event {
        HookEventKind::SessionStart => &config.session_start,
        HookEventKind::SessionEnd => &config.session_end,
        HookEventKind::PreToolUse => &config.pre_tool_use,
        HookEventKind::PostToolUse => &config.post_tool_use,
        HookEventKind::PostToolFailure => &config.post_tool_failure,
        HookEventKind::ApprovalRequested => &config.approval_requested,
        HookEventKind::SubagentStart => &config.subagent_start,
        HookEventKind::SubagentStop => &config.subagent_stop,
    }
}

fn hook_matches(matcher: Option<&str>, value: Option<&str>) -> bool {
    match matcher.map(str::trim) {
        None | Some("") | Some("*") => true,
        Some(matcher) => value
            .map(|value| value == matcher || value.contains(matcher))
            .unwrap_or(false),
    }
}

async fn run_hook_command(hook: &HookCommand, payload: &Value) -> Result<(), String> {
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(&hook.command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|err| err.to_string())?;

    if let Some(mut stdin) = child.stdin.take() {
        let input = serde_json::to_vec(payload).map_err(|err| err.to_string())?;
        stdin.write_all(&input).await.map_err(|err| err.to_string())?;
    }

    let output = child.wait_with_output().await.map_err(|err| err.to_string())?;
    if output.status.success() || !hook.blocking {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let reason = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("hook command `{}` failed", hook.command)
    };
    Err(reason)
}
