//! Shared text formatting utilities used across TUI components.

use serde_json::Value;

/// Truncate a string to at most `max` characters (Unicode-aware), trimming whitespace.
/// Appends `"…"` when truncated.
pub fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        format!(
            "{}…",
            t.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

/// Return the first 8 characters of a session id (or the full id if shorter).
pub(crate) fn short_session_prefix(id: &str) -> &str {
    if id.len() > 8 { &id[..8] } else { id }
}

/// Format tool input for display in the transcript.
/// Special-cases `spawn_subagent` for a compact multi-line summary.
pub(crate) fn format_tool_input_for_display(tool: &str, value: &Value) -> String {
    if tool == "spawn_subagent" {
        format_spawn_subagent_input(value)
    } else {
        format_tool_input(value)
    }
}

fn format_spawn_subagent_input(v: &Value) -> String {
    let task = v.get("task").and_then(|t| t.as_str()).unwrap_or("").trim();
    let wt = v
        .get("use_worktree")
        .and_then(|b| b.as_bool())
        .unwrap_or(true);
    let n_focus = v
        .get("focus_files")
        .and_then(|a| a.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    format!(
        "task:\n{}\nworktree: {} · focus_files: {}",
        truncate(task, 500),
        wt,
        n_focus
    )
}

fn format_tool_input(value: &Value) -> String {
    if let Some(raw) = value.as_str()
        && let Ok(parsed) = serde_json::from_str::<Value>(raw)
    {
        return serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| raw.to_string());
    }
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}
