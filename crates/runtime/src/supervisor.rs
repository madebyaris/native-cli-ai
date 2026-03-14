use crate::ipc::{IpcHandle, IpcServer};
use crate::memory_store::{MemoryNote, MemoryStore};
use crate::pty::PtyManager;
use crate::session_store::SessionStore;
use chrono::Utc;
use nca_common::config::NcaConfig;
use nca_common::event::{AgentCommand, AgentEvent, EndReason, EventEnvelope};
use nca_common::session::{
    OrchestrationContext, SessionMeta, SessionSnapshot, SessionState, SessionStatus,
};
use nca_core::agent::AgentLoop;
use nca_core::approval::{ApprovalHandler, ApprovalPolicy};
use nca_core::harness::build_system_prompt;
use nca_core::hooks::{HookEventKind, HookRunner};
use nca_core::provider::ProviderError;
use nca_core::provider::factory::build_provider;
use nca_core::tools::mcp::load_mcp_tools;
use nca_core::tools::ToolRegistry;
use nca_core::tools::spawn_subagent::{SpawnRequest, SpawnSubagentTool};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

/// Reusable runtime supervisor that owns session lifecycle, IPC, event fanout,
/// and command handling. Both CLI and desktop app use this instead of managing
/// the lifecycle directly.
pub struct Supervisor {
    pub session_id: String,
    pub workspace_root: PathBuf,
    pub model: String,
    pub created_at: chrono::DateTime<Utc>,
    status: SessionStatus,
    pid: Option<u32>,
    socket_path: Option<PathBuf>,
    agent: AgentLoop,
    session_store: SessionStore,
    ipc_handle: Option<IpcHandle>,
    event_rx: Option<mpsc::Receiver<AgentEvent>>,
    approval_pending: Option<Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>>,
    spawn_rx: Option<mpsc::Receiver<SpawnRequest>>,
    worktree_path: Option<PathBuf>,
    branch: Option<String>,
    base_branch: Option<String>,
    parent_session_id: Option<String>,
    child_session_ids: Vec<String>,
    inherited_summary: Option<String>,
    spawn_reason: Option<String>,
    session_summary: Option<String>,
    orchestration: Option<OrchestrationContext>,
    config: NcaConfig,
    hooks: Option<HookRunner>,
}

/// Configuration for creating a new supervised session.
pub struct SupervisorConfig {
    pub config: NcaConfig,
    pub workspace_root: PathBuf,
    pub safe_mode: bool,
    pub interactive_approvals: bool,
    pub session_id: Option<String>,
    pub approval_handler: Option<Arc<dyn ApprovalHandler>>,
    pub orchestration_context: Option<OrchestrationContext>,
}

/// A handle returned to callers for interacting with a running supervisor.
/// The supervisor itself runs in a background task; this handle provides
/// the control surface.
pub struct SupervisorHandle {
    pub session_id: String,
    pub workspace_root: PathBuf,
    pub model: String,
    pub socket_path: Option<PathBuf>,
    pub event_log_path: PathBuf,
    event_rx: Option<mpsc::Receiver<AgentEvent>>,
    ipc_handle: Option<IpcHandle>,
    approval_pending: Option<Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>>,
    spawn_rx: Option<mpsc::Receiver<SpawnRequest>>,
}

impl SupervisorHandle {
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<AgentEvent>> {
        self.event_rx.take()
    }

    pub fn take_ipc_handle(&mut self) -> Option<IpcHandle> {
        self.ipc_handle.take()
    }

    pub fn take_approval_pending(
        &mut self,
    ) -> Option<Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>> {
        self.approval_pending.take()
    }

    pub fn take_spawn_rx(&mut self) -> Option<mpsc::Receiver<SpawnRequest>> {
        self.spawn_rx.take()
    }
}

impl Supervisor {
    /// Create a new supervised session. This sets up the agent loop, IPC server,
    /// event channels, and persists initial session metadata.
    pub async fn create(cfg: SupervisorConfig) -> Result<Self, ProviderError> {
        let workspace_root = cfg
            .workspace_root
            .canonicalize()
            .map_err(|e| ProviderError::Configuration(format!("invalid workspace root: {e}")))?;

        let mut config = cfg.config;
        if cfg.safe_mode {
            config.permissions.deny.push("execute_bash".into());
        }

        let provider = build_provider(&config)?;
        let mut tools = if cfg.safe_mode {
            ToolRegistry::with_default_readonly_tools(workspace_root.clone(), config.web.clone())
        } else {
            ToolRegistry::with_default_full_tools(workspace_root.clone(), config.web.clone())
        };
        if !config.mcp.servers.is_empty() && (!cfg.safe_mode || config.mcp.expose_in_safe_mode) {
            match load_mcp_tools(&workspace_root, &config.mcp.servers) {
                Ok(mcp_tools) => {
                    for tool in mcp_tools {
                        tools.register(tool);
                    }
                }
                Err(error) => tracing::warn!("failed to load MCP tools: {}", error),
            }
        }

        let pty = Arc::new(PtyManager::new(&workspace_root));
        tools.register(Box::new(crate::bash_tool::RuntimeBashTool::new(pty)));

        let (spawn_tx, spawn_rx) = mpsc::channel::<SpawnRequest>(16);
        if !cfg.safe_mode {
            tools.register(Box::new(SpawnSubagentTool::new(spawn_tx)));
        }

        let approval_pending: Option<Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>>;
        let approval = if cfg.interactive_approvals {
            match cfg.approval_handler {
                Some(handler) => {
                    approval_pending = None;
                    ApprovalPolicy::new(config.permissions.clone()).with_handler(handler)
                }
                None => {
                    let ipc_handler = IpcApprovalHandler::new();
                    approval_pending = Some(ipc_handler.pending());
                    ApprovalPolicy::new(config.permissions.clone())
                        .with_handler(ipc_handler as Arc<dyn ApprovalHandler>)
                }
            }
        } else {
            approval_pending = None;
            ApprovalPolicy::new(config.permissions.clone())
                .fail_on_ask()
                .with_handler(Arc::new(AutoDenyHandler) as Arc<dyn ApprovalHandler>)
        };

        let (event_tx, event_rx) = mpsc::channel(256);
        let session_id = cfg.session_id.unwrap_or_else(generate_session_id);
        let session_store = SessionStore::new(workspace_root.join(&config.session.history_dir));

        let ipc_server = IpcServer::new(&session_id);
        let socket_path = ipc_server.socket_path();
        let ipc_handle = ipc_server
            .start()
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let _ = event_tx.try_send(AgentEvent::SessionStarted {
            session_id: session_id.clone(),
            workspace: workspace_root.clone(),
            model: config.model.default_model.clone(),
        });

        let created_at = Utc::now();
        let hook_runner = {
            let runner = HookRunner::new(config.hooks.clone());
            runner.has_any().then_some(runner)
        };
        let mut agent = AgentLoop::new(
            provider,
            tools,
            approval,
            config.model.default_model.clone(),
            event_tx,
            config.session.max_turns_per_run,
            config.session.max_tool_calls_per_turn,
            config.session.checkpoint_interval,
            hook_runner.clone(),
        );
        let system_prompt =
            build_system_prompt(&config, &workspace_root, cfg.orchestration_context.as_ref());
        agent.set_system_prompt(system_prompt);

        let sup = Self {
            session_id,
            workspace_root,
            model: config.model.default_model.clone(),
            created_at,
            status: SessionStatus::Running,
            pid: Some(std::process::id()),
            socket_path: Some(socket_path),
            agent,
            session_store,
            ipc_handle: Some(ipc_handle),
            event_rx: Some(event_rx),
            approval_pending,
            spawn_rx: Some(spawn_rx),
            worktree_path: None,
            branch: None,
            base_branch: None,
            parent_session_id: None,
            child_session_ids: Vec::new(),
            inherited_summary: None,
            spawn_reason: None,
            session_summary: None,
            orchestration: cfg.orchestration_context,
            config,
            hooks: hook_runner,
        };
        sup.save().await.map_err(|e| ProviderError::Other(e))?;
        sup.run_session_hook(HookEventKind::SessionStart, json!(sup.snapshot()))
            .await;
        Ok(sup)
    }

    /// Resume an existing session by loading its state and creating a fresh
    /// IPC server + agent loop.
    pub async fn resume(
        config: NcaConfig,
        workspace_root: &Path,
        safe_mode: bool,
        interactive_approvals: bool,
        session_id: &str,
    ) -> Result<Self, ProviderError> {
        let mut sup = Self::create(SupervisorConfig {
            config: config.clone(),
            workspace_root: workspace_root.to_path_buf(),
            safe_mode,
            interactive_approvals,
            session_id: Some(session_id.into()),
            approval_handler: None,
            orchestration_context: None,
        })
        .await?;

        let store = SessionStore::new(workspace_root.join(&config.session.history_dir));
        let loaded = store
            .load(session_id)
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        sup.session_id = loaded.meta.id.clone();
        sup.workspace_root = loaded.meta.workspace.clone();
        sup.model = loaded.meta.model.clone();
        sup.agent.model = loaded.meta.model.clone();
        sup.created_at = loaded.meta.created_at;
        sup.status = loaded.meta.status;
        sup.pid = Some(std::process::id());
        sup.agent.messages = loaded.messages;
        sup.session_store = store;
        sup.worktree_path = loaded.meta.worktree_path;
        sup.branch = loaded.meta.branch;
        sup.base_branch = loaded.meta.base_branch;
        sup.parent_session_id = loaded.meta.parent_session_id;
        sup.child_session_ids = loaded.meta.child_session_ids;
        sup.inherited_summary = loaded.meta.inherited_summary;
        sup.spawn_reason = loaded.meta.spawn_reason;
        sup.session_summary = loaded.meta.session_summary;
        sup.orchestration = loaded.meta.orchestration;
        Ok(sup)
    }

    /// Extract a handle for the caller. The handle provides event_rx, ipc_handle,
    /// approval_pending, and spawn_rx for wiring into stream/command tasks.
    pub fn take_handle(&mut self) -> SupervisorHandle {
        SupervisorHandle {
            session_id: self.session_id.clone(),
            workspace_root: self.workspace_root.clone(),
            model: self.model.clone(),
            socket_path: self.socket_path.clone(),
            event_log_path: self.event_log_path(),
            event_rx: self.event_rx.take(),
            ipc_handle: self.ipc_handle.take(),
            approval_pending: self.approval_pending.take(),
            spawn_rx: self.spawn_rx.take(),
        }
    }

    pub fn event_log_path(&self) -> PathBuf {
        self.session_store
            .sessions_dir()
            .join(format!("{}.events.jsonl", self.session_id))
    }

    pub async fn run_turn(&mut self, prompt: &str) -> Result<String, ProviderError> {
        let output = self.agent.run_turn(prompt).await?;
        self.refresh_session_summary();
        self.save().await.map_err(|e| ProviderError::Other(e))?;
        Ok(output)
    }

    pub async fn finish(&mut self, reason: EndReason) {
        self.status = match reason {
            EndReason::Completed | EndReason::UserExit => SessionStatus::Completed,
            EndReason::Error => SessionStatus::Error,
            EndReason::Cancelled => SessionStatus::Cancelled,
        };
        if let Some(tx) = self.agent.event_sender() {
            let _ = tx
                .send(AgentEvent::SessionEnded {
                    reason: reason.clone(),
                })
                .await;
        }
        self.refresh_session_summary();
        if self.config.memory.auto_compact_on_finish {
            let _ = self.append_memory_note("session-summary", self.session_summary.clone()).await;
        }
        self.run_session_hook(
            HookEventKind::SessionEnd,
            json!({
                "reason": format!("{reason:?}"),
                "session": self.snapshot(),
            }),
        )
        .await;
        let _ = self.save().await;
    }

    pub async fn save(&self) -> Result<(), String> {
        let session = self.current_session_state(Utc::now());
        self.session_store
            .save(&session)
            .await
            .map_err(|e| e.to_string())
    }

    fn current_session_state(&self, updated_at: chrono::DateTime<Utc>) -> SessionState {
        SessionState {
            meta: SessionMeta {
                id: self.session_id.clone(),
                created_at: self.created_at,
                updated_at,
                workspace: self.workspace_root.clone(),
                model: self.model.clone(),
                status: self.status.clone(),
                pid: self.pid,
                socket_path: self.socket_path.clone(),
                worktree_path: self.worktree_path.clone(),
                branch: self.branch.clone(),
                base_branch: self.base_branch.clone(),
                parent_session_id: self.parent_session_id.clone(),
                child_session_ids: self.child_session_ids.clone(),
                inherited_summary: self.inherited_summary.clone(),
                spawn_reason: self.spawn_reason.clone(),
                session_summary: self.session_summary.clone(),
                orchestration: self.orchestration.clone(),
            },
            messages: self.agent.messages.clone(),
            total_input_tokens: self.agent.cost_tracker.input_tokens,
            total_output_tokens: self.agent.cost_tracker.output_tokens,
            estimated_cost_usd: self.agent.cost_tracker.estimated_cost_usd(),
        }
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        self.current_session_state(Utc::now()).snapshot()
    }

    pub fn compact_summary(&self) -> String {
        build_parent_summary(&self.agent.messages)
    }

    pub fn set_session_summary(&mut self, summary: Option<String>) {
        self.session_summary = summary.filter(|summary| !summary.trim().is_empty());
    }

    pub async fn append_memory_note(
        &self,
        kind: &str,
        content: Option<String>,
    ) -> Result<(), String> {
        let content = content
            .map(|content| content.trim().to_string())
            .filter(|content| !content.is_empty())
            .ok_or_else(|| "memory note content is empty".to_string())?;
        let store = MemoryStore::new(self.memory_store_path());
        let note = MemoryNote {
            id: format!("{}-{}", kind, Utc::now().timestamp_millis()),
            created_at: Utc::now(),
            kind: kind.to_string(),
            title: Some(self.session_id.clone()),
            content,
        };
        store
            .append_note(note, self.config.memory.max_notes)
            .await
            .map(|_| ())
    }

    pub fn memory_store_path(&self) -> PathBuf {
        if self.config.memory.file_path.is_absolute() {
            self.config.memory.file_path.clone()
        } else {
            self.workspace_root.join(&self.config.memory.file_path)
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn status(&self) -> &SessionStatus {
        &self.status
    }

    pub fn agent(&self) -> &AgentLoop {
        &self.agent
    }

    pub fn agent_mut(&mut self) -> &mut AgentLoop {
        &mut self.agent
    }

    pub fn request_cancel(&self) {
        self.agent.request_cancel();
    }

    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.agent.cancel_handle()
    }

    pub fn set_worktree_info(
        &mut self,
        worktree_path: PathBuf,
        branch: String,
        base_branch: String,
    ) {
        self.worktree_path = Some(worktree_path);
        self.branch = Some(branch);
        self.base_branch = Some(base_branch);
    }

    pub fn set_parent(
        &mut self,
        parent_id: String,
        summary: Option<String>,
        reason: Option<String>,
    ) {
        self.parent_session_id = Some(parent_id);
        self.inherited_summary = summary;
        self.spawn_reason = reason;
    }

    pub fn add_child(&mut self, child_id: String) {
        if !self.child_session_ids.contains(&child_id) {
            self.child_session_ids.push(child_id);
        }
    }

    pub fn event_tx(&self) -> Option<tokio::sync::mpsc::Sender<AgentEvent>> {
        self.agent.event_sender()
    }

    pub fn session_store(&self) -> &SessionStore {
        &self.session_store
    }

    fn refresh_session_summary(&mut self) {
        self.set_session_summary(Some(self.compact_summary()));
    }

    async fn run_session_hook(&self, event: HookEventKind, payload: serde_json::Value) {
        if let Some(hooks) = &self.hooks {
            hooks.run_best_effort(event, None, &payload).await;
        }
    }
}

/// Spawns the event fanout task: writes events to disk as `EventEnvelope`,
/// broadcasts over IPC, and renders to the provided callback.
pub fn spawn_event_fanout(
    mut event_rx: mpsc::Receiver<AgentEvent>,
    log_path: PathBuf,
    ipc_handle: Option<IpcHandle>,
    on_event: Option<Box<dyn Fn(&EventEnvelope) + Send>>,
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
            let envelope = EventEnvelope::new(event_id, event);
            if let Some(ref tx) = event_tx {
                let line = serde_json::to_string(&envelope).unwrap_or_default();
                let _ = tx.send(line);
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

/// Spawns a task that consumes IPC commands and resolves approvals/cancellation.
/// Also handles desktop-oriented commands like QueryState and ListSessions.
pub fn spawn_command_consumer(
    command_rx: mpsc::UnboundedReceiver<AgentCommand>,
    approval_pending: Option<Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>>,
    cancel_tx: Option<oneshot::Sender<()>>,
) -> tokio::task::JoinHandle<()> {
    spawn_command_consumer_with_store(
        command_rx,
        approval_pending,
        cancel_tx,
        None,
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

/// Extended command consumer that can also answer query commands using a session store.
pub fn spawn_command_consumer_with_store(
    mut command_rx: mpsc::UnboundedReceiver<AgentCommand>,
    approval_pending: Option<Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>>,
    cancel_tx: Option<oneshot::Sender<()>>,
    session_store: Option<SessionStore>,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    prompt_tx: Option<mpsc::UnboundedSender<String>>,
    control_tx: Option<mpsc::UnboundedSender<SessionControlCommand>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut cancel = cancel_tx;
        while let Some(cmd) = command_rx.recv().await {
            match cmd {
                AgentCommand::ApproveToolCall { call_id } => {
                    if let Some(ref p) = approval_pending {
                        if let Ok(mut m) = p.lock() {
                            if let Some(tx) = m.remove(&call_id) {
                                let _ = tx.send(true);
                            }
                        }
                    }
                }
                AgentCommand::DenyToolCall { call_id } => {
                    if let Some(ref p) = approval_pending {
                        if let Ok(mut m) = p.lock() {
                            if let Some(tx) = m.remove(&call_id) {
                                let _ = tx.send(false);
                            }
                        }
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
                AgentCommand::QueryState { session_id } => {
                    if let Some(ref store) = session_store {
                        match store.load(&session_id).await {
                            Ok(state) => {
                                let resp = nca_common::event::AgentResponse::SessionState {
                                    session: state,
                                };
                                if let Some(ref tx) = event_tx {
                                    let _ = tx.send(AgentEvent::Response { response: resp }).await;
                                }
                            }
                            Err(e) => {
                                if let Some(ref tx) = event_tx {
                                    let _ = tx
                                        .send(AgentEvent::Error {
                                            message: format!("QueryState failed: {e}"),
                                        })
                                        .await;
                                }
                            }
                        }
                    }
                }
                AgentCommand::ListSessions { workspace } => {
                    let store = SessionStore::new(workspace.join(".nca").join("sessions"));
                    match store.list().await {
                        Ok(ids) => {
                            let mut metas = Vec::new();
                            for id in &ids {
                                if let Ok(state) = store.load(id).await {
                                    metas.push(state.meta);
                                }
                            }
                            let resp =
                                nca_common::event::AgentResponse::SessionList { sessions: metas };
                            if let Some(ref tx) = event_tx {
                                let _ = tx.send(AgentEvent::Response { response: resp }).await;
                            }
                        }
                        Err(e) => {
                            if let Some(ref tx) = event_tx {
                                let _ = tx
                                    .send(AgentEvent::Error {
                                        message: format!("ListSessions failed: {e}"),
                                    })
                                    .await;
                            }
                        }
                    }
                }
                AgentCommand::StartSession { .. } | AgentCommand::ResumeSession { .. } => {
                    tracing::warn!(
                        "StartSession/ResumeSession commands should be handled by the desktop app, not the command consumer"
                    );
                }
            }
        }
    })
}

/// IPC-based approval handler that waits for approve/deny commands from
/// connected clients (monitor or CLI).
pub struct IpcApprovalHandler {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
}

impl IpcApprovalHandler {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn pending(&self) -> Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>> {
        self.pending.clone()
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for IpcApprovalHandler {
    async fn resolve(&self, call: &nca_common::tool::ToolCall, _description: &str) -> bool {
        let (tx, rx) = oneshot::channel();
        {
            let mut m = self.pending.lock().unwrap();
            m.insert(call.id.clone(), tx);
        }
        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                let mut m = self.pending.lock().unwrap();
                m.remove(&call.id);
                false
            }
        }
    }
}

/// Auto-deny handler for non-interactive sessions.
struct AutoDenyHandler;

#[async_trait::async_trait]
impl ApprovalHandler for AutoDenyHandler {
    async fn resolve(&self, _call: &nca_common::tool::ToolCall, _description: &str) -> bool {
        false
    }
}

fn generate_session_id() -> String {
    static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{}-{counter}", Utc::now().timestamp_micros())
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

        let pid_alive = session
            .meta
            .pid
            .map(|pid| is_pid_alive(pid))
            .unwrap_or(false);

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

/// Spawns a background task that consumes spawn requests from the sub-agent tool
/// and runs child sessions. Each child session inherits parent context.
pub fn spawn_subagent_consumer(
    mut spawn_rx: mpsc::Receiver<SpawnRequest>,
    parent_session_id: String,
    workspace_root: PathBuf,
    config: NcaConfig,
    parent_messages: Vec<nca_common::message::Message>,
    event_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let parent_sessions_dir = workspace_root.join(&config.session.history_dir);
        let parent_summary = build_parent_summary(&parent_messages);

        while let Some(req) = spawn_rx.recv().await {
            let parent_session_id = parent_session_id.clone();
            let workspace_root = workspace_root.clone();
            let config = config.clone();
            let event_tx = event_tx.clone();
            let parent_store = SessionStore::new(parent_sessions_dir.clone());
            let parent_summary = parent_summary.clone();

            let child_cfg = ChildSessionConfig {
                parent_session_id: parent_session_id.clone(),
                task: req.task.clone(),
                workspace_root: workspace_root.clone(),
                config,
                parent_summary,
                use_worktree: req.use_worktree,
                focus_files: req.focus_files,
            };

            tokio::spawn(async move {
                let hook_runner = {
                    let runner = HookRunner::new(child_cfg.config.hooks.clone());
                    runner.has_any().then_some(runner)
                };
                if let Some(hooks) = &hook_runner {
                    hooks
                        .run_best_effort(
                            HookEventKind::SubagentStart,
                            None,
                            &json!({
                                "parent_session_id": parent_session_id.clone(),
                                "task": child_cfg.task.clone(),
                                "workspace": child_cfg.workspace_root.clone(),
                            }),
                        )
                        .await;
                }
                let result = spawn_child_session(child_cfg, event_tx.clone()).await;
                match result {
                    Ok(res) => {
                        append_child_to_parent(
                            &parent_store,
                            &parent_session_id,
                            &res.child_session_id,
                        )
                        .await;

                        if let Some(ref tx) = event_tx {
                            let _ = tx
                                .send(AgentEvent::ChildSessionCompleted {
                                    parent_session_id: parent_session_id.clone(),
                                    child_session_id: res.child_session_id.clone(),
                                    status: res.status.clone(),
                                })
                                .await;
                        }
                        if let Some(hooks) = &hook_runner {
                            hooks
                                .run_best_effort(
                                    HookEventKind::SubagentStop,
                                    None,
                                    &json!({
                                        "parent_session_id": parent_session_id.clone(),
                                        "child_session_id": res.child_session_id.clone(),
                                        "status": res.status.clone(),
                                    }),
                                )
                                .await;
                        }
                        let response = nca_core::tools::spawn_subagent::SpawnResponse {
                            child_session_id: res.child_session_id,
                            status: res.status,
                            output: res.output,
                            workspace: res.workspace,
                            branch: res.branch,
                            worktree_path: res.worktree_path,
                        };
                        let _ = req.reply.send(response);
                    }
                    Err(e) => {
                        if let Some(hooks) = &hook_runner {
                            hooks
                                .run_best_effort(
                                    HookEventKind::SubagentStop,
                                    None,
                                    &json!({
                                        "parent_session_id": parent_session_id.clone(),
                                        "status": "error",
                                        "error": e.clone(),
                                    }),
                                )
                                .await;
                        }
                        if let Some(ref tx) = event_tx {
                            let _ = tx
                                .send(AgentEvent::Error {
                                    message: format!("Failed to spawn child session: {e}"),
                                })
                                .await;
                        }
                        let response = nca_core::tools::spawn_subagent::SpawnResponse {
                            child_session_id: String::new(),
                            status: "error".into(),
                            output: e,
                            workspace: workspace_root.display().to_string(),
                            branch: None,
                            worktree_path: None,
                        };
                        let _ = req.reply.send(response);
                    }
                }
            });
        }
    })
}

/// Append a child session ID to the parent session's metadata on disk.
async fn append_child_to_parent(store: &SessionStore, parent_id: &str, child_id: &str) {
    if let Ok(mut parent) = store.load(parent_id).await {
        if !parent
            .meta
            .child_session_ids
            .contains(&child_id.to_string())
        {
            parent.meta.child_session_ids.push(child_id.to_string());
            let _ = store.save(&parent).await;
        }
    }
}

/// Build a concise summary of the parent conversation for context inheritance.
fn build_parent_summary(messages: &[nca_common::message::Message]) -> String {
    use nca_common::message::Role;

    let mut summary = String::new();
    let recent: Vec<_> = messages
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant | Role::System))
        .collect();

    let window = if recent.len() > 10 {
        &recent[recent.len() - 10..]
    } else {
        &recent
    };

    for msg in window {
        let role = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
            Role::Tool => continue,
        };
        let content = if msg.content.len() > 500 {
            let truncated: String = msg.content.chars().take(500).collect();
            format!("{truncated}...")
        } else {
            msg.content.clone()
        };
        summary.push_str(&format!("[{role}]: {content}\n\n"));
    }

    summary
}

/// Configuration for spawning a child session.
pub struct ChildSessionConfig {
    pub parent_session_id: String,
    pub task: String,
    pub workspace_root: PathBuf,
    pub config: NcaConfig,
    pub parent_summary: String,
    pub use_worktree: bool,
    pub focus_files: Vec<String>,
}

/// Result of a spawned child session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChildSessionResult {
    pub child_session_id: String,
    pub status: String,
    pub output: String,
    pub workspace: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
}

/// Spawn a child session that inherits parent context and runs to completion.
/// Returns the result of the child run. This is a blocking async call.
pub async fn spawn_child_session(
    cfg: ChildSessionConfig,
    event_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<ChildSessionResult, String> {
    // Child sessions are non-interactive and already authorized by the parent
    // approval. Elevate to BypassPermissions so sub-agents can write files,
    // run tools, and spawn their own children without being auto-denied.
    let mut child_config = cfg.config.clone();
    child_config.permissions.mode = nca_common::config::PermissionMode::BypassPermissions;

    let mut sup = Supervisor::create(SupervisorConfig {
        config: child_config,
        workspace_root: cfg.workspace_root.clone(),
        safe_mode: false,
        interactive_approvals: false,
        session_id: None,
        approval_handler: Some(Arc::new(AutoDenyHandler) as Arc<dyn ApprovalHandler>),
        orchestration_context: None,
    })
    .await
    .map_err(|e| e.to_string())?;

    let child_id = sup.session_id.clone();

    sup.set_parent(
        cfg.parent_session_id.clone(),
        Some(cfg.parent_summary.clone()),
        Some(cfg.task.clone()),
    );

    if cfg.use_worktree {
        let wt_mgr = crate::worktree::WorktreeManager::new(&cfg.workspace_root);
        if wt_mgr.is_git_repo() {
            match wt_mgr.create_worktree(&child_id) {
                Ok(info) => {
                    sup.set_worktree_info(
                        info.worktree_path.clone(),
                        info.branch_name.clone(),
                        info.base_branch.clone(),
                    );
                    sup.workspace_root = info.worktree_path;
                }
                Err(e) => {
                    tracing::warn!("Failed to create worktree for child session: {e}");
                }
            }
        }
    }

    if let Some(ref tx) = event_tx {
        let _ = tx
            .send(AgentEvent::ChildSessionSpawned {
                parent_session_id: cfg.parent_session_id.clone(),
                child_session_id: child_id.clone(),
                task: cfg.task.clone(),
                workspace: sup.workspace_root.clone(),
                branch: sup.branch.clone(),
            })
            .await;
    }

    let mut context_prompt = format!(
        "You are a sub-agent spawned by a parent session to handle a specific task.\n\n\
         ## Parent Context\n{}\n\n\
         ## Your Task\n{}",
        cfg.parent_summary, cfg.task
    );

    if !cfg.focus_files.is_empty() {
        context_prompt.push_str("\n\n## Focus Files\n");
        for f in &cfg.focus_files {
            context_prompt.push_str(&format!("- {f}\n"));
        }
    }

    let mut handle = sup.take_handle();
    let event_rx = handle.take_event_rx();
    let log_path = handle.event_log_path.clone();

    let fanout = if let Some(rx) = event_rx {
        Some(spawn_event_fanout(rx, log_path, None, None))
    } else {
        None
    };

    let result = sup.run_turn(&context_prompt).await;

    let (status, output) = match result {
        Ok(text) => {
            sup.finish(EndReason::Completed).await;
            ("completed".to_string(), text)
        }
        Err(e) => {
            sup.finish(EndReason::Error).await;
            ("error".to_string(), e.to_string())
        }
    };

    if let Some(f) = fanout {
        f.abort();
    }

    let branch = sup.branch.clone();
    let wt_path = sup.worktree_path.clone().map(|p| p.display().to_string());

    Ok(ChildSessionResult {
        child_session_id: child_id,
        status,
        output,
        workspace: sup.workspace_root.display().to_string(),
        branch,
        worktree_path: wt_path,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nca_common::event::{AgentCommand, AgentEvent, AgentResponse};
    use nca_common::session::{SessionMeta, SessionState, SessionStatus};
    use tempfile::tempdir;

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
                content: "hello from monitor".into(),
            })
            .unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), prompt_rx.recv())
            .await
            .expect("prompt should be forwarded")
            .expect("prompt channel should remain open");
        assert_eq!(received, "hello from monitor");

        let _ = cmd_tx.send(AgentCommand::Shutdown);
        task.abort();
    }

    #[tokio::test]
    async fn query_state_emits_structured_response_event() {
        let tmp = tempdir().unwrap();
        let sessions_dir = tmp.path().join(".nca").join("sessions");
        let store = SessionStore::new(&sessions_dir);
        let now = Utc::now();
        let state = SessionState {
            meta: SessionMeta {
                id: "session-test".into(),
                created_at: now,
                updated_at: now,
                workspace: tmp.path().to_path_buf(),
                model: "MiniMax-M2.5".into(),
                status: SessionStatus::Running,
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
                orchestration: None,
            },
            messages: Vec::new(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            estimated_cost_usd: 0.0,
        };
        store.save(&state).await.unwrap();

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let task = spawn_command_consumer_with_store(
            cmd_rx,
            None,
            None,
            Some(store),
            Some(event_tx),
            None,
            None,
        );

        cmd_tx
            .send(AgentCommand::QueryState {
                session_id: "session-test".into(),
            })
            .unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("event should be emitted")
            .expect("event channel should remain open");

        match event {
            AgentEvent::Response { response } => match response {
                AgentResponse::SessionState { session } => {
                    assert_eq!(session.meta.id, "session-test");
                }
                other => panic!("unexpected response payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }

        let _ = cmd_tx.send(AgentCommand::Shutdown);
        task.abort();
    }
}
