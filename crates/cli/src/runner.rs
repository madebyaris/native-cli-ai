use crate::approval_prompt::IpcApprovalHandler;
use nca_common::config::{NcaConfig, PermissionMode};
use nca_common::event::{AgentEvent, EndReason};
use nca_common::session::{OrchestrationContext, SessionSnapshot};
use nca_core::approval::ApprovalHandler;
use nca_core::provider::ProviderError;
use nca_core::tools::spawn_subagent::SpawnRequest;
use nca_runtime::ipc::IpcHandle;
use nca_runtime::supervisor::{Supervisor, SupervisorConfig, SupervisorHandle};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

/// Thin CLI wrapper around the runtime `Supervisor`.
/// Keeps the same public API so existing CLI code (repl, main) works unchanged.
pub struct SessionRuntime {
    supervisor: Supervisor,
    handle: Option<SupervisorHandle>,
    config: NcaConfig,
}

impl SessionRuntime {
    pub fn take_event_rx(&mut self) -> Option<tokio::sync::mpsc::Receiver<AgentEvent>> {
        self.handle.as_mut()?.take_event_rx()
    }

    pub fn event_log_path(&self) -> std::path::PathBuf {
        self.supervisor.event_log_path()
    }

    pub async fn run_turn(&mut self, prompt: &str) -> Result<String, ProviderError> {
        self.supervisor.run_turn(prompt).await
    }

    pub async fn finish(&mut self, reason: EndReason) {
        self.supervisor.finish(reason).await;
    }

    pub async fn save(&self) -> Result<(), String> {
        self.supervisor.save().await
    }

    pub fn take_ipc_handle(&mut self) -> Option<IpcHandle> {
        self.handle.as_mut()?.take_ipc_handle()
    }

    pub fn take_ipc_approval_pending(
        &mut self,
    ) -> Option<Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>> {
        self.handle.as_mut()?.take_approval_pending()
    }

    pub fn session_id(&self) -> &str {
        self.supervisor.session_id()
    }

    pub fn model(&self) -> &str {
        &self.supervisor.model
    }

    pub fn workspace_root(&self) -> &std::path::Path {
        &self.supervisor.workspace_root
    }

    pub fn take_spawn_rx(&mut self) -> Option<mpsc::Receiver<SpawnRequest>> {
        self.handle.as_mut()?.take_spawn_rx()
    }

    pub fn messages(&self) -> &[nca_common::message::Message] {
        &self.supervisor.agent().messages
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.supervisor.model = model.clone();
        self.supervisor.agent_mut().model = model;
    }

    pub fn permission_mode(&self) -> PermissionMode {
        self.supervisor.agent().approval.mode()
    }

    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.supervisor.agent_mut().approval.set_mode(mode);
    }

    pub async fn list_session_ids(&self) -> Result<Vec<String>, String> {
        let store = nca_runtime::session_store::SessionStore::new(
            self.workspace_root().join(&self.config.session.history_dir),
        );
        store.list().await.map_err(|err| err.to_string())
    }

    pub fn config(&self) -> &NcaConfig {
        &self.config
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        self.supervisor.snapshot()
    }

    pub fn compact_summary(&self) -> String {
        self.supervisor.compact_summary()
    }

    pub fn set_session_summary(&mut self, summary: Option<String>) {
        self.supervisor.set_session_summary(summary);
    }

    pub async fn append_memory_note(
        &self,
        kind: &str,
        content: Option<String>,
    ) -> Result<(), String> {
        self.supervisor.append_memory_note(kind, content).await
    }

    pub fn memory_store_path(&self) -> std::path::PathBuf {
        self.supervisor.memory_store_path()
    }
}

pub async fn build_session_runtime(
    config: NcaConfig,
    workspace_root: &Path,
    safe_mode: bool,
    interactive_approvals: bool,
    session_id: Option<String>,
    ipc_approval_handler: Option<Arc<IpcApprovalHandler>>,
    orchestration_context: Option<OrchestrationContext>,
) -> Result<SessionRuntime, ProviderError> {
    let approval_handler: Option<Arc<dyn ApprovalHandler>> =
        ipc_approval_handler.map(|h| h as Arc<dyn ApprovalHandler>);

    let mut supervisor = Supervisor::create(SupervisorConfig {
        config: config.clone(),
        workspace_root: workspace_root.to_path_buf(),
        safe_mode,
        interactive_approvals,
        session_id,
        approval_handler,
        orchestration_context,
    })
    .await?;

    let handle = supervisor.take_handle();
    Ok(SessionRuntime {
        supervisor,
        handle: Some(handle),
        config,
    })
}

pub async fn build_resumed_session_runtime(
    config: NcaConfig,
    workspace_root: &Path,
    safe_mode: bool,
    interactive_approvals: bool,
    session_id: &str,
) -> Result<SessionRuntime, ProviderError> {
    let mut supervisor = Supervisor::resume(
        config.clone(),
        workspace_root,
        safe_mode,
        interactive_approvals,
        session_id,
    )
    .await?;
    let handle = supervisor.take_handle();
    Ok(SessionRuntime {
        supervisor,
        handle: Some(handle),
        config,
    })
}
