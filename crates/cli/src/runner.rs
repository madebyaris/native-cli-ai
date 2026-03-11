use crate::approval_prompt::{AutoDenyApprovalHandler, StdioApprovalHandler};
use crate::runtime_tooling::RuntimeBashTool;
use chrono::Utc;
use nca_common::config::NcaConfig;
use nca_common::event::{AgentEvent, EndReason};
use nca_common::session::{SessionMeta, SessionState, SessionStatus};
use nca_core::agent::AgentLoop;
use nca_core::approval::ApprovalPolicy;
use nca_core::harness::build_system_prompt;
use nca_core::provider::factory::build_provider;
use nca_core::provider::ProviderError;
use nca_core::tools::ToolRegistry;
use nca_runtime::ipc::{IpcHandle, IpcServer};
use nca_runtime::pty::PtyManager;
use nca_runtime::session_store::SessionStore;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct SessionRuntime {
    pub agent: AgentLoop,
    pub session_id: String,
    pub session_store: SessionStore,
    pub workspace_root: std::path::PathBuf,
    pub model: String,
    created_at: chrono::DateTime<Utc>,
    status: SessionStatus,
    pid: Option<u32>,
    socket_path: Option<std::path::PathBuf>,
    ipc_handle: Option<IpcHandle>,
    event_rx: Option<tokio::sync::mpsc::Receiver<AgentEvent>>,
}

impl SessionRuntime {
    pub fn take_event_rx(&mut self) -> Option<tokio::sync::mpsc::Receiver<AgentEvent>> {
        self.event_rx.take()
    }

    pub fn event_log_path(&self) -> std::path::PathBuf {
        self.session_store
            .sessions_dir()
            .join(format!("{}.events.jsonl", self.session_id))
    }

    pub async fn run_turn(&mut self, prompt: &str) -> Result<String, ProviderError> {
        let output = self.agent.run_turn(prompt).await?;
        self.save().await.map_err(|err| ProviderError::Other(err))?;
        Ok(output)
    }

    pub async fn finish(&mut self, reason: EndReason) {
        self.status = match reason {
            EndReason::Completed | EndReason::UserExit => SessionStatus::Completed,
            EndReason::Error => SessionStatus::Error,
            EndReason::Cancelled => SessionStatus::Cancelled,
        };
        if let Some(tx) = self.agent.event_sender() {
            let _ = tx.send(AgentEvent::SessionEnded { reason }).await;
        }
        let _ = self.save().await;
    }

    pub async fn save(&self) -> Result<(), String> {
        let now = Utc::now();
        let session = SessionState {
            meta: SessionMeta {
                id: self.session_id.clone(),
                created_at: self.created_at,
                updated_at: now,
                workspace: self.workspace_root.clone(),
                model: self.model.clone(),
                status: self.status.clone(),
                pid: self.pid,
                socket_path: self.socket_path.clone(),
            },
            messages: self.agent.messages.clone(),
            total_input_tokens: self.agent.cost_tracker.input_tokens,
            total_output_tokens: self.agent.cost_tracker.output_tokens,
            estimated_cost_usd: self.agent.cost_tracker.estimated_cost_usd(),
        };
        self.session_store.save(&session).await.map_err(|e| e.to_string())
    }

    pub fn take_ipc_handle(&mut self) -> Option<IpcHandle> {
        self.ipc_handle.take()
    }
}

pub async fn build_session_runtime(
    mut config: NcaConfig,
    workspace_root: &Path,
    safe_mode: bool,
    interactive_approvals: bool,
    session_id: Option<String>,
) -> Result<SessionRuntime, ProviderError> {
    let workspace_root = workspace_root
        .canonicalize()
        .map_err(|err| ProviderError::Configuration(format!("invalid workspace root: {err}")))?;

    if safe_mode {
        config.permissions.deny.push("execute_bash".into());
    }

    let provider = build_provider(&config)?;
    let mut tools = if safe_mode {
        ToolRegistry::with_default_readonly_tools(workspace_root.clone(), config.web.clone())
    } else {
        ToolRegistry::with_default_full_tools(workspace_root.clone(), config.web.clone())
    };
    tools.register(Box::new(RuntimeBashTool::new(Arc::new(PtyManager::new(
        &workspace_root,
    )))));

    let approval = if interactive_approvals {
        ApprovalPolicy::new(config.permissions.clone()).with_handler(StdioApprovalHandler::new())
    } else {
        ApprovalPolicy::new(config.permissions.clone()).with_handler(AutoDenyApprovalHandler::new())
    };
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(256);
    let session_id = session_id.unwrap_or_else(generate_session_id);
    let session_store = SessionStore::new(workspace_root.join(&config.session.history_dir));
    let ipc_server = IpcServer::new(&session_id);
    let socket_path = ipc_server.socket_path();
    let ipc_handle = ipc_server
        .start()
        .await
        .map_err(|err| ProviderError::Other(err.to_string()))?;
    let _ = event_tx.try_send(AgentEvent::SessionStarted {
        session_id: session_id.clone(),
        workspace: workspace_root.clone(),
        model: config.model.default_model.clone(),
    });
    let created_at = Utc::now();
    let mut loop_runner = AgentLoop::new(
        provider,
        tools,
        approval,
        config.model.default_model.clone(),
        event_tx,
        config.session.max_turns_per_run,
        config.session.max_tool_calls_per_turn,
        config.session.checkpoint_interval,
    );
    let system_prompt = build_system_prompt(&config, &workspace_root);
    loop_runner.set_system_prompt(system_prompt);

    let runtime = SessionRuntime {
        agent: loop_runner,
        session_id,
        session_store,
        workspace_root,
        model: config.model.default_model,
        created_at,
        status: SessionStatus::Running,
        pid: Some(std::process::id()),
        socket_path: Some(socket_path),
        ipc_handle: Some(ipc_handle),
        event_rx: Some(event_rx),
    };
    runtime
        .save()
        .await
        .map_err(|err| ProviderError::Other(err.to_string()))?;
    Ok(runtime)
}

pub async fn build_resumed_session_runtime(
    config: NcaConfig,
    workspace_root: &Path,
    safe_mode: bool,
    interactive_approvals: bool,
    session_id: &str,
) -> Result<SessionRuntime, ProviderError> {
    let mut runtime =
        build_session_runtime(
            config.clone(),
            workspace_root,
            safe_mode,
            interactive_approvals,
            Some(session_id.into()),
        )
        .await?;
    let session_store = SessionStore::new(workspace_root.join(&config.session.history_dir));
    let loaded = session_store
        .load(session_id)
        .await
        .map_err(|err| ProviderError::Other(err.to_string()))?;

    runtime.session_id = loaded.meta.id.clone();
    runtime.workspace_root = loaded.meta.workspace.clone();
    runtime.model = loaded.meta.model.clone();
    runtime.created_at = loaded.meta.created_at;
    runtime.status = loaded.meta.status;
    runtime.pid = loaded.meta.pid;
    runtime.socket_path = loaded.meta.socket_path;
    runtime.agent.messages = loaded.messages;
    runtime.session_store = session_store;
    Ok(runtime)
}

fn generate_session_id() -> String {
    static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{}-{counter}", Utc::now().timestamp_micros())
}
