//! Session listing, status, resume, and cost dashboard.

use crate::approval_prompts::InteractiveIpcApprovalHandler;
use crate::cli::SessionStatusFilter;
use crate::cmd::attach::wait_for_session_end;
use crate::cmd::util::{print_human_session, print_json};
use crate::stream::{StreamMode, spawn_stream_task};
use nca_common::config::NcaConfig;
use nca_common::event::{AgentCommand, EndReason};
use nca_common::session::{SessionSnapshot, SessionStatus};
use nca_tui::{Repl, build_resumed_session_runtime};
use std::io::{IsTerminal, stdin, stdout};
use std::path::Path;
use std::time::Duration;

#[derive(serde::Serialize)]
pub(crate) struct SessionListOutput {
    sessions: Vec<SessionSnapshot>,
    unreadable: Vec<String>,
}

#[derive(serde::Serialize)]
pub(crate) struct CancelCommandOutput {
    session: SessionSnapshot,
    cancelled: bool,
}

pub async fn list_sessions(
    config: &NcaConfig,
    workspace_root: &Path,
    json: bool,
    status_filter: Option<SessionStatusFilter>,
    since_hours: Option<u32>,
    search: Option<String>,
    limit: usize,
) -> anyhow::Result<()> {
    use nca_common::session::SessionStatus;

    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let ids = store.list().await.map_err(anyhow::Error::msg)?;
    let mut sessions = Vec::new();
    let mut unreadable = Vec::new();

    for id in ids {
        match store.load_snapshot(&id).await {
            Ok(session) => sessions.push(session),
            Err(_) => unreadable.push(id),
        }
    }

    // Apply status filter
    if let Some(status) = status_filter {
        sessions.retain(|s| match status {
            SessionStatusFilter::Running => matches!(s.status, SessionStatus::Running),
            SessionStatusFilter::Completed => matches!(s.status, SessionStatus::Completed),
            SessionStatusFilter::Cancelled => matches!(s.status, SessionStatus::Cancelled),
            SessionStatusFilter::Failed => matches!(s.status, SessionStatus::Error),
        });
    }

    // Apply time filter
    if let Some(hours) = since_hours {
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
        sessions.retain(|s| s.updated_at > cutoff);
    }

    // Apply search filter
    if let Some(pattern) = search {
        let pattern_lower = pattern.to_lowercase();
        sessions.retain(|s| {
            s.id.to_lowercase().contains(&pattern_lower)
                || s.session_summary
                    .as_ref()
                    .map(|sum| sum.to_lowercase().contains(&pattern_lower))
                    .unwrap_or(false)
                || s.model.to_lowercase().contains(&pattern_lower)
        });
    }

    sessions.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });

    // Apply limit
    sessions.truncate(limit);
    unreadable.sort();

    if json {
        print_json(
            &SessionListOutput {
                sessions,
                unreadable,
            },
            false,
        )?;
    } else {
        for session in sessions {
            print_human_session(&session);
        }
        for id in unreadable {
            println!("{id}\tUnreadable");
        }
    }
    Ok(())
}

pub async fn latest_session_id(
    config: &NcaConfig,
    workspace_root: &Path,
) -> anyhow::Result<String> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let ids = store.list().await.map_err(anyhow::Error::msg)?;
    let mut latest = None;

    for id in ids {
        let Ok(session) = store.load(&id).await else {
            continue;
        };

        let should_replace = latest
            .as_ref()
            .map(|(_, updated_at)| session.meta.updated_at > *updated_at)
            .unwrap_or(true);
        if should_replace {
            latest = Some((session.meta.id, session.meta.updated_at));
        }
    }

    latest
        .map(|(id, _)| id)
        .ok_or_else(|| anyhow::anyhow!("no saved sessions found to resume"))
}

pub async fn resume_session(
    config: NcaConfig,
    workspace_root: &Path,
    session_id: &str,
    prompt: Option<String>,
    safe: bool,
    stream: StreamMode,
    no_tui: bool,
) -> anyhow::Result<()> {
    let use_tui = !no_tui
        && stdout().is_terminal()
        && stdin().is_terminal()
        && matches!(stream, StreamMode::Human);
    let (approval_handler, approval_pending_cfg) = if use_tui {
        (None, None)
    } else {
        let (handler, pending) = InteractiveIpcApprovalHandler::new();
        (
            Some(handler as std::sync::Arc<dyn nca_core::approval::ApprovalHandler>),
            Some(pending),
        )
    };
    let mut runtime = build_resumed_session_runtime(
        config,
        workspace_root,
        safe,
        true,
        session_id,
        approval_handler,
        approval_pending_cfg,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    if let Some(prompt) = prompt {
        if let Some(rx) = runtime.take_event_rx() {
            let ipc_handle = runtime.take_ipc_handle();
            let approval_pending = runtime.take_ipc_approval_pending();
            let _stream_task = spawn_stream_task(
                rx,
                stream,
                runtime.event_log_path(),
                ipc_handle,
                approval_pending,
                runtime.question_pending(),
                None,
            );
        }
        let output = runtime
            .run_turn(&prompt)
            .await
            .map_err(anyhow::Error::msg)?;
        println!("{output}");
        return Ok(());
    }

    if !use_tui && let Some(rx) = runtime.take_event_rx() {
        let ipc_handle = runtime.take_ipc_handle();
        let approval_pending = runtime.take_ipc_approval_pending();
        let _stream_task = spawn_stream_task(
            rx,
            stream,
            runtime.event_log_path(),
            ipc_handle,
            approval_pending,
            runtime.question_pending(),
            None,
        );
    }
    let mut repl = Repl::new(runtime, safe, true);
    if use_tui {
        repl.run_with_tui().await?;
    } else {
        repl.run().await?;
    }
    Ok(())
}

pub async fn show_status(
    config: &NcaConfig,
    workspace_root: &Path,
    session_id: &str,
    json: bool,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let snapshot = store
        .load_snapshot(session_id)
        .await
        .map_err(anyhow::Error::msg)?;
    if json {
        print_json(&snapshot, false)?;
    } else {
        print_human_session(&snapshot);
    }
    Ok(())
}

pub async fn cancel_session(
    config: &NcaConfig,
    workspace_root: &Path,
    session_id: &str,
    json: bool,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let mut session = store.load(session_id).await.map_err(anyhow::Error::msg)?;

    if let Some(socket_path) = session.meta.socket_path.clone() {
        let client = nca_runtime::ipc::IpcClient::new(socket_path);
        let _ = client.send_command(&AgentCommand::Cancel).await;

        if !wait_for_session_end(&client, EndReason::Cancelled, Duration::from_secs(10)).await {
            let _ = client.send_command(&AgentCommand::Shutdown).await;
            let _ =
                wait_for_session_end(&client, EndReason::UserExit, Duration::from_secs(2)).await;
        }
    }

    if let Some(pid) = session.meta.pid {
        if nca_runtime::supervisor::is_pid_alive(pid) {
            let _ = tokio::process::Command::new("kill")
                .arg(pid.to_string())
                .output()
                .await;
        }
    }

    session.meta.status = SessionStatus::Cancelled;
    session.meta.updated_at = chrono::Utc::now();
    session.meta.pid = None;
    session.meta.socket_path = None;
    store.save(&session).await.map_err(anyhow::Error::msg)?;
    if json {
        print_json(
            &CancelCommandOutput {
                session: session.snapshot(),
                cancelled: true,
            },
            false,
        )?;
    } else {
        println!("Cancelled {session_id}");
    }
    Ok(())
}

pub async fn run_cost_dashboard(
    config: &NcaConfig,
    workspace_root: &Path,
    since_hours: Option<u32>,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let ids = store.list().await.map_err(anyhow::Error::msg)?;
    let mut sessions = Vec::new();
    for id in ids {
        if let Ok(snap) = store.load_snapshot(&id).await {
            sessions.push(snap);
        }
    }
    if let Some(hours) = since_hours {
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
        sessions.retain(|s| s.updated_at > cutoff);
    }
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.truncate(limit);

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut total_cost: f64 = 0.0;
    for s in &sessions {
        total_in += s.total_input_tokens;
        total_out += s.total_output_tokens;
        total_cost += s.estimated_cost_usd;
    }

    if json {
        let payload = serde_json::json!({
            "sessions": sessions.iter().map(|s| serde_json::json!({
                "id": s.id,
                "model": s.model,
                "updated_at": s.updated_at,
                "input_tokens": s.total_input_tokens,
                "output_tokens": s.total_output_tokens,
                "estimated_cost_usd": s.estimated_cost_usd,
            })).collect::<Vec<_>>(),
            "totals": {
                "input_tokens": total_in,
                "output_tokens": total_out,
                "estimated_cost_usd": total_cost,
            }
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "{:<24} {:<28} {:>10} {:>10} {:>10}",
            "SESSION", "MODEL", "IN", "OUT", "COST"
        );
        println!("{}", "-".repeat(84));
        for s in &sessions {
            let id_short: String = s.id.chars().take(22).collect();
            let model_short: String = s.model.chars().take(26).collect();
            println!(
                "{:<24} {:<28} {:>10} {:>10} {:>10}",
                id_short,
                model_short,
                s.total_input_tokens,
                s.total_output_tokens,
                format!("${:.4}", s.estimated_cost_usd)
            );
        }
        println!("{}", "-".repeat(84));
        println!(
            "{:<24} {:<28} {:>10} {:>10} {:>10}",
            format!("{} sessions", sessions.len()),
            "",
            total_in,
            total_out,
            format!("${:.4}", total_cost)
        );
    }
    Ok(())
}
