use crate::session_store::SessionStore;
use chrono::Utc;
use nca_common::config::NcaConfig;
use nca_common::event::{AgentCommand, AgentEvent, EndReason, EventEnvelope, QuestionSelection};
use nca_common::session::{SessionState, SessionStatus};
use nca_core::approval::ApprovalVerdict;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

pub(crate) type ApprovalPendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalVerdict>>>>;
pub(crate) type QuestionPendingMap =
    Arc<Mutex<HashMap<String, oneshot::Sender<QuestionSelection>>>>;
type EventFanoutCallback = Box<dyn Fn(&EventEnvelope) + Send>;

/// Maps a child session event to a parent-visible activity line (sidebar + transcript).
pub(crate) fn map_child_event_for_parent_broadcast(
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
        _ => None,
    }
}

fn truncate_child_detail(s: &str, max_chars: usize) -> String {
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

fn tool_input_one_line(input: &serde_json::Value) -> String {
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

/// Spawns the event fanout task: writes events to disk as `EventEnvelope`,
/// broadcasts over IPC, and renders to the provided callback.
pub fn spawn_event_fanout(
    mut event_rx: mpsc::Receiver<AgentEvent>,
    log_path: PathBuf,
    ipc_tx: Option<tokio::sync::broadcast::Sender<String>>,
    on_event: Option<EventFanoutCallback>,
    parent_forward: Option<(String, mpsc::Sender<AgentEvent>)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
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
            if let Some(ref tx) = ipc_tx {
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

/// Spawns a task that consumes IPC commands and resolves approvals/cancellation.
pub fn spawn_command_consumer(
    command_rx: mpsc::UnboundedReceiver<AgentCommand>,
    approval_pending: Option<ApprovalPendingMap>,
    question_pending: Option<QuestionPendingMap>,
    cancel_tx: Option<oneshot::Sender<()>>,
) -> tokio::task::JoinHandle<()> {
    spawn_command_consumer_with_store(
        command_rx,
        approval_pending,
        question_pending,
        cancel_tx,
        None,
        None,
        None,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionControlCommand {
    Cancel,
    Shutdown,
}

/// Extended command consumer with optional event fanout, prompt forwarding, and session control.
pub fn spawn_command_consumer_with_store(
    mut command_rx: mpsc::UnboundedReceiver<AgentCommand>,
    approval_pending: Option<ApprovalPendingMap>,
    question_pending: Option<QuestionPendingMap>,
    cancel_tx: Option<oneshot::Sender<()>>,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    prompt_tx: Option<mpsc::UnboundedSender<String>>,
    control_tx: Option<mpsc::UnboundedSender<SessionControlCommand>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut cancel = cancel_tx;
        while let Some(cmd) = command_rx.recv().await {
            match cmd {
                AgentCommand::ApproveToolCall { call_id } => {
                    if let Some(ref p) = approval_pending
                        && let Ok(mut m) = p.lock()
                        && let Some(tx) = m.remove(&call_id)
                    {
                        let _ = tx.send(ApprovalVerdict::Approved);
                    }
                }
                AgentCommand::DenyToolCall { call_id } => {
                    if let Some(ref p) = approval_pending
                        && let Ok(mut m) = p.lock()
                        && let Some(tx) = m.remove(&call_id)
                    {
                        let _ = tx.send(ApprovalVerdict::Denied);
                    }
                }
                AgentCommand::Cancel => {
                    if let Some(tx) = cancel.take() {
                        let _ = tx.send(());
                    }
                    if let Some(ref tx) = control_tx {
                        let _ = tx.send(SessionControlCommand::Cancel);
                    } else if let Some(ref tx) = event_tx {
                        let _ = tx
                            .send(AgentEvent::SessionEnded {
                                reason: EndReason::Cancelled,
                            })
                            .await;
                    }
                }
                AgentCommand::Shutdown => {
                    if let Some(tx) = cancel.take() {
                        let _ = tx.send(());
                    }
                    if let Some(ref tx) = control_tx {
                        let _ = tx.send(SessionControlCommand::Shutdown);
                    } else if let Some(ref tx) = event_tx {
                        let _ = tx
                            .send(AgentEvent::SessionEnded {
                                reason: EndReason::UserExit,
                            })
                            .await;
                    }
                    break;
                }
                AgentCommand::SendMessage { content } => {
                    if let Some(ref tx) = prompt_tx {
                        let _ = tx.send(content);
                    } else if let Some(ref tx) = event_tx {
                        let _ = tx
                            .send(AgentEvent::MessageReceived {
                                role: "user".into(),
                                content,
                            })
                            .await;
                    }
                }
                AgentCommand::AnswerQuestion {
                    question_id,
                    selection,
                } => {
                    if let Some(ref qp) = question_pending
                        && let Ok(mut m) = qp.lock()
                        && let Some(tx) = m.remove(&question_id)
                    {
                        let _ = tx.send(selection);
                    }
                }
            }
        }
    })
}

/// Query the current state of a session from its store.
pub async fn query_session_state(
    session_store: &SessionStore,
    session_id: &str,
) -> Result<SessionState, String> {
    session_store
        .load(session_id)
        .await
        .map_err(|e| e.to_string())
}

/// List all session IDs in a workspace.
pub async fn list_sessions(session_store: &SessionStore) -> Result<Vec<String>, String> {
    session_store.list().await.map_err(|e| e.to_string())
}

/// Clean up stale sessions: sessions marked as Running whose PID is no longer alive
/// and whose socket no longer exists. Marks them as Error.
pub async fn cleanup_stale_sessions(session_store: &SessionStore) {
    let ids = match session_store.list().await {
        Ok(ids) => ids,
        Err(_) => return,
    };

    for id in ids {
        let mut session = match session_store.load(&id).await {
            Ok(s) => s,
            Err(_) => continue,
        };

        if session.meta.status != SessionStatus::Running {
            continue;
        }

        let pid_alive = session.meta.pid.map(is_pid_alive).unwrap_or(false);

        let socket_exists = session
            .meta
            .socket_path
            .as_ref()
            .map(|p| p.exists())
            .unwrap_or(false);

        if !pid_alive && !socket_exists {
            session.meta.status = SessionStatus::Error;
            session.meta.updated_at = Utc::now();
            let _ = session_store.save(&session).await;
        }
    }
}

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Get the last session ID from `.nca/.last_session`, if it exists and is valid.
/// Falls back to finding the most recently updated session in the sessions directory.
pub async fn get_last_session_id(
    config: &NcaConfig,
    workspace_root: &Path,
) -> anyhow::Result<Option<String>> {
    use crate::last_session::LastSessionStore;

    // First, try the explicit last-session pointer
    let store = LastSessionStore::new(workspace_root.join(&config.session.last_session_file));
    match store.load().await {
        Ok(Some(id)) => {
            // Verify the session still exists on disk.
            let session_store = SessionStore::new(workspace_root.join(&config.session.history_dir));
            match session_store.load(&id).await {
                Ok(_) => return Ok(Some(id)),
                Err(_) => {
                    // Session file missing or corrupted; clear the stale pointer.
                    let _ = store.clear().await;
                }
            }
        }
        Ok(None) => {
            // No pointer file - fall through to scan sessions dir
        }
        Err(e) => {
            tracing::warn!("failed to load last session pointer: {}", e);
            // Fall through to scan sessions dir
        }
    }

    // Fallback: find the most recently updated session in the sessions directory
    let session_store = SessionStore::new(workspace_root.join(&config.session.history_dir));
    let ids = match session_store.list().await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::debug!("failed to list sessions: {}", e);
            return Ok(None);
        }
    };

    let mut latest: Option<(String, chrono::DateTime<chrono::Utc>)> = None;
    for id in ids {
        match session_store.load(&id).await {
            Ok(session) => {
                let should_replace = latest
                    .as_ref()
                    .map(|(_, updated_at)| session.meta.updated_at > *updated_at)
                    .unwrap_or(true);
                if should_replace {
                    latest = Some((session.meta.id, session.meta.updated_at));
                }
            }
            Err(_) => continue,
        }
    }

    if let Some((id, _)) = latest {
        // Update the last-session pointer for future runs
        let _ = store.save(&id).await;
        Ok(Some(id))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use nca_common::event::AgentCommand;
    use nca_common::message::Message;
    use nca_common::session::{SessionMeta, SessionState, SessionStatus};
    use std::fs;

    fn write_session_for_test(
        workspace: &std::path::Path,
        id: &str,
        updated_at: chrono::DateTime<Utc>,
        model: &str,
        status: SessionStatus,
    ) {
        let sessions_dir = workspace.join(".nca").join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");

        let session = SessionState {
            meta: SessionMeta {
                id: id.to_string(),
                created_at: updated_at - Duration::minutes(1),
                updated_at,
                workspace: workspace.to_path_buf(),
                model: model.to_string(),
                status,
                pid: None,
                socket_path: None,
                worktree_path: None,
                branch: None,
                base_branch: None,
                parent_session_id: None,
                child_session_ids: Vec::new(),
                inherited_summary: None,
                spawn_reason: None,
                session_summary: None,
                session_title: None,
                orchestration: None,
            },
            messages: vec![Message::user("hello")],
            total_input_tokens: 0,
            total_output_tokens: 0,
            estimated_cost_usd: 0.0,
        };

        let json = serde_json::to_string_pretty(&session).expect("serialize session");
        fs::write(sessions_dir.join(format!("{id}.json")), json).expect("write session");
    }

    #[tokio::test]
    async fn get_last_session_id_falls_back_to_most_recent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path();
        let now = Utc::now();

        // Write sessions WITHOUT .last_session file
        write_session_for_test(
            workspace,
            "session-oldest",
            now - Duration::minutes(10),
            "MiniMax-M2.5",
            SessionStatus::Completed,
        );
        write_session_for_test(
            workspace,
            "session-middle",
            now - Duration::minutes(5),
            "MiniMax-M2.5",
            SessionStatus::Completed,
        );
        write_session_for_test(
            workspace,
            "session-newest",
            now,
            "MiniMax-M2.5",
            SessionStatus::Running,
        );

        let config = nca_common::config::NcaConfig::default();
        let session_id = get_last_session_id(&config, workspace)
            .await
            .expect("get_last_session_id should succeed")
            .expect("should find a session");

        // Should find the most recent session
        assert_eq!(session_id, "session-newest");

        // The .last_session file should now be updated
        let last_session_path = workspace.join(".nca").join(".last_session");
        assert!(
            last_session_path.exists(),
            ".last_session should be created"
        );
        let content = std::fs::read_to_string(&last_session_path).unwrap();
        assert_eq!(content.trim(), "session-newest");
    }

    #[tokio::test]
    async fn send_message_forwards_prompt_to_session_queue() {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (control_tx, _control_rx) = mpsc::unbounded_channel();

        let task = spawn_command_consumer_with_store(
            cmd_rx,
            None,
            None,
            None,
            None,
            Some(prompt_tx),
            Some(control_tx),
        );

        cmd_tx
            .send(AgentCommand::SendMessage {
                content: "hello from ipc".into(),
            })
            .unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), prompt_rx.recv())
            .await
            .expect("prompt should be forwarded")
            .expect("prompt channel should remain open");
        assert_eq!(received, "hello from ipc");

        let _ = cmd_tx.send(AgentCommand::Shutdown);
        task.abort();
    }

    #[tokio::test]
    async fn answer_question_resolves_pending_channel() {
        use nca_common::event::QuestionSelection;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<QuestionSelection>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert("q-1".into(), tx);

        let task = spawn_command_consumer_with_store(
            cmd_rx,
            None,
            Some(pending.clone()),
            None,
            None,
            None,
            None,
        );

        cmd_tx
            .send(AgentCommand::AnswerQuestion {
                question_id: "q-1".into(),
                selection: QuestionSelection::Suggested,
            })
            .unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("timeout")
            .expect("channel");
        assert!(matches!(got, QuestionSelection::Suggested));

        let _ = cmd_tx.send(AgentCommand::Shutdown);
        task.abort();
    }
}
