//! Command consumers, session queries, and lifecycle utilities for supervisor.

use super::{ApprovalPendingMap, QuestionPendingMap};
use crate::last_session::LastSessionStore;
use crate::session_store::SessionStore;
use chrono::Utc;
use nca_common::config::NcaConfig;
use nca_common::event::{AgentCommand, AgentEvent, EndReason};
use nca_common::session::{SessionState, SessionStatus};
use nca_core::approval::ApprovalVerdict;
use std::path::Path;
use tokio::sync::{mpsc, oneshot};

/// Spawns a task that consumes IPC commands and resolves approvals/cancellation.
pub fn spawn_command_consumer(
    command_rx: mpsc::Receiver<AgentCommand>,
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
    /// Cooperative cancel: stop the in-flight turn and persist session state.
    Cancel,
    /// Shut down the supervisor and close the IPC socket.
    Shutdown,
}

/// Extended command consumer with optional event fanout, prompt forwarding, and session control.
pub fn spawn_command_consumer_with_store(
    mut command_rx: mpsc::Receiver<AgentCommand>,
    approval_pending: Option<ApprovalPendingMap>,
    question_pending: Option<QuestionPendingMap>,
    cancel_tx: Option<oneshot::Sender<()>>,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    prompt_tx: Option<mpsc::Sender<String>>,
    control_tx: Option<mpsc::Sender<SessionControlCommand>>,
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
                        let _ = tx.send(SessionControlCommand::Cancel).await;
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
                        let _ = tx.send(SessionControlCommand::Shutdown).await;
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
                        let _ = tx.send(content).await;
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

        let pid_alive = session.meta.pid.map(is_pid_alive_impl).unwrap_or(false);

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

pub fn is_pid_alive(pid: u32) -> bool {
    is_pid_alive_impl(pid)
}

fn is_pid_alive_impl(pid: u32) -> bool {
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
    let store = LastSessionStore::new(workspace_root.join(&config.session.last_session_file));
    match store.load().await {
        Ok(Some(id)) => {
            let session_store = SessionStore::new(workspace_root.join(&config.session.history_dir));
            match session_store.load(&id).await {
                Ok(_) => return Ok(Some(id)),
                Err(_) => {
                    let _ = store.clear().await;
                }
            }
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!("failed to load last session pointer: {}", e);
        }
    }

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
        let _ = store.save(&id).await;
        Ok(Some(id))
    } else {
        Ok(None)
    }
}
