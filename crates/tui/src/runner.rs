use crate::ipc_pending::{ApprovalPendingMap, QuestionPendingMap};
use nca_common::config::{NcaConfig, PermissionMode};
use nca_common::event::{AgentEvent, EndReason, QuestionSelection};
use nca_common::session::{OrchestrationContext, SessionSnapshot};
use nca_core::approval::{ApprovalHandler, ApprovalVerdict};
use nca_core::provider::ProviderError;
use nca_core::tools::spawn_subagent::SpawnRequest;
use nca_runtime::ipc::IpcHandle;
use nca_runtime::supervisor::{Supervisor, SupervisorConfig, SupervisorHandle};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc;

/// Resolve a pending `ask_question` without going through `SessionRuntime` (e.g. TUI side task
/// while `run_turn` is blocked waiting on the same question).
pub fn dispatch_question_answer(
    qp: &Option<QuestionPendingMap>,
    question_id: &str,
    selection: QuestionSelection,
) -> bool {
    let Some(qp) = qp else {
        return false;
    };
    let Ok(mut m) = qp.lock() else {
        return false;
    };
    let Some(tx) = m.remove(question_id) else {
        return false;
    };
    tx.send(selection).is_ok()
}

/// Resolve a pending approval without going through the main command loop.
pub fn dispatch_tool_approval(
    approvals: &Option<ApprovalPendingMap>,
    call_id: &str,
    verdict: ApprovalVerdict,
) -> bool {
    let Some(approvals) = approvals else {
        return false;
    };
    let Ok(mut map) = approvals.lock() else {
        return false;
    };
    let Some(tx) = map.remove(call_id) else {
        return false;
    };
    tx.send(verdict).is_ok()
}

/// Thin CLI wrapper around the runtime `Supervisor`.
/// Keeps the same public API so existing CLI code (repl, main) works unchanged.
pub struct SessionRuntime {
    supervisor: Supervisor,
    handle: Option<SupervisorHandle>,
    question_pending: Option<QuestionPendingMap>,
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

    pub async fn run_turn_with_images(
        &mut self,
        prompt: &str,
        attachments: Vec<nca_common::message::ImageAttachment>,
    ) -> Result<String, ProviderError> {
        self.supervisor
            .run_turn_with_images(prompt, &attachments)
            .await
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

    pub fn take_ipc_approval_pending(&mut self) -> Option<ApprovalPendingMap> {
        self.handle.as_mut()?.take_approval_pending()
    }

    /// Pending `ask_question` resolvers (same map the runtime tool waits on).
    pub fn question_pending(&self) -> Option<QuestionPendingMap> {
        self.question_pending.clone()
    }

    /// Submit an answer for the current interactive question (TUI / REPL).
    pub fn submit_question_answer(&self, question_id: &str, selection: QuestionSelection) -> bool {
        dispatch_question_answer(&self.question_pending, question_id, selection)
    }

    /// Accept the model's suggested answer when exactly one question is pending.
    pub fn submit_suggested_answer(&self) -> bool {
        let Some(ref qp) = self.question_pending else {
            return false;
        };
        let Ok(mut m) = qp.lock() else {
            return false;
        };
        let keys: Vec<String> = m.keys().cloned().collect();
        if keys.len() != 1 {
            return false;
        }
        let id = keys[0].clone();
        let Some(tx) = m.remove(&id) else {
            return false;
        };
        tx.send(QuestionSelection::Suggested).is_ok()
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

    pub fn request_cancel(&self) {
        self.supervisor.request_cancel();
    }

    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.supervisor.cancel_handle()
    }

    pub fn event_tx(&self) -> Option<tokio::sync::mpsc::Sender<AgentEvent>> {
        self.supervisor.event_tx()
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

    pub fn config_mut(&mut self) -> &mut NcaConfig {
        &mut self.config
    }

    /// Replace merged config and rebuild the provider (fails if API key missing, etc.).
    pub fn apply_nca_config(&mut self, config: NcaConfig) -> Result<(), ProviderError> {
        self.supervisor.apply_nca_config(config.clone())?;
        self.config = config;
        Ok(())
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

    /// Start a fresh session: save the current one, generate a new ID, clear messages.
    pub async fn new_session(&mut self) -> Result<(), String> {
        self.supervisor.finish(EndReason::Completed).await;
        self.supervisor.save().await?;
        self.supervisor.reset_for_new_session();
        Ok(())
    }
}

pub async fn build_session_runtime(
    config: NcaConfig,
    workspace_root: &Path,
    safe_mode: bool,
    interactive_approvals: bool,
    session_id: Option<String>,
    ipc_approval_handler: Option<Arc<dyn ApprovalHandler>>,
    orchestration_context: Option<OrchestrationContext>,
) -> Result<SessionRuntime, ProviderError> {
    let approval_handler = ipc_approval_handler;

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

    let mut handle = supervisor.take_handle();
    let question_pending = handle.take_question_pending();
    Ok(SessionRuntime {
        supervisor,
        handle: Some(handle),
        question_pending,
        config,
    })
}

pub async fn build_resumed_session_runtime(
    config: NcaConfig,
    workspace_root: &Path,
    safe_mode: bool,
    interactive_approvals: bool,
    session_id: &str,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
) -> Result<SessionRuntime, ProviderError> {
    let mut supervisor = Supervisor::resume(
        config.clone(),
        workspace_root,
        safe_mode,
        interactive_approvals,
        session_id,
        approval_handler,
    )
    .await?;
    let mut handle = supervisor.take_handle();
    let question_pending = handle.take_question_pending();
    Ok(SessionRuntime {
        supervisor,
        handle: Some(handle),
        question_pending,
        config,
    })
}
