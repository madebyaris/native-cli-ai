use crate::context_manager::{ContextManager, ContextManagerConfig, ContextStats};
use crate::ipc::{IpcHandle, IpcServer};
use crate::last_session::LastSessionStore;
use crate::memory_store::{MemoryNote, MemoryStore};
use crate::model_limits_api;
use crate::pty::PtyManager;
use crate::session_store::SessionStore;
use chrono::Utc;
use nca_common::config::NcaConfig;
use nca_common::event::{AgentEvent, EndReason, QuestionSelection};
use nca_common::session::{
    OrchestrationContext, SESSION_STATE_SCHEMA_VERSION, SessionMeta, SessionSnapshot, SessionState,
    SessionStatus,
};
use nca_core::agent::AgentLoop;
use nca_core::approval::{ApprovalHandler, ApprovalPolicy, ApprovalVerdict};
use nca_core::harness::build_system_prompt;
use nca_core::hooks::{HookEventKind, HookRunner};
use nca_core::provider::ProviderError;
use nca_core::provider::factory::build_provider;
use nca_core::tools::AskQuestionTool;
use nca_core::tools::InvokeSkillTool;
use nca_core::tools::RecentSkillHints;
use nca_core::tools::TodoWriteTool;
use nca_core::tools::ToolRegistry;
use nca_core::tools::mcp::load_mcp_tools;
use nca_core::tools::spawn_subagent::{SpawnRequest, SpawnSubagentTool};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

mod approvals;
mod commands;
mod fanout;
mod spawn;

use approvals::AutoDenyHandler;
pub use approvals::IpcApprovalHandler;
pub use commands::{
    SessionControlCommand, cleanup_stale_sessions, get_last_session_id, is_pid_alive,
    list_sessions, query_session_state, spawn_command_consumer, spawn_command_consumer_with_store,
};
pub use fanout::{EventFanoutCallback, spawn_event_fanout};
use spawn::build_parent_summary;
pub use spawn::{
    ChildSessionConfig, ChildSessionResult, spawn_child_session, spawn_subagent_consumer,
};

pub type ApprovalPendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalVerdict>>>>;
pub type QuestionPendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<QuestionSelection>>>>;

/// Reusable runtime supervisor that owns session lifecycle, IPC, event fanout,
/// and command handling.
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
    approval_pending: Option<ApprovalPendingMap>,
    question_pending: Option<QuestionPendingMap>,
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
    context_manager: ContextManager,
    last_summary_at_tokens: usize,
}

/// Configuration for creating a new supervised session.
pub struct SupervisorConfig {
    pub config: NcaConfig,
    pub workspace_root: PathBuf,
    pub safe_mode: bool,
    pub interactive_approvals: bool,
    pub session_id: Option<String>,
    pub approval_handler: Option<Arc<dyn ApprovalHandler>>,
    /// When set, used instead of deriving approval pending from the default IPC handler.
    pub approval_pending: Option<ApprovalPendingMap>,
    pub orchestration_context: Option<OrchestrationContext>,
    /// When set, applied before the initial persist so resume does not overwrite
    /// an existing session snapshot with empty conversation state.
    pub preloaded_state: Option<SessionState>,
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
    approval_pending: Option<ApprovalPendingMap>,
    question_pending: Option<QuestionPendingMap>,
    spawn_rx: Option<mpsc::Receiver<SpawnRequest>>,
}

impl SupervisorHandle {
    /// Take ownership of the agent event receiver (consumed once).
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<AgentEvent>> {
        self.event_rx.take()
    }

    /// Take ownership of the IPC handle for wiring command consumers.
    pub fn take_ipc_handle(&mut self) -> Option<IpcHandle> {
        self.ipc_handle.take()
    }

    /// Take the approval pending map used to resolve tool approvals over IPC.
    pub fn take_approval_pending(&mut self) -> Option<ApprovalPendingMap> {
        self.approval_pending.take()
    }

    /// Take the question pending map used to resolve interactive questions.
    pub fn take_question_pending(&mut self) -> Option<QuestionPendingMap> {
        self.question_pending.take()
    }

    /// Take the sub-agent spawn request receiver.
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

        let (spawn_tx, spawn_rx) = mpsc::channel::<SpawnRequest>(16);
        let recent_skills = RecentSkillHints::default();
        if !cfg.safe_mode {
            tools.register(Box::new(SpawnSubagentTool::new(
                spawn_tx,
                recent_skills.clone(),
            )));
        }

        let approval_pending: Option<ApprovalPendingMap>;
        let approval = if cfg.interactive_approvals {
            match cfg.approval_handler {
                Some(handler) => {
                    approval_pending = cfg.approval_pending.clone();
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

        if config.session.stream_bash_output {
            tools.register(Box::new(crate::bash_tool::RuntimeBashTool::with_streaming(
                pty.clone(),
                event_tx.clone(),
            )));
        } else {
            tools.register(Box::new(crate::bash_tool::RuntimeBashTool::new(
                pty.clone(),
            )));
        }

        let question_pending = Arc::new(Mutex::new(HashMap::new()));
        tools.register(Box::new(AskQuestionTool::new(
            event_tx.clone(),
            question_pending.clone(),
        )));
        tools.register(Box::new(InvokeSkillTool::new(
            workspace_root.clone(),
            config.harness.skill_directories.clone(),
            recent_skills,
        )));
        let session_id = cfg.session_id.unwrap_or_else(generate_session_id);
        let session_store = SessionStore::new(workspace_root.join(&config.session.history_dir));

        tools.register(Box::new(TodoWriteTool::new(
            event_tx.clone(),
            session_store.sessions_dir().to_path_buf(),
            session_id.clone(),
        )));

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
        let model_name = config.model.resolve_alias(&config.model.default_model);
        let pricing = config.model.pricing_for(&model_name);
        let mut agent = AgentLoop::new(
            provider,
            tools,
            approval,
            config.model.default_model.clone(),
            event_tx.clone(),
            config.session.max_turns_per_run,
            config.session.max_tool_calls_per_turn,
            config.session.checkpoint_interval,
            hook_runner.clone(),
            pricing,
            config.model.retry.clone(),
        );
        let system_prompt =
            build_system_prompt(&config, &workspace_root, cfg.orchestration_context.as_ref());
        agent.set_system_prompt(system_prompt);

        let context_manager =
            Self::make_context_manager(&config, &config.model.default_model).await;

        let mut sup = Self {
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
            question_pending: Some(question_pending),
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
            context_manager,
            last_summary_at_tokens: 0,
        };
        if let Some(loaded) = cfg.preloaded_state {
            sup.apply_loaded_state(loaded).await;
        }
        sup.save().await.map_err(ProviderError::Other)?;
        sup.update_last_session()
            .await
            .map_err(ProviderError::Other)?;
        sup.run_session_hook(HookEventKind::SessionStart, json!(sup.snapshot()))
            .await;
        Ok(sup)
    }

    /// Resume an existing session by loading persisted state first, then
    /// starting a fresh IPC server and agent loop with the restored messages.
    pub async fn resume(
        config: NcaConfig,
        workspace_root: &Path,
        safe_mode: bool,
        interactive_approvals: bool,
        session_id: &str,
        approval_handler: Option<Arc<dyn ApprovalHandler>>,
        approval_pending: Option<ApprovalPendingMap>,
    ) -> Result<Self, ProviderError> {
        let store = SessionStore::new(workspace_root.join(&config.session.history_dir));
        let loaded = store
            .load(session_id)
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        Self::create(SupervisorConfig {
            config,
            workspace_root: workspace_root.to_path_buf(),
            safe_mode,
            interactive_approvals,
            session_id: Some(loaded.meta.id.clone()),
            approval_handler,
            approval_pending,
            orchestration_context: loaded.meta.orchestration.clone(),
            preloaded_state: Some(loaded),
        })
        .await
    }

    async fn apply_loaded_state(&mut self, loaded: SessionState) {
        self.session_id = loaded.meta.id;
        self.workspace_root = loaded.meta.workspace;
        self.model = loaded.meta.model.clone();
        self.agent.model = loaded.meta.model;
        self.created_at = loaded.meta.created_at;
        self.status = loaded.meta.status;
        self.pid = Some(std::process::id());
        self.agent.messages = loaded.messages;
        self.agent.cost_tracker.input_tokens = loaded.total_input_tokens;
        self.agent.cost_tracker.output_tokens = loaded.total_output_tokens;
        self.worktree_path = loaded.meta.worktree_path;
        self.branch = loaded.meta.branch;
        self.base_branch = loaded.meta.base_branch;
        self.parent_session_id = loaded.meta.parent_session_id;
        self.child_session_ids = loaded.meta.child_session_ids;
        self.inherited_summary = loaded.meta.inherited_summary;
        self.spawn_reason = loaded.meta.spawn_reason;
        self.session_summary = loaded.meta.session_summary;
        self.orchestration = loaded.meta.orchestration;
        self.context_manager = Self::make_context_manager(&self.config, &self.model).await;
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
            question_pending: self.question_pending.take(),
            spawn_rx: self.spawn_rx.take(),
        }
    }

    pub fn event_log_path(&self) -> PathBuf {
        self.session_store
            .sessions_dir()
            .join(format!("{}.events.jsonl", self.session_id))
    }

    pub async fn run_turn(&mut self, prompt: &str) -> Result<String, ProviderError> {
        self.run_turn_with_images(prompt, &[]).await
    }

    /// Like [`run_turn`], but attaches on-disk images (paths relative to workspace) for vision models.
    pub async fn run_turn_with_images(
        &mut self,
        prompt: &str,
        attachments: &[nca_common::message::ImageAttachment],
    ) -> Result<String, ProviderError> {
        if !attachments.is_empty()
            && !nca_common::model_caps::model_accepts_native_images(
                self.config.provider.default,
                self.model.as_str(),
            )
        {
            return Err(ProviderError::Configuration(format!(
                "native images are not supported for provider {} with model `{}` (pick a vision-capable model or remove image attachments)",
                self.config.provider.default.display_name(),
                self.model
            )));
        }

        // Check context before running turn
        self.maybe_compact_context().await;

        let output = self
            .agent
            .run_turn(prompt, self.workspace_root.as_path(), attachments)
            .await?;

        // Check context after turn
        self.check_and_summarize_context().await;

        self.refresh_session_summary();
        self.save().await.map_err(ProviderError::Other)?;
        self.update_last_session()
            .await
            .map_err(ProviderError::Other)?;
        Ok(output)
    }

    /// Get current context statistics with model info.
    pub fn context_stats(&self) -> ContextStats {
        self.context_manager.stats(&self.agent.messages)
    }

    async fn make_context_manager(config: &NcaConfig, model: &str) -> ContextManager {
        let model_limits = model_limits_api::resolve_model_limits(config, model).await;
        let context_window = if config.memory.context.auto_detect_context_window {
            tracing::info!(
                "Context window target for {}: {} tokens",
                model,
                model_limits.context_window
            );
            model_limits.context_window
        } else {
            config.memory.context.context_window_target
        };

        let context_config = ContextManagerConfig {
            context_window_target: context_window,
            max_retained_messages: config.memory.context.max_retained_messages,
            auto_summarize_threshold: config.memory.context.auto_summarize_threshold,
            enable_auto_summarize: config.memory.context.enable_auto_summarize,
            max_message_chars_for_summary: 10000,
        };
        ContextManager::new(context_config, model.to_string())
    }

    /// Check if context needs attention or summarization.
    async fn maybe_compact_context(&mut self) {
        if !self.context_manager.config().enable_auto_summarize {
            return;
        }

        let stats = self.context_manager.stats(&self.agent.messages);
        if stats.needs_attention
            && let Some(tx) = self.agent.event_sender()
        {
            let _ = tx
                .send(AgentEvent::ContextWarning {
                    message: format!(
                        "Context window at {}% ({} tokens). Consider summarizing.",
                        stats.usage_percent, stats.estimated_tokens
                    ),
                })
                .await;
        }
    }

    /// Check if context should be summarized and trigger if needed.
    async fn check_and_summarize_context(&mut self) {
        if !self.context_manager.config().enable_auto_summarize {
            return;
        }

        let stats = self.context_manager.stats(&self.agent.messages);

        // Don't summarize if we just summarized
        if self.last_summary_at_tokens > 0 && stats.estimated_tokens < self.last_summary_at_tokens {
            // Context was reduced, reset the flag
            self.last_summary_at_tokens = 0;
        }

        if stats.should_summarize && self.last_summary_at_tokens == 0 {
            // Emit event that summarization is starting
            if let Some(tx) = self.agent.event_sender() {
                let _ = tx
                    .send(AgentEvent::ContextCompaction {
                        phase: "starting".to_string(),
                        message: format!(
                            "Auto-summarizing context ({}% full, {} tokens)",
                            stats.usage_percent, stats.estimated_tokens
                        ),
                    })
                    .await;
            }

            // Trigger summarization
            if let Err(e) = self.perform_auto_summarize().await {
                tracing::error!("Auto-summarize failed: {}", e);
                // Reset so we can try again
                self.last_summary_at_tokens = 0;
            }
        }
    }

    /// Perform the actual auto-summarization.
    async fn perform_auto_summarize(&mut self) -> Result<(), String> {
        let messages_to_summarize = self
            .context_manager
            .get_messages_to_summarize(&self.agent.messages);

        if messages_to_summarize.is_empty() {
            // Nothing to summarize, use sliding window instead
            let compacted = self
                .context_manager
                .get_sliding_window(&self.agent.messages, None);
            self.agent.messages = compacted;
            return Ok(());
        }

        // Generate summary prompt
        let summary_prompt = self.context_manager.summary_prompt(&messages_to_summarize);

        // Try to use the AI to summarize. If the provider supports a quick call,
        // we can use it. Otherwise, fall back to extractive summarization.
        match self.summarize_with_ai(&summary_prompt).await {
            Ok(summary) => {
                // Apply the summary
                self.agent.messages = self
                    .context_manager
                    .apply_summary(&self.agent.messages, &summary);
                self.last_summary_at_tokens = self
                    .context_manager
                    .stats(&self.agent.messages)
                    .estimated_tokens;

                if let Some(tx) = self.agent.event_sender() {
                    let _ = tx
                        .send(AgentEvent::ContextCompaction {
                            phase: "completed".to_string(),
                            message: format!(
                                "Context summarized. Reduced from {} to ~{} tokens.",
                                messages_to_summarize.len() * 100, // rough estimate
                                self.last_summary_at_tokens
                            ),
                        })
                        .await;
                }
            }
            Err(e) => {
                // Fallback: just use sliding window
                tracing::warn!("AI summarization failed, using sliding window: {}", e);
                let compacted = self
                    .context_manager
                    .get_sliding_window(&self.agent.messages, None);
                self.agent.messages = compacted;
                self.last_summary_at_tokens = self
                    .context_manager
                    .stats(&self.agent.messages)
                    .estimated_tokens;
            }
        }

        Ok(())
    }

    /// Use AI to generate a summary of the conversation.
    async fn summarize_with_ai(&self, prompt: &str) -> Result<String, String> {
        use nca_common::message::Message;

        let messages = vec![Message::user(prompt)];

        let mut stream = self
            .agent
            .provider
            .chat(&messages, &[], &self.model, self.workspace_root.as_path())
            .await
            .map_err(|e| e.to_string())?;

        // Collect the response
        let mut summary = String::new();
        while let Some(chunk) = stream.recv().await {
            match chunk {
                nca_core::provider::StreamChunk::TextDelta(delta) => {
                    summary.push_str(&delta);
                }
                nca_core::provider::StreamChunk::Done => break,
                _ => {}
            }
        }

        Ok(summary.trim().to_string())
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
            let _ = self
                .append_memory_note("session-summary", self.session_summary.clone())
                .await;
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
        // Always update last session on finish so stale pointers are avoided.
        let _ = self.update_last_session().await;
    }

    pub async fn save(&self) -> Result<(), String> {
        let session = self.current_session_state(Utc::now());
        self.session_store
            .save(&session)
            .await
            .map_err(|e| e.to_string())
    }

    /// Mark this session as the last active session for the workspace.
    /// Called on create, resume, run_turn, and finish to keep the pointer fresh.
    pub async fn update_last_session(&self) -> Result<(), String> {
        let store = LastSessionStore::new(
            self.workspace_root
                .join(&self.config.session.last_session_file),
        );
        store
            .save(&self.session_id)
            .await
            .map_err(|e| e.to_string())
    }

    fn current_session_state(&self, updated_at: chrono::DateTime<Utc>) -> SessionState {
        SessionState {
            schema_version: SESSION_STATE_SCHEMA_VERSION,
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

    /// Reset for a fresh session: new ID, rebuild system prompt, clear lineage and cost.
    pub fn reset_for_new_session(&mut self) {
        self.session_id = generate_session_id();
        self.agent.messages.clear();
        let system_prompt = build_system_prompt(
            &self.config,
            &self.workspace_root,
            self.orchestration.as_ref(),
        );
        self.agent.set_system_prompt(system_prompt);
        self.child_session_ids.clear();
        self.parent_session_id = None;
        self.inherited_summary = None;
        self.spawn_reason = None;
        self.session_summary = None;
        self.agent.cost_tracker = Default::default();
        self.status = SessionStatus::Running;
        self.created_at = Utc::now();
        self.last_summary_at_tokens = 0;
        self.session_store =
            SessionStore::new(self.workspace_root.join(&self.config.session.history_dir));
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

    /// Apply a new [`NcaConfig`] and rebuild the active LLM provider (in-session provider switch).
    pub fn apply_nca_config(&mut self, config: NcaConfig) -> Result<(), ProviderError> {
        let provider = build_provider(&config)?;
        self.config = config;
        self.model = self.config.provider.active_model().to_string();
        let resolved = self.config.model.resolve_alias(&self.model);
        let pricing = self.config.model.pricing_for(&resolved);
        let retry = self.config.model.retry.clone();
        let m = self.model.clone();
        let agent = self.agent_mut();
        agent.model = m;
        agent.replace_provider(provider);
        agent.set_pricing(pricing);
        agent.set_retry(retry);
        self.rebuild_context_manager_sync();
        Ok(())
    }

    /// Rebuild context_manager from current config (sync, uses configured window target).
    fn rebuild_context_manager_sync(&mut self) {
        let ctx = &self.config.memory.context;
        let window = if ctx.context_window_target > 0 {
            ctx.context_window_target
        } else {
            128_000
        };
        let context_config = ContextManagerConfig {
            context_window_target: window,
            max_retained_messages: ctx.max_retained_messages,
            auto_summarize_threshold: ctx.auto_summarize_threshold,
            enable_auto_summarize: ctx.enable_auto_summarize,
            max_message_chars_for_summary: 10000,
        };
        self.context_manager = ContextManager::new(context_config, self.model.clone());
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

fn generate_session_id() -> String {
    static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{}-{counter}", Utc::now().timestamp_micros())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use nca_common::event::AgentCommand;
    use nca_common::message::Message;
    use nca_common::session::{SessionMeta, SessionState, SessionStatus};
    use std::fs;

    fn write_session_for_test(
        workspace: &std::path::Path,
        id: &str,
        updated_at: chrono::DateTime<Utc>,
        model: &str,
        status: SessionStatus,
    ) {
        let sessions_dir = workspace.join(".nca").join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");

        let session = SessionState {
            schema_version: nca_common::session::SESSION_STATE_SCHEMA_VERSION,
            meta: SessionMeta {
                id: id.to_string(),
                created_at: updated_at - Duration::minutes(1),
                updated_at,
                workspace: workspace.to_path_buf(),
                model: model.to_string(),
                status,
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
            messages: vec![Message::user("hello")],
            total_input_tokens: 0,
            total_output_tokens: 0,
            estimated_cost_usd: 0.0,
        };

        let json = serde_json::to_string_pretty(&session).expect("serialize session");
        fs::write(sessions_dir.join(format!("{id}.json")), json).expect("write session");
    }

    #[tokio::test]
    async fn get_last_session_id_falls_back_to_most_recent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path();
        let now = Utc::now();

        // Write sessions WITHOUT .last_session file
        write_session_for_test(
            workspace,
            "session-oldest",
            now - Duration::minutes(10),
            "MiniMax-M2.5",
            SessionStatus::Completed,
        );
        write_session_for_test(
            workspace,
            "session-middle",
            now - Duration::minutes(5),
            "MiniMax-M2.5",
            SessionStatus::Completed,
        );
        write_session_for_test(
            workspace,
            "session-newest",
            now,
            "MiniMax-M2.5",
            SessionStatus::Running,
        );

        let config = nca_common::config::NcaConfig::default();
        let session_id = get_last_session_id(&config, workspace)
            .await
            .expect("get_last_session_id should succeed")
            .expect("should find a session");

        // Should find the most recent session
        assert_eq!(session_id, "session-newest");

        // The .last_session file should now be updated
        let last_session_path = workspace.join(".nca").join(".last_session");
        assert!(
            last_session_path.exists(),
            ".last_session should be created"
        );
        let content = std::fs::read_to_string(&last_session_path).unwrap();
        assert_eq!(content.trim(), "session-newest");
    }

    #[tokio::test]
    async fn send_message_forwards_prompt_to_session_queue() {
        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        let (prompt_tx, mut prompt_rx) = mpsc::channel(16);
        let (control_tx, _control_rx) = mpsc::channel(16);

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
                content: "hello from ipc".into(),
            })
            .await
            .unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), prompt_rx.recv())
            .await
            .expect("prompt should be forwarded")
            .expect("prompt channel should remain open");
        assert_eq!(received, "hello from ipc");

        let _ = cmd_tx.send(AgentCommand::Shutdown).await;
        task.abort();
    }

    #[tokio::test]
    async fn answer_question_resolves_pending_channel() {
        use nca_common::event::QuestionSelection;

        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<QuestionSelection>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert("q-1".into(), tx);

        let task = spawn_command_consumer_with_store(
            cmd_rx,
            None,
            Some(pending.clone()),
            None,
            None,
            None,
            None,
        );

        cmd_tx
            .send(AgentCommand::AnswerQuestion {
                question_id: "q-1".into(),
                selection: QuestionSelection::Suggested,
            })
            .await
            .unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("timeout")
            .expect("channel");
        assert!(matches!(got, QuestionSelection::Suggested));

        let _ = cmd_tx.send(AgentCommand::Shutdown).await;
        task.abort();
    }
}
