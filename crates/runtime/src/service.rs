use crate::session_store::SessionStore;
use crate::supervisor::{
    SessionControlCommand, Supervisor, SupervisorConfig, spawn_command_consumer_with_store,
    spawn_subagent_consumer,
};
use nca_common::config::NcaConfig;
use nca_common::event::{AgentEvent, EndReason, EventEnvelope};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc as std_mpsc;

#[derive(Debug, Clone)]
pub enum ServiceSessionKind {
    New {
        session_id: Option<String>,
    },
    Resume {
        session_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct ServiceSessionRequest {
    pub config: NcaConfig,
    pub workspace_root: PathBuf,
    pub safe_mode: bool,
    pub initial_prompt: Option<String>,
    pub kind: ServiceSessionKind,
}

#[derive(Debug, Clone)]
pub struct ServiceSessionInfo {
    pub session_id: String,
    pub workspace_root: PathBuf,
    pub model: String,
    pub socket_path: Option<PathBuf>,
    pub event_log_path: PathBuf,
}

pub struct ServiceSessionHandle {
    info: ServiceSessionInfo,
    #[allow(dead_code)]
    join_handle: Option<std::thread::JoinHandle<()>>,
}

impl ServiceSessionHandle {
    pub fn info(&self) -> &ServiceSessionInfo {
        &self.info
    }
}

pub fn spawn_service_session(
    request: ServiceSessionRequest,
) -> Result<ServiceSessionHandle, String> {
    let (startup_tx, startup_rx) = std_mpsc::channel::<Result<ServiceSessionInfo, String>>();
    let join_handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();

        match runtime {
            Ok(rt) => {
                if let Err(error) = rt.block_on(run_service_session_with_startup(
                    request,
                    Some(startup_tx.clone()),
                )) {
                    let _ = startup_tx.send(Err(error));
                }
            }
            Err(error) => {
                let _ = startup_tx.send(Err(format!("failed to create tokio runtime: {error}")));
            }
        }
    });

    let info = startup_rx
        .recv()
        .map_err(|_| "service session failed before startup completed".to_string())??;

    Ok(ServiceSessionHandle {
        info,
        join_handle: Some(join_handle),
    })
}

pub async fn run_service_session(request: ServiceSessionRequest) -> Result<(), String> {
    run_service_session_with_startup(request, None).await
}

async fn run_service_session_with_startup(
    request: ServiceSessionRequest,
    startup_tx: Option<std_mpsc::Sender<Result<ServiceSessionInfo, String>>>,
) -> Result<(), String> {
    let mut supervisor = match &request.kind {
        ServiceSessionKind::New { session_id } => Supervisor::create(SupervisorConfig {
            config: request.config.clone(),
            workspace_root: request.workspace_root.clone(),
            safe_mode: request.safe_mode,
            interactive_approvals: true,
            session_id: session_id.clone(),
            approval_handler: None,
        })
        .await,
        ServiceSessionKind::Resume { session_id } => {
            Supervisor::resume(
                request.config.clone(),
                &request.workspace_root,
                request.safe_mode,
                true,
                session_id,
            )
            .await
        }
    }
    .map_err(|error| error.to_string())?;

    let mut handle = supervisor.take_handle();
    let info = ServiceSessionInfo {
        session_id: handle.session_id.clone(),
        workspace_root: handle.workspace_root.clone(),
        model: handle.model.clone(),
        socket_path: handle.socket_path.clone(),
        event_log_path: handle.event_log_path.clone(),
    };

    if let Some(tx) = startup_tx {
        let _ = tx.send(Ok(info.clone()));
    }

    let event_rx = handle
        .take_event_rx()
        .ok_or_else(|| "missing event receiver".to_string())?;
    let approval_pending = handle.take_approval_pending();

    let mut command_rx = None;
    let mut event_tx_ipc = None;
    if let Some(ipc_handle) = handle.take_ipc_handle() {
        let (etx, crx) = ipc_handle.into_parts();
        event_tx_ipc = Some(etx);
        command_rx = Some(crx);
    }

    let fanout_task =
        spawn_service_event_fanout(event_rx, info.event_log_path.clone(), event_tx_ipc);

    let subagent_task = if let Some(spawn_rx) = handle.take_spawn_rx() {
        Some(spawn_subagent_consumer(
            spawn_rx,
            info.session_id.clone(),
            info.workspace_root.clone(),
            request.config.clone(),
            supervisor.agent().messages.clone(),
            supervisor.event_tx(),
        ))
    } else {
        None
    };

    let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (control_tx, mut control_rx) =
        tokio::sync::mpsc::unbounded_channel::<SessionControlCommand>();

    let command_task = command_rx.map(|crx| {
        spawn_command_consumer_with_store(
            crx,
            approval_pending,
            None,
            Some(SessionStore::new(
                info.workspace_root
                    .join(&request.config.session.history_dir),
            )),
            supervisor.event_tx(),
            Some(prompt_tx.clone()),
            Some(control_tx.clone()),
        )
    });

    if let Some(prompt) = request.initial_prompt.clone() {
        let _ = prompt_tx.send(prompt);
    }

    let cancel_handle = supervisor.cancel_handle();
    let mut reason = EndReason::UserExit;

    loop {
        let prompt = tokio::select! {
            control = control_rx.recv() => {
                match control {
                    Some(SessionControlCommand::Cancel) => {
                        cancel_handle.store(true, Ordering::SeqCst);
                        reason = EndReason::Cancelled;
                        break;
                    }
                    Some(SessionControlCommand::Shutdown) => {
                        cancel_handle.store(true, Ordering::SeqCst);
                        reason = EndReason::UserExit;
                        break;
                    }
                    None => break,
                }
            }
            prompt = prompt_rx.recv() => match prompt {
                Some(prompt) => prompt,
                None => break,
            }
        };

        let run_fut = supervisor.run_turn(&prompt);
        tokio::pin!(run_fut);

        let result = tokio::select! {
            result = &mut run_fut => result,
            control = control_rx.recv() => {
                match control {
                    Some(SessionControlCommand::Cancel) => {
                        cancel_handle.store(true, Ordering::SeqCst);
                        reason = EndReason::Cancelled;
                    }
                    Some(SessionControlCommand::Shutdown) => {
                        cancel_handle.store(true, Ordering::SeqCst);
                        reason = EndReason::UserExit;
                    }
                    None => {}
                }
                run_fut.await
            }
        };

        if let Err(error) = result {
            if error.to_string().contains("run cancelled") {
                if matches!(reason, EndReason::Cancelled | EndReason::UserExit) {
                    break;
                }
                continue;
            }
            reason = EndReason::Error;
            break;
        }
    }

    supervisor.finish(reason).await;
    fanout_task.abort();
    if let Some(task) = command_task {
        task.abort();
    }
    if let Some(task) = subagent_task {
        task.abort();
    }
    Ok(())
}

fn spawn_service_event_fanout(
    mut event_rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    log_path: PathBuf,
    event_tx_ipc: Option<tokio::sync::broadcast::Sender<String>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use tokio::fs::OpenOptions;
        use tokio::io::AsyncWriteExt;

        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .ok();

        let mut event_id: u64 = 0;
        while let Some(event) = event_rx.recv().await {
            if let Some(ref tx) = event_tx_ipc {
                let line = serde_json::to_string(&event).unwrap_or_default();
                let _ = tx.send(line);
            }

            if let Some(file) = log_file.as_mut() {
                event_id += 1;
                let envelope = EventEnvelope {
                    id: event_id,
                    ts: Some(chrono::Utc::now()),
                    event: event.clone(),
                };
                if let Ok(line) = serde_json::to_string(&envelope) {
                    let _ = file.write_all(line.as_bytes()).await;
                    let _ = file.write_all(b"\n").await;
                }
            }
        }
    })
}
