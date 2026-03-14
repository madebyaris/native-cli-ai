use nca_common::event::{AgentEvent, EventEnvelope};
use nca_runtime::ipc::IpcHandle;
use nca_runtime::supervisor;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum StreamMode {
    Off,
    Human,
    Ndjson,
}

/// Spawns the stream task: event fanout (disk + IPC + rendering) and command consumer.
pub fn spawn_stream_task(
    rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    mode: StreamMode,
    log_path: std::path::PathBuf,
    ipc_handle: Option<IpcHandle>,
    approval_pending: Option<Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>>,
    cancel_tx: Option<oneshot::Sender<()>>,
) -> tokio::task::JoinHandle<()> {
    let (event_tx_ipc, command_rx) = match ipc_handle {
        Some(h) => {
            let (etx, crx) = h.into_parts();
            (Some(etx), Some(crx))
        }
        None => (None, None),
    };

    if let Some(crx) = command_rx {
        supervisor::spawn_command_consumer(crx, approval_pending, cancel_tx);
    }

    spawn_event_fanout_task(rx, mode, log_path, event_tx_ipc)
}

pub fn spawn_event_fanout_task(
    rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    mode: StreamMode,
    log_path: std::path::PathBuf,
    event_tx_ipc: Option<tokio::sync::broadcast::Sender<String>>,
) -> tokio::task::JoinHandle<()> {
    let on_event: Option<Box<dyn Fn(&EventEnvelope) + Send>> = match mode {
        StreamMode::Off => None,
        StreamMode::Ndjson => Some(Box::new(|envelope: &EventEnvelope| {
            if let Ok(line) = serde_json::to_string(envelope) {
                println!("{line}");
            }
        })),
        StreamMode::Human => Some(Box::new(|envelope: &EventEnvelope| {
            render_human_event(&envelope.event);
        })),
    };

    let ipc_handle_rebuilt = event_tx_ipc.map(|tx| IpcRebroadcast { event_tx: tx });

    tokio::spawn(async move {
        use nca_common::event::EventEnvelope;
        use tokio::fs::OpenOptions;
        use tokio::io::AsyncWriteExt;

        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .ok();

        let mut event_id: u64 = 0;
        let mut rx = rx;
        while let Some(event) = rx.recv().await {
            event_id += 1;
            let envelope = EventEnvelope::new(event_id, event);

            if let Some(ref ipc) = ipc_handle_rebuilt {
                let line = serde_json::to_string(&envelope).unwrap_or_default();
                let _ = ipc.event_tx.send(line);
            }

            if let Some(file) = log_file.as_mut() {
                if let Ok(line) = serde_json::to_string(&envelope) {
                    let _ = file.write_all(line.as_bytes()).await;
                    let _ = file.write_all(b"\n").await;
                }
            }

            if let Some(ref cb) = on_event {
                cb(&envelope);
            }
        }
    })
}

struct IpcRebroadcast {
    event_tx: tokio::sync::broadcast::Sender<String>,
}

pub(crate) fn render_human_event(event: &AgentEvent) {
    match event {
        AgentEvent::SessionStarted {
            session_id, model, ..
        } => eprintln!("[session] {session_id} model={model}"),
        AgentEvent::TokensStreamed { delta } => {
            print!("{delta}");
        }
        AgentEvent::ToolCallStarted { tool, input, .. } => {
            eprintln!("\n[tool:start] {tool} {input}");
        }
        AgentEvent::ToolCallCompleted { output, .. } => {
            if output.success {
                eprintln!("[tool:done] ok");
            } else {
                eprintln!(
                    "[tool:done] error: {}",
                    output.error.as_deref().unwrap_or("tool failed")
                );
            }
        }
        AgentEvent::ApprovalRequested {
            tool, description, ..
        } => {
            eprintln!("[approval] {tool}: {description}");
        }
        AgentEvent::ApprovalResolved { approved, .. } => {
            eprintln!("[approval] resolved={approved}");
        }
        AgentEvent::Checkpoint {
            phase,
            detail,
            turn,
        } => {
            eprintln!("[checkpoint] turn={turn} phase={phase} {detail}");
        }
        AgentEvent::SessionEnded { reason } => {
            eprintln!("\n[session:end] {:?}", reason);
        }
        AgentEvent::Error { message } => {
            eprintln!("[error] {message}");
        }
        AgentEvent::Response { response } => {
            eprintln!(
                "[response] {}",
                serde_json::to_string(response).unwrap_or_default()
            );
        }
        AgentEvent::ChildSessionSpawned {
            child_session_id,
            task,
            ..
        } => {
            eprintln!("[subagent:spawn] {child_session_id} task={task}");
        }
        AgentEvent::ChildSessionCompleted {
            child_session_id,
            status,
            ..
        } => {
            eprintln!("[subagent:done] {child_session_id} status={status}");
        }
        AgentEvent::MessageReceived { .. }
        | AgentEvent::CostUpdated { .. }
        | AgentEvent::TodoStatusChanged { .. }
        | AgentEvent::TodoAssigned { .. }
        | AgentEvent::RunLinked { .. }
        | AgentEvent::DesktopModeChanged { .. } => {}
    }
}
