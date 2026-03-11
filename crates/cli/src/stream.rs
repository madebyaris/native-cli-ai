use nca_common::event::AgentEvent;
use nca_runtime::ipc::IpcHandle;
use std::path::PathBuf;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum StreamMode {
    Off,
    Human,
    Ndjson,
}

pub fn spawn_stream_task(
    mut rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    mode: StreamMode,
    log_path: PathBuf,
    ipc_handle: Option<IpcHandle>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let ipc_handle = ipc_handle;
        let mut log_file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
        {
            Ok(file) => Some(file),
            Err(_) => None,
        };

        while let Some(event) = rx.recv().await {
            if let Some(handle) = ipc_handle.as_ref() {
                let _ = handle.broadcast(&event).await;
            }

            if let Some(file) = log_file.as_mut() {
                if let Ok(line) = serde_json::to_string(&event) {
                    let _ = file.write_all(line.as_bytes()).await;
                    let _ = file.write_all(b"\n").await;
                }
            }

            match mode {
                StreamMode::Off => {}
                StreamMode::Ndjson => {
                    if let Ok(line) = serde_json::to_string(&event) {
                        println!("{line}");
                    }
                }
                StreamMode::Human => render_human_event(&event),
            }
        }
    })
}

fn render_human_event(event: &AgentEvent) {
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
        AgentEvent::ApprovalRequested { tool, description, .. } => {
            eprintln!("[approval] {tool}: {description}");
        }
        AgentEvent::ApprovalResolved { approved, .. } => {
            eprintln!("[approval] resolved={approved}");
        }
        AgentEvent::Checkpoint { phase, detail, turn } => {
            eprintln!("[checkpoint] turn={turn} phase={phase} {detail}");
        }
        AgentEvent::SessionEnded { reason } => {
            eprintln!("\n[session:end] {:?}", reason);
        }
        AgentEvent::Error { message } => {
            eprintln!("[error] {message}");
        }
        AgentEvent::MessageReceived { .. } | AgentEvent::CostUpdated { .. } => {}
    }
}
