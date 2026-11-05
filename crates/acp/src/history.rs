//! Replay a persisted nca event log as ACP `session/update` notifications.

use crate::translate;
use agent_client_protocol::schema::v1::{SessionId, SessionNotification};
use agent_client_protocol::{Client, ConnectionTo};
use nca_common::event::{AgentEvent, EventEnvelope};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Stream the session history to the client. Returns the last event id seen.
pub async fn replay(
    log_path: &Path,
    session_id: &SessionId,
    cx: &ConnectionTo<Client>,
    workspace: &Path,
) -> Result<u64, String> {
    let file = match tokio::fs::File::open(log_path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };

    let mut lines = BufReader::new(file).lines();
    let mut last_id = 0u64;
    let mut text = String::new();
    let mut saw_token = false;
    let mut tool_inputs = std::collections::HashMap::new();

    while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(envelope) = serde_json::from_str::<EventEnvelope>(&line) else {
            continue;
        };
        last_id = last_id.max(envelope.id);

        match envelope.event {
            AgentEvent::MessageReceived { role, content } => {
                if role == "user" {
                    flush_text(cx, session_id, &mut text);
                    send(cx, session_id, translate::user_chunk(content));
                } else if !saw_token {
                    send(cx, session_id, translate::text_chunk(content));
                }
                saw_token = false;
            }
            AgentEvent::TokensStreamed { delta } => {
                text.push_str(&delta);
                saw_token = true;
            }
            AgentEvent::ToolCallStarted {
                call_id,
                tool,
                input,
            } => {
                flush_text(cx, session_id, &mut text);
                saw_token = false;
                send(
                    cx,
                    session_id,
                    translate::tool_started(&call_id, &tool, &input, workspace),
                );
                tool_inputs.insert(call_id, (tool, input));
            }
            AgentEvent::ToolCallCompleted { call_id, output } => {
                flush_text(cx, session_id, &mut text);
                saw_token = false;
                // Images are re-read from disk; one that was deleted since
                // is skipped and only its text result is replayed.
                let (tool, input) = tool_inputs
                    .remove(&call_id)
                    .unwrap_or((String::new(), serde_json::Value::Null));
                for update in
                    translate::tool_completed_updates(&call_id, &tool, &input, &output, workspace)
                        .await
                {
                    send(cx, session_id, update);
                }
            }
            AgentEvent::TodosUpdated { todos } => {
                flush_text(cx, session_id, &mut text);
                saw_token = false;
                send(cx, session_id, translate::plan_update(&todos));
            }
            AgentEvent::SessionEnded { .. } => {
                flush_text(cx, session_id, &mut text);
                saw_token = false;
            }
            _ => {}
        }
    }

    flush_text(cx, session_id, &mut text);
    Ok(last_id)
}

fn flush_text(cx: &ConnectionTo<Client>, session_id: &SessionId, text: &mut String) {
    if text.is_empty() {
        return;
    }
    let chunk = std::mem::take(text);
    send(cx, session_id, translate::text_chunk(chunk));
}

fn send(
    cx: &ConnectionTo<Client>,
    session_id: &SessionId,
    update: agent_client_protocol::schema::v1::SessionUpdate,
) {
    if let Err(error) = cx.send_notification(SessionNotification::new(session_id.clone(), update)) {
        tracing::warn!("failed to replay session update: {error}");
    }
}
