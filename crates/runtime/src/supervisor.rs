pub(crate) use crate::subagent::build_parent_summary;
pub use crate::subagent::{
    ChildSessionConfig, ChildSessionResult, spawn_child_session, spawn_subagent_consumer,
};

pub(crate) use crate::session_utils::{ApprovalPendingMap, QuestionPendingMap};
pub use crate::session_utils::{
    SessionControlCommand, cleanup_stale_sessions, get_last_session_id, list_sessions,
    query_session_state, spawn_command_consumer, spawn_command_consumer_with_store,
    spawn_event_fanout,
};

use crate::context_manager::{ContextManager, ContextManagerConfig, ContextStats};
use crate::ipc::{IpcHandle, IpcServer};
use crate::last_session::LastSessionStore;
use crate::memory_store::{MemoryNote, MemoryStore};
use crate::model_limits_api;
use crate::pty::PtyManager;
use crate::session_store::SessionStore;
use chrono::Utc;
use nca_common::config::NcaConfig;
use nca_common::event::{AgentEvent, EndReason};
use nca_common::session::{
    OrchestrationContext, SessionMeta, SessionSnapshot, SessionState, SessionStatus,
};
use nca_core::agent::AgentLoop;
use nca_core::approval::{ApprovalHandler, ApprovalPolicy, ApprovalVerdict};
use nca_core::harness::build_system_prompt;
use nca_core::hooks::{HookEventKind, HookRunner};
use nca_core::provider::ProviderError;
use nca_core::provider::factory::build_provider;
use nca_core::tools::AskQuestionTool;
use nca_core::tools::InvokeSkillTool;
use nca_core::tools::ToolRegistry;
use nca_core::tools::mcp::load_mcp_tools;
use nca_core::tools::spawn_subagent::{SpawnRequest, SpawnSubagentTool};
use nca_core::workspace_fs::RealFs;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

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
    pub(crate) worktree_path: Option<PathBuf>,
    pub(crate) branch: Option<String>,
    pub(crate) base_branch: Option<String>,
    parent_session_id: Option<String>,
    child_session_ids: Vec<String>,
    inherited_summary: Option<String>,
    spawn_reason: Option<String>,
    session_summary: Option<String>,
    session_title: Option<String>,
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
    approval_pending: Option<ApprovalPendingMap>,
    question_pending: Option<QuestionPendingMap>,
    spawn_rx: Option<mpsc::Receiver<SpawnRequest>>,
}

impl SupervisorHandle {
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<AgentEvent>> {
        self.event_rx.take()
    }

    pub fn take_ipc_handle(&mut self) -> Option<IpcHandle> {
        self.ipc_handle.take()
    }

    pub fn take_approval_pending(&mut self) -> Option<ApprovalPendingMap> {
        self.approval_pending.take()
    }

    pub fn take_question_pending(&mut self) -> Option<QuestionPendingMap> {
        self.question_pending.take()
    }

    pub fn take_spawn_rx(&mut self) -> Option<mpsc::Receiver<SpawnRequest>> {
        self.spawn_rx.take()
    }
}

/// Persist an approved allow pattern to the workspace config file.
fn persist_allow_pattern(workspace_root: &Path, pattern: String) {
    let root = workspace_root.to_path_buf();
    std::mem::drop(tokio::runtime::Handle::current().spawn_blocking(move || {
        match nca_common::config::NcaConfig::load_for_workspace(&root) {
            Ok(mut config) => {
                if !config.permissions.allow.contains(&pattern) {
                    config.permissions.allow.push(pattern);
                    tracing::info!("persisted allow pattern to workspace config");
                    if let Err(e) = config.save_workspace_file(&root) {
                        tracing::warn!("failed to persist allow pattern: {e}");
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to load config for pattern persistence: {e}");
            }
        }
    }));
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
        let fs: Arc<dyn nca_core::workspace_fs::WorkspaceFs> =
            Arc::new(RealFs::new(workspace_root.clone()));
        let mut tools = if cfg.safe_mode {
            ToolRegistry::with_default_readonly_tools(fs.clone(), config.web.clone())
        } else {
            ToolRegistry::with_default_full_tools(fs, config.web.clone())
        };
        if !config.mcp.servers.is_empty() && (!cfg.safe_mode || config.mcp.expose_in_safe_mode) {
            match load_mcp_tools(&workspace_root, &config.mcp.servers).await {
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

        let approval_pending: Option<ApprovalPendingMap>;
        let approval = if cfg.interactive_approvals {
            match cfg.approval_handler {
                Some(handler) => {
                    approval_pending = None;
                    ApprovalPolicy::new(config.permissions.clone())
                        .with_handler(handler)
                        .with_persist({
                            let wr = workspace_root.clone();
                            move |p| persist_allow_pattern(&wr, p)
                        })
                }
                None => {
                    let ipc_handler = IpcApprovalHandler::new();
                    approval_pending = Some(ipc_handler.pending());
                    ApprovalPolicy::new(config.permissions.clone())
                        .with_handler(ipc_handler as Arc<dyn ApprovalHandler>)
                        .with_persist({
                            let wr = workspace_root.clone();
                            move |p| persist_allow_pattern(&wr, p)
                        })
                }
            }
        } else {
            approval_pending = None;
            ApprovalPolicy::new(config.permissions.clone())
                .fail_on_ask()
                .with_handler(Arc::new(AutoDenyHandler) as Arc<dyn ApprovalHandler>)
                .with_persist({
                    let wr = workspace_root.clone();
                    move |p| persist_allow_pattern(&wr, p)
                })
        };

        let (event_tx, event_rx) = mpsc::channel(256);
        let question_pending = Arc::new(Mutex::new(HashMap::new()));
        tools.register(Box::new(AskQuestionTool::new(
            event_tx.clone(),
            question_pending.clone(),
        )));
        tools.register(Box::new(InvokeSkillTool::new(
            workspace_root.clone(),
            config.harness.skill_directories.clone(),
        )));
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
            event_tx.clone(),
            config.session.max_turns_per_run,
            config.session.max_tool_calls_per_turn,
            config.session.checkpoint_interval,
            hook_runner.clone(),
        );
        let system_prompt =
            build_system_prompt(&config, &workspace_root, cfg.orchestration_context.as_ref());
        agent.set_system_prompt(system_prompt);

        let context_manager =
            Self::make_context_manager(&config, &config.model.default_model).await;

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
            session_title: None,
            orchestration: cfg.orchestration_context,
            config,
            hooks: hook_runner,
            context_manager,
            last_summary_at_tokens: 0,
        };
        sup.save().await.map_err(ProviderError::Other)?;
        sup.update_last_session()
            .await
            .map_err(ProviderError::Other)?;
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
        approval_handler: Option<Arc<dyn ApprovalHandler>>,
    ) -> Result<Self, ProviderError> {
        // Load the original session state BEFORE create() overwrites the file.
        // create() calls save() with an empty message list, which would destroy
        // the conversation history if we loaded after.
        let store = SessionStore::new(workspace_root.join(&config.session.history_dir));
        let loaded = store
            .load(session_id)
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let mut sup = Self::create(SupervisorConfig {
            config: config.clone(),
            workspace_root: workspace_root.to_path_buf(),
            safe_mode,
            interactive_approvals,
            session_id: Some(session_id.into()),
            approval_handler,
            orchestration_context: None,
        })
        .await?;

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
        sup.session_title = loaded.meta.session_title;
        sup.orchestration = loaded.meta.orchestration;
        sup.context_manager = Self::make_context_manager(&sup.config, &sup.model).await;
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

        // Emit context stats for UI
        self.emit_context_stats().await;

        self.refresh_session_summary();

        // Generate session title from the first user prompt if not yet set.
        if self.session_title.is_none() {
            self.generate_session_title(prompt).await;
        }

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
            tracing::debug!(
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
    /// Proactively compact context **before** a turn runs.
    /// Unlike `check_and_summarize_context` (post-turn), this must act immediately
    /// to prevent empty-response failures caused by an overflowing context window.
    async fn maybe_compact_context(&mut self) {
        if !self.context_manager.config().enable_auto_summarize {
            return;
        }

        let stats = self.context_manager.stats(&self.agent.messages);

        // Already small enough — nothing to do.
        if !stats.should_summarize {
            return;
        }

        // Don't re-summarize if we already compacted in this session and context
        // has not grown past the previous post-compaction size.
        if self.last_summary_at_tokens > 0 && stats.estimated_tokens <= self.last_summary_at_tokens
        {
            return;
        }

        if let Some(tx) = self.agent.event_sender() {
            let _ = tx
                .send(AgentEvent::ContextCompaction {
                    phase: "starting".to_string(),
                    message: format!(
                        "Auto-summarizing context before turn ({}% full, {} tokens)",
                        stats.usage_percent, stats.estimated_tokens
                    ),
                })
                .await;
        }

        if let Err(e) = self.perform_auto_summarize().await {
            tracing::error!("Pre-turn auto-summarize failed: {}", e);
            self.last_summary_at_tokens = 0;
        }
    }

    /// Check if context should be summarized after a turn and trigger if needed.
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

    /// Emit current context statistics to the UI via the event bus.
    async fn emit_context_stats(&self) {
        let stats = self.context_manager.stats(&self.agent.messages);
        if let Some(tx) = self.agent.event_sender() {
            let _ = tx
                .send(AgentEvent::ContextStatsUpdated {
                    estimated_tokens: stats.estimated_tokens,
                    context_window: stats.context_window,
                    usage_percent: stats.usage_percent,
                })
                .await;
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
            let _ = tx.send(AgentEvent::SessionEnded { reason }).await;
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
                session_title: self.session_title.clone(),
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

    pub fn set_session_title(&mut self, title: Option<String>) {
        self.session_title = title.filter(|t| !t.trim().is_empty());
    }

    pub fn session_title(&self) -> Option<&str> {
        self.session_title.as_deref()
    }

    /// Generate a concise session title from the first user prompt using the LLM.
    /// Runs asynchronously and does not block the main turn flow.
    pub async fn generate_session_title(&mut self, first_prompt: &str) {
        if self.session_title.is_some() {
            return;
        }
        let prompt = format!(
            "Based on the user's first message below, generate a very short title \
             (at most 20 words, in the same language as the user's message) that \
             summarizes the topic of this coding session. Output ONLY the title, \
             nothing else.\n\nUser's first message:\n{first_prompt}"
        );
        match self.summarize_with_ai(&prompt).await {
            Ok(title) => {
                let cleaned = title
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !cleaned.is_empty() {
                    self.set_session_title(Some(cleaned));
                }
            }
            Err(e) => {
                tracing::warn!("failed to generate session title: {e}");
            }
        }
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
        self.session_title = None;
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
        let m = self.model.clone();
        let agent = self.agent_mut();
        agent.model = m;
        agent.replace_provider(provider);
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

/// IPC-based approval handler that waits for approve/deny commands from
/// connected clients (e.g. CLI over the session socket).
pub struct IpcApprovalHandler {
    pending: ApprovalPendingMap,
}

impl IpcApprovalHandler {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn pending(&self) -> ApprovalPendingMap {
        self.pending.clone()
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for IpcApprovalHandler {
    async fn resolve(
        &self,
        call: &nca_common::tool::ToolCall,
        _description: &str,
    ) -> ApprovalVerdict {
        let (tx, rx) = oneshot::channel();
        {
            let mut m = self.pending.lock().unwrap();
            m.insert(call.id.clone(), tx);
        }
        match rx.await {
            Ok(verdict) => verdict,
            Err(_) => {
                let mut m = self.pending.lock().unwrap();
                m.remove(&call.id);
                ApprovalVerdict::Denied
            }
        }
    }
}

/// Auto-deny handler for non-interactive sessions.
pub(crate) struct AutoDenyHandler;

#[async_trait::async_trait]
impl ApprovalHandler for AutoDenyHandler {
    async fn resolve(
        &self,
        _call: &nca_common::tool::ToolCall,
        _description: &str,
    ) -> ApprovalVerdict {
        ApprovalVerdict::Denied
    }
}

fn generate_session_id() -> String {
    static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{}-{counter}", Utc::now().timestamp_micros())
}
