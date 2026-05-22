//! Attach to live sessions and print event logs.

use crate::cmd::util::{print_event_envelope, print_json};
use nca_common::config::NcaConfig;
use nca_common::event::{AgentCommand, AgentEvent, EndReason, EventEnvelope};
use nca_runtime::ipc::IpcClient;
use std::path::Path;
use std::time::Duration;

pub async fn show_logs(
    config: &NcaConfig,
    workspace_root: &Path,
    session_id: &str,
    follow: bool,
    json: bool,
) -> anyhow::Result<()> {
    if follow {
        return attach_session(config, workspace_root, session_id, json).await;
    }
    print_log_file(config, workspace_root, session_id, json).await
}

pub async fn attach_session(
    config: &NcaConfig,
    workspace_root: &Path,
    session_id: &str,
    json: bool,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let session = store.load(session_id).await.map_err(anyhow::Error::msg)?;

    if let Some(socket_path) = session.meta.socket_path.clone() {
        let client = nca_runtime::ipc::IpcClient::new(socket_path);
        if let Ok(mut rx) = client.connect().await {
            while let Some(envelope) = rx.recv().await {
                print_event_envelope(&envelope, json)?;
            }
            return Ok(());
        }
    }

    print_log_file(config, workspace_root, session_id, json).await
}

pub async fn wait_for_session_end(
    client: &nca_runtime::ipc::IpcClient,
    expected: EndReason,
    timeout: Duration,
) -> bool {
    let Ok(mut events) = client.connect().await else {
        return false;
    };

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }

        tokio::select! {
            maybe = events.recv() => {
                match maybe {
                    Some(envelope) => {
                        if matches!(
                            envelope.event,
                            AgentEvent::SessionEnded { reason, .. } if reason == expected
                        ) {
                            return true;
                        }
                    }
                    None => return false,
                }
            }
            _ = tokio::time::sleep(remaining.min(Duration::from_millis(100))) => {}
        }
    }
}

pub async fn print_log_file(
    config: &NcaConfig,
    workspace_root: &Path,
    session_id: &str,
    json: bool,
) -> anyhow::Result<()> {
    let log_path = workspace_root
        .join(&config.session.history_dir)
        .join(format!("{session_id}.events.jsonl"));
    let data = match tokio::fs::read_to_string(&log_path).await {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!("No event log found for {session_id}");
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };
    for line in data.lines() {
        let envelope: EventEnvelope = serde_json::from_str(line)?;
        print_event_envelope(&envelope, json)?;
    }
    Ok(())
}
