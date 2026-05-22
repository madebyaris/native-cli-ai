//! Event fanout: disk logging, IPC broadcast, and optional parent forwarding.

use crate::ipc::IpcHandle;
use nca_common::event::{AgentEvent, EventEnvelope};
use std::path::PathBuf;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

pub type EventFanoutCallback = Box<dyn Fn(&EventEnvelope) + Send>;

pub(super) fn truncate_child_detail(s: &str, max_chars: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max_chars {
        t.to_string()
    } else {
        format!(
            "{}…",
            t.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

pub(super) fn tool_input_one_line(input: &serde_json::Value) -> String {
    if let Some(s) = input.as_str() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
            return tool_input_one_line(&v);
        }
        return truncate_child_detail(s, 120);
    }
    if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
        return truncate_child_detail(cmd, 120);
    }
    if let Some(p) = input
        .get("path")
        .or_else(|| input.get("file_path"))
        .and_then(|v| v.as_str())
    {
        return truncate_child_detail(p, 120);
    }
    let s = serde_json::to_string(input).unwrap_or_default();
    truncate_child_detail(&s, 120)
}

/// Maps a child session event to a parent-visible activity line (sidebar + transcript).
fn map_child_event_for_parent_broadcast(
    child_session_id: &str,
    event: &AgentEvent,
) -> Option<AgentEvent> {
    match event {
        AgentEvent::ToolCallStarted { tool, input, .. } => Some(AgentEvent::ChildSessionActivity {
            child_session_id: child_session_id.to_string(),
            phase: tool.clone(),
            detail: tool_input_one_line(input),
        }),
        AgentEvent::Checkpoint { phase, detail, .. } => Some(AgentEvent::ChildSessionActivity {
            child_session_id: child_session_id.to_string(),
            phase: phase.clone(),
            detail: truncate_child_detail(detail, 120),
        }),
        AgentEvent::ChildSessionSpawned { task, .. } => Some(AgentEvent::ChildSessionActivity {
            child_session_id: child_session_id.to_string(),
            phase: "nested_subagent".to_string(),
            detail: truncate_child_detail(task, 120),
        }),
        AgentEvent::Error { message } => Some(AgentEvent::ChildSessionActivity {
            child_session_id: child_session_id.to_string(),
            phase: "error".to_string(),
            detail: truncate_child_detail(message, 160),
        }),
        AgentEvent::CostUpdated {
            input_tokens,
            output_tokens,
            ..
        } => Some(AgentEvent::ChildSessionActivity {
            child_session_id: child_session_id.to_string(),
            phase: "tokens".to_string(),
            detail: format!("{input_tokens}/{output_tokens}"),
        }),
        _ => None,
    }
}

/// Spawns the event fanout task: writes events to disk as `EventEnvelope`,
/// broadcasts over IPC, and renders to the provided callback.
pub fn spawn_event_fanout(
    mut event_rx: mpsc::Receiver<AgentEvent>,
    log_path: PathBuf,
    ipc_handle: Option<IpcHandle>,
    on_event: Option<EventFanoutCallback>,
    parent_forward: Option<(String, mpsc::Sender<AgentEvent>)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (event_tx, _command_rx) = match ipc_handle {
            Some(h) => {
                let (etx, crx) = h.into_parts();
                (Some(etx), Some(crx))
            }
            None => (None, None),
        };

        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .ok();

        let mut event_id: u64 = 0;
        while let Some(event) = event_rx.recv().await {
            event_id += 1;
            if let Some((ref child_id, ref ptx)) = parent_forward
                && let Some(fwd) = map_child_event_for_parent_broadcast(child_id, &event)
            {
                let _ = ptx.send(fwd).await;
            }
            let envelope = EventEnvelope::new(event_id, event);
            if let Some(ref tx) = event_tx {
                let line = serde_json::to_string(&envelope).unwrap_or_default();
                let _ = tx.send(line);
            }

            if let Some(file) = log_file.as_mut()
                && let Ok(line) = serde_json::to_string(&envelope)
            {
                let _ = file.write_all(line.as_bytes()).await;
                let _ = file.write_all(b"\n").await;
            }

            if let Some(ref cb) = on_event {
                cb(&envelope);
            }
        }
    })
}
