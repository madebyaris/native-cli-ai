use crate::approval_prompt::IpcApprovalHandler;
use nca_common::config::NcaConfig;
use nca_common::event::{AgentEvent, EndReason};
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

    pub fn config(&self) -> &NcaConfig {
        // We need access to config for child spawning; expose through supervisor
        // For now, return a reference through the supervisor's stored config
        // Actually the config isn't stored on the supervisor. We'll store it on SessionRuntime.
        &self.config
    }
}

pub async fn build_session_runtime(
    config: NcaConfig,
    workspace_root: &Path,
    safe_mode: bool,
    interactive_approvals: bool,
    session_id: Option<String>,
    ipc_approval_handler: Option<Arc<IpcApprovalHandler>>,
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
