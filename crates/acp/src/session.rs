//! Per-session state: supervisor lifecycle, event pump, and prompt execution.

use crate::approval::AcpApprovalHandler;
use crate::translate;
use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v1::{
    CompactionStatus, ConfigOptionUpdate, ContentBlock, CurrentModeUpdate, ElicitationAction,
    ElicitationContentValue, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope,
    EmbeddedResourceResource, SessionId, SessionInfoUpdate, SessionNotification, SessionUpdate,
    StopReason, ToolCallId,
};
use agent_client_protocol::schema::v1::{CreateElicitationRequest, SessionConfigOption};
use agent_client_protocol::{Client, ConnectionTo};
use base64::Engine;
use nca_common::config::{NcaConfig, PermissionMode};
use nca_common::event::{AgentEvent, EventEnvelope, InteractiveQuestionPayload, QuestionSelection};
use nca_common::message::ImageAttachment;
use nca_runtime::supervisor::{
    QuestionPendingMap, Supervisor, SupervisorConfig, spawn_subagent_consumer,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
const FLUSH_BYTES: usize = 256;
/// `@file` mentions in an ACP prompt are inlined like the TUI does, but one
/// huge binary must not become the whole context.
const MAX_LINKED_FILE_BYTES: u64 = 512 * 1024;

/// A failure that should be reported to the client as a JSON-RPC error.
pub struct TurnError {
    pub message: String,
    pub stop_reason: Option<StopReason>,
}

impl TurnError {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stop_reason: None,
        }
    }
}

pub struct SessionSlot {
    pub id: String,
    pub cwd: PathBuf,
    safe: bool,
    config: Mutex<NcaConfig>,
    pub(crate) supervisor: Mutex<Option<Supervisor>>,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
    turn_cancel: Mutex<Option<Arc<AtomicBool>>>,
    flush_tx: Mutex<Option<tokio::sync::mpsc::Sender<tokio::sync::oneshot::Sender<()>>>>,
    pub busy: AtomicBool,
    pub last_used: AtomicU64,
    started: AtomicBool,
    /// Model the client picked with `set_model`; survives an eviction so the
    /// resumed supervisor uses it instead of the model saved on disk.
    model_override: Mutex<Option<String>>,
    /// Model ids from the active provider's catalog; empty until fetched.
    model_catalog: Mutex<Vec<String>>,
    question_pending: Mutex<Option<QuestionPendingMap>>,
    pump: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(crate) cx: ConnectionTo<Client>,
    elicitation: Arc<AtomicBool>,
    supports_compaction: bool,
}

impl SessionSlot {
    pub fn new(
        id: String,
        cwd: PathBuf,
        config: NcaConfig,
        safe: bool,
        cx: ConnectionTo<Client>,
        elicitation: Arc<AtomicBool>,
        supports_compaction: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            cwd,
            safe,
            config: Mutex::new(config),
            supervisor: Mutex::new(None),
            cancel: Mutex::new(None),
            turn_cancel: Mutex::new(None),
            flush_tx: Mutex::new(None),
            busy: AtomicBool::new(false),
            last_used: AtomicU64::new(0),
            started: AtomicBool::new(false),
            model_override: Mutex::new(None),
            model_catalog: Mutex::new(Vec::new()),
            question_pending: Mutex::new(None),
            pump: Mutex::new(None),
            cx,
            elicitation,
            supports_compaction,
        })
    }

    pub fn touch(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last_used.store(now, Ordering::Relaxed);
    }

    /// Signal the running turn to stop. Never waits for the supervisor lock.
    pub async fn cancel(&self) {
        if let Some(flag) = self.cancel.lock().await.as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
        // The agent's own flag is what aborts a stalled provider request or a
        // streaming response; the approval flag alone only stops new tools.
        if let Some(flag) = self.turn_cancel.lock().await.as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    pub async fn config_snapshot(&self) -> (NcaConfig, String, PermissionMode) {
        let config = self.config.lock().await.clone();
        let model = config.provider.active_model().to_string();
        let mode = config.permissions.mode;
        (config, model, mode)
    }

    pub async fn config_options(&self) -> Vec<SessionConfigOption> {
        let (config, model, mode) = self.config_snapshot().await;
        let catalog = self.model_catalog.lock().await;
        crate::options::config_options(&config, &model, mode, &catalog)
    }

    /// Load the provider's model list if it arrives within `wait`. Returns
    /// without it otherwise; `refresh_model_catalog` then fills it in later.
    pub async fn load_model_catalog(&self, wait: Duration) {
        let config = self.config.lock().await.clone();
        // A list saved by an earlier process answers at once; the background
        // refresh then replaces it with the live one.
        let saved = nca_runtime::model_limits_api::cached_provider_model_ids(&config);
        if !saved.is_empty() {
            *self.model_catalog.lock().await = saved;
            return;
        }
        let fetch = nca_runtime::model_limits_api::fetch_provider_model_ids(&config);
        if let Ok(ids) = tokio::time::timeout(wait, fetch).await
            && !ids.is_empty()
        {
            *self.model_catalog.lock().await = ids;
        }
    }

    /// Fetch the active provider's model list in the background and push the
    /// refreshed picker to the client. The session response never waits on
    /// the network; the catalog is cached per process after the first fetch.
    pub fn refresh_model_catalog(self: &Arc<Self>) {
        let slot = Arc::clone(self);
        tokio::spawn(async move {
            let config = slot.config.lock().await.clone();
            let ids = nca_runtime::model_limits_api::fetch_provider_model_ids(&config).await;
            if ids.is_empty() {
                return;
            }
            {
                let mut catalog = slot.model_catalog.lock().await;
                if *catalog == ids {
                    return;
                }
                *catalog = ids;
            }
            let options = slot.config_options().await;
            slot.notify(SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(
                options,
            )));
        });
    }

    pub async fn set_mode(&self, mode: PermissionMode) -> Result<(), String> {
        {
            let mut config = self.config.lock().await;
            config.permissions.mode = mode;
        }
        let mut guard = self.supervisor.lock().await;
        if let Some(sup) = guard.as_mut() {
            sup.set_permission_mode(mode);
        }
        Ok(())
    }

    pub async fn set_model(&self, model: &str) -> Result<(), String> {
        let updated = {
            let config = self.config.lock().await;
            let mut candidate = config.clone();
            candidate.apply_model_override(model);
            candidate
        };

        let mut guard = self.supervisor.lock().await;
        if let Some(sup) = guard.as_mut() {
            sup.apply_nca_config(updated.clone())
                .map_err(|e| e.to_string())?;
        }
        *self.config.lock().await = updated;
        *self.model_override.lock().await = Some(model.to_string());
        Ok(())
    }

    /// Create the supervisor the first time, resume it after an eviction.
    pub async fn ensure_supervisor(&self) -> Result<(), String> {
        if self.supervisor.lock().await.is_some() {
            return Ok(());
        }
        if self.started.load(Ordering::SeqCst) {
            let start_id = last_event_id(&self.event_log_path().await).await;
            self.resume(start_id).await
        } else {
            self.create().await
        }
    }

    /// Resume an existing on-disk session, replaying history separately.
    ///
    /// A slot that is already live (or mid-turn) is left alone: rebuilding it
    /// would orphan the running turn's cancel flags and event pump, and later
    /// replace the live history with the older copy on disk.
    pub async fn resume_existing(&self, start_event_id: u64) -> Result<(), String> {
        // The session exists on disk, so a later prompt must resume it even if
        // this attempt fails; creating would save an empty session over it.
        self.started.store(true, Ordering::SeqCst);
        if self.busy.load(Ordering::SeqCst) {
            return Ok(());
        }
        match self.supervisor.try_lock() {
            Ok(guard) if guard.is_some() => return Ok(()),
            Ok(_) => {}
            Err(_) => return Ok(()),
        }
        self.resume(start_event_id).await
    }

    async fn create(&self) -> Result<(), String> {
        let config = self.config.lock().await.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let handler = AcpApprovalHandler::new(
            SessionId::new(self.id.clone()),
            self.cx.clone(),
            self.cwd.clone(),
            cancel.clone(),
        );
        let sup = Supervisor::create(SupervisorConfig {
            config,
            workspace_root: self.cwd.clone(),
            safe_mode: self.safe,
            interactive_approvals: true,
            session_id: Some(self.id.clone()),
            approval_handler: Some(Arc::new(handler)),
            orchestration_context: None,
            enable_ipc: false,
        })
        .await
        .map_err(|e| e.to_string())?;

        *self.cancel.lock().await = Some(cancel);
        self.started.store(true, Ordering::SeqCst);
        self.install(sup, 0).await
    }

    async fn resume(&self, start_event_id: u64) -> Result<(), String> {
        let config = self.config.lock().await.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let handler = AcpApprovalHandler::new(
            SessionId::new(self.id.clone()),
            self.cx.clone(),
            self.cwd.clone(),
            cancel.clone(),
        );
        let mut sup = Supervisor::resume_with_ipc(
            config.clone(),
            &self.cwd,
            self.safe,
            true,
            &self.id,
            Some(Arc::new(handler)),
            false,
        )
        .await
        .map_err(|e| e.to_string())?;

        // Resume restores the model saved with the session. Keep a model the
        // client picked since then; otherwise report the restored one.
        let override_model = self.model_override.lock().await.clone();
        let mut synced = config;
        synced.apply_model_override(override_model.as_deref().unwrap_or(&sup.model));
        if override_model.is_some_and(|model| model != sup.model) {
            sup.apply_nca_config(synced.clone())
                .map_err(|e| e.to_string())?;
        }
        *self.config.lock().await = synced;

        *self.cancel.lock().await = Some(cancel);
        self.started.store(true, Ordering::SeqCst);
        self.install(sup, start_event_id).await
    }

    async fn install(&self, mut sup: Supervisor, start_event_id: u64) -> Result<(), String> {
        let event_log = sup.event_log_path();
        let event_tx = sup.event_tx();
        let parent_messages = sup.agent().messages.clone();
        let config = self.config.lock().await.clone();
        let workspace = sup.workspace_root.clone();

        let mut handle = sup.take_handle();
        let event_rx = handle
            .take_event_rx()
            .ok_or_else(|| "session event channel already taken".to_string())?;
        let question_pending = handle.take_question_pending();
        if let Some(spawn_rx) = handle.take_spawn_rx() {
            spawn_subagent_consumer(
                spawn_rx,
                self.id.clone(),
                workspace.clone(),
                config,
                parent_messages,
                event_tx,
            );
        }

        *self.question_pending.lock().await = question_pending.clone();
        let (flush_tx, flush_rx) = tokio::sync::mpsc::channel(4);
        let pump = spawn_event_pump(
            event_rx,
            flush_rx,
            PumpConfig {
                log_path: event_log,
                session_id: SessionId::new(self.id.clone()),
                cx: self.cx.clone(),
                question_pending,
                workspace,
                elicitation: self.elicitation.clone(),
                supports_compaction: self.supports_compaction,
                start_event_id,
            },
        );
        *self.pump.lock().await = Some(pump);
        *self.flush_tx.lock().await = Some(flush_tx);

        *self.turn_cancel.lock().await = Some(sup.cancel_handle());
        *self.supervisor.lock().await = Some(sup);
        Ok(())
    }

    async fn event_log_path(&self) -> PathBuf {
        let config = self.config.lock().await;
        nca_common::config::resolve_sessions_dir(&config, &self.cwd)
            .join(format!("{}.events.jsonl", self.id))
    }

    /// Save and drop the supervisor while keeping the lightweight slot.
    pub async fn evict(&self) {
        let mut guard = self.supervisor.lock().await;
        if let Some(sup) = guard.as_ref()
            && let Err(error) = sup.save().await
        {
            tracing::warn!("failed to save evicted session {}: {error}", self.id);
        }
        *guard = None;
        drop(guard);
        *self.turn_cancel.lock().await = None;
        *self.flush_tx.lock().await = None;
        self.stop_pump().await;
    }

    /// Drop the supervisor without saving. Used by `session/delete`, where the
    /// state and event log are about to be removed from disk.
    pub async fn discard(&self) {
        *self.supervisor.lock().await = None;
        *self.cancel.lock().await = None;
        *self.turn_cancel.lock().await = None;
        *self.flush_tx.lock().await = None;
        self.stop_pump().await;
    }

    async fn stop_pump(&self) {
        let handle = self.pump.lock().await.take();
        if let Some(handle) = handle {
            let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
        }
    }

    /// Wait until the event pump has sent every pending text chunk. Without
    /// this, the last tokens can arrive after the prompt response.
    async fn flush_pending(&self) {
        let Some(tx) = self.flush_tx.lock().await.clone() else {
            return;
        };
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        if tx.send(done_tx).await.is_err() {
            return;
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), done_rx).await;
    }

    pub async fn run_prompt(&self, blocks: Vec<ContentBlock>) -> Result<StopReason, TurnError> {
        let command = command_text(&blocks);
        let (prompt, images) = convert_prompt(blocks, &self.cwd).await?;
        if let Some(command) = command
            && let Some(stop) = self.run_command(&command).await?
        {
            return Ok(stop);
        }
        self.ensure_supervisor().await.map_err(TurnError::plain)?;

        let cancel = self.cancel.lock().await.clone();
        if let Some(flag) = cancel.as_ref() {
            flag.store(false, Ordering::SeqCst);
        }
        let mut guard = self.supervisor.lock().await;
        let sup = guard
            .as_mut()
            .ok_or_else(|| TurnError::plain("session supervisor is not available"))?;

        let result = sup.run_turn_with_images(&prompt, &images).await;
        let stats = sup.context_stats();
        let snapshot = sup.snapshot();
        drop(guard);

        self.flush_pending().await;

        self.notify(translate::usage(
            stats.estimated_tokens as u64,
            stats.context_window as u64,
            snapshot.estimated_cost_usd,
        ));

        let cancelled = cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst));
        match result {
            Ok(_) if cancelled => Ok(StopReason::Cancelled),
            Ok(_) => Ok(StopReason::EndTurn),
            Err(error) => {
                let message = error.to_string();
                if cancelled || message.contains("run cancelled") {
                    Ok(StopReason::Cancelled)
                } else if message.contains("output token limit") {
                    Ok(StopReason::MaxTokens)
                } else {
                    Err(TurnError::plain(message))
                }
            }
        }
    }

    /// Commands handled inside the agent instead of being sent to the model.
    /// Unknown `/...` prompts fall through unchanged, so a message such as
    /// `/etc/hosts is broken` still reaches the model.
    async fn run_command(&self, prompt: &str) -> Result<Option<StopReason>, TurnError> {
        let trimmed = prompt.trim();
        let Some(rest) = trimmed.strip_prefix('/') else {
            return Ok(None);
        };
        let mut parts = rest.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or("").to_ascii_lowercase();
        let arg = parts.next().unwrap_or("").trim();

        match command.as_str() {
            "rename" => {
                self.ensure_supervisor().await.map_err(TurnError::plain)?;
                let title = {
                    let mut guard = self.supervisor.lock().await;
                    let sup = guard
                        .as_mut()
                        .ok_or_else(|| TurnError::plain("session supervisor is not available"))?;
                    sup.set_title(arg);
                    let title = sup.title().map(str::to_string);
                    sup.save().await.map_err(TurnError::plain)?;
                    title
                };
                let update = match title.as_deref() {
                    Some(title) => SessionInfoUpdate::new().title(title.to_string()),
                    None => SessionInfoUpdate::new().title(MaybeUndefined::<String>::Null),
                };
                self.notify(SessionUpdate::SessionInfoUpdate(update));
                self.notify(translate::text_chunk(match title {
                    Some(title) => format!("Session renamed to \"{title}\"."),
                    None => "Session title cleared.".to_string(),
                }));
                Ok(Some(StopReason::EndTurn))
            }
            "model" => {
                if arg.is_empty() {
                    return Err(TurnError::plain("usage: /model <name>"));
                }
                self.set_model(arg).await.map_err(TurnError::plain)?;
                let options = self.config_options().await;
                self.notify(SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(
                    options,
                )));
                self.notify(translate::text_chunk(format!("Model set to {arg}.")));
                Ok(Some(StopReason::EndTurn))
            }
            "mode" => {
                let Some(mode) = crate::options::permission_mode_for(arg) else {
                    return Err(TurnError::plain(format!("unknown mode `{arg}`")));
                };
                self.set_mode(mode).await.map_err(TurnError::plain)?;
                self.notify(SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(
                    crate::options::mode_id(mode),
                )));
                self.notify(translate::text_chunk(format!(
                    "Mode set to {}.",
                    crate::options::mode_id(mode)
                )));
                Ok(Some(StopReason::EndTurn))
            }
            "compact" => {
                self.ensure_supervisor().await.map_err(TurnError::plain)?;
                let (stats, snapshot) = {
                    let mut guard = self.supervisor.lock().await;
                    let sup = guard
                        .as_mut()
                        .ok_or_else(|| TurnError::plain("session supervisor is not available"))?;
                    sup.compact_context().await.map_err(TurnError::plain)?;
                    (sup.context_stats(), sup.snapshot())
                };
                self.notify(translate::usage(
                    stats.estimated_tokens as u64,
                    stats.context_window as u64,
                    snapshot.estimated_cost_usd,
                ));
                self.notify(translate::text_chunk(format!(
                    "Context compacted to about {} tokens.",
                    stats.estimated_tokens
                )));
                Ok(Some(StopReason::EndTurn))
            }
            _ => Ok(None),
        }
    }

    fn notify(&self, update: SessionUpdate) {
        let _ = self.cx.send_notification(SessionNotification::new(
            SessionId::new(self.id.clone()),
            update,
        ));
    }
}

fn command_text(blocks: &[ContentBlock]) -> Option<String> {
    if !blocks
        .iter()
        .all(|block| matches!(block, ContentBlock::Text(_)))
    {
        return None;
    }
    Some(
        blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(content) => Some(content.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

async fn convert_prompt(
    blocks: Vec<ContentBlock>,
    cwd: &Path,
) -> Result<(String, Vec<ImageAttachment>), TurnError> {
    let mut text = String::new();
    let mut images = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text(content) => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&content.text);
            }
            ContentBlock::ResourceLink(link) => {
                let path = link
                    .uri
                    .strip_prefix("file://")
                    .unwrap_or(link.uri.as_str());
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&read_linked_file(Path::new(path), cwd).await);
            }
            ContentBlock::Resource(resource) => match resource.resource {
                EmbeddedResourceResource::TextResourceContents(contents) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!(
                        "[resource: {}]\n{}\n[/resource]",
                        contents.uri, contents.text
                    ));
                }
                EmbeddedResourceResource::BlobResourceContents(contents) => {
                    tracing::warn!(
                        "ignoring binary embedded resource {} ({:?})",
                        contents.uri,
                        contents.mime_type
                    );
                }
                _ => {}
            },
            ContentBlock::Image(image) => {
                let attachment = write_image(&image.data, &image.mime_type, cwd).await?;
                images.push(attachment);
            }
            _ => {
                tracing::warn!("ignoring unsupported ACP prompt content block");
            }
        }
    }

    if text.trim().is_empty() && !images.is_empty() {
        text.push_str("Analyze the attached image.");
    }

    Ok((text, images))
}

/// Inline a `resource_link` file the way the TUI expands `@path`, falling back
/// to a bare reference when the file is missing, binary, or too large.
async fn read_linked_file(path: &Path, cwd: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let relative = absolute.strip_prefix(cwd).unwrap_or(&absolute);
    let reference = format!("file:{}", relative.display());

    let Ok(metadata) = tokio::fs::metadata(&absolute).await else {
        return reference;
    };
    if !metadata.is_file() || metadata.len() > MAX_LINKED_FILE_BYTES {
        return reference;
    }
    let Ok(bytes) = tokio::fs::read(&absolute).await else {
        return reference;
    };
    let Ok(content) = String::from_utf8(bytes) else {
        return reference;
    };
    format!(
        "```{}\n{}\n```",
        relative.display(),
        content.trim_end_matches('\n')
    )
}

async fn write_image(
    data: &str,
    mime_type: &str,
    cwd: &Path,
) -> Result<ImageAttachment, TurnError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| TurnError::plain(format!("invalid image payload: {e}")))?;
    let extension = match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    };
    let dir = cwd.join(".nca").join("acp-attachments");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| TurnError::plain(format!("cannot create attachment dir: {e}")))?;
    let name = format!(
        "acp-{}-{}.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        std::process::id(),
        extension
    );
    let path = dir.join(&name);
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| TurnError::plain(format!("cannot write image attachment: {e}")))?;
    Ok(ImageAttachment {
        media_type: mime_type.to_string(),
        path: format!(".nca/acp-attachments/{name}"),
    })
}

async fn last_event_id(path: &Path) -> u64 {
    use tokio::io::AsyncBufReadExt;
    let Ok(file) = tokio::fs::File::open(path).await else {
        return 0;
    };
    let mut lines = tokio::io::BufReader::new(file).lines();
    let mut last = 0u64;
    while let Ok(Some(line)) = lines.next_line().await {
        if let Ok(envelope) = serde_json::from_str::<EventEnvelope>(&line) {
            last = last.max(envelope.id);
        }
    }
    last
}

#[allow(clippy::too_many_arguments)]
struct PumpState {
    log_file: Option<tokio::fs::File>,
    event_id: u64,
    text: String,
    thought: String,
    saw_token: bool,
    deadline: Option<tokio::time::Instant>,
    compaction_id: u64,
    active_compaction: Option<String>,
    session_id: SessionId,
    cx: ConnectionTo<Client>,
    question_pending: Option<QuestionPendingMap>,
    workspace: PathBuf,
    elicitation: Arc<AtomicBool>,
    supports_compaction: bool,
    /// Tool name and input per running call, so a result can be matched to
    /// the image it produced.
    tool_inputs: std::collections::HashMap<String, (String, serde_json::Value)>,
}

impl PumpState {
    fn flush(&mut self) {
        flush(
            &self.cx,
            &self.session_id,
            &mut self.text,
            &mut self.thought,
        );
    }

    async fn record(&mut self, event: &AgentEvent) {
        self.event_id += 1;
        if let Some(file) = self.log_file.as_mut() {
            let envelope = EventEnvelope::new(self.event_id, event.clone());
            if let Ok(mut line) = serde_json::to_string(&envelope) {
                line.push('\n');
                let _ = file.write_all(line.as_bytes()).await;
            }
        }
    }

    async fn handle(&mut self, event: AgentEvent) {
        self.record(&event).await;
        match event {
            AgentEvent::TokensStreamed { delta } => {
                self.text.push_str(&delta);
                self.saw_token = true;
                if self.text.len() >= FLUSH_BYTES {
                    self.flush();
                    self.deadline = None;
                } else if self.deadline.is_none() {
                    self.deadline = Some(tokio::time::Instant::now() + FLUSH_INTERVAL);
                }
            }
            AgentEvent::MessageReceived { role, content } if role == "assistant" => {
                if !self.saw_token {
                    self.text.push_str(&content);
                }
                self.saw_token = false;
            }
            AgentEvent::MessageReceived { .. } => {}
            AgentEvent::ToolCallStarted {
                call_id,
                tool,
                input,
            } => {
                self.flush();
                self.deadline = None;
                self.saw_token = false;
                send(
                    &self.cx,
                    &self.session_id,
                    translate::tool_started(&call_id, &tool, &input, &self.workspace),
                );
                self.tool_inputs.insert(call_id, (tool, input));
            }
            AgentEvent::ToolCallCompleted { call_id, output } => {
                self.flush();
                self.deadline = None;
                self.saw_token = false;
                let (tool, input) = self
                    .tool_inputs
                    .remove(&call_id)
                    .unwrap_or((String::new(), serde_json::Value::Null));
                for update in translate::tool_completed_updates(
                    &call_id,
                    &tool,
                    &input,
                    &output,
                    &self.workspace,
                )
                .await
                {
                    send(&self.cx, &self.session_id, update);
                }
            }
            AgentEvent::TodosUpdated { todos } => {
                self.flush();
                self.deadline = None;
                send(&self.cx, &self.session_id, translate::plan_update(&todos));
            }
            AgentEvent::ContextCompaction { phase, message, .. } if self.supports_compaction => {
                self.flush();
                self.deadline = None;
                let status = match phase.as_str() {
                    "starting" => CompactionStatus::InProgress,
                    "completed" => CompactionStatus::Completed,
                    "failed" => CompactionStatus::Failed,
                    "cancelled" => CompactionStatus::Cancelled,
                    _ => CompactionStatus::Other(phase.clone()),
                };
                if status == CompactionStatus::InProgress || self.active_compaction.is_none() {
                    self.compaction_id += 1;
                    self.active_compaction = Some(format!("ctx-{}", self.compaction_id));
                }
                let id = self
                    .active_compaction
                    .clone()
                    .unwrap_or_else(|| format!("ctx-{}", self.compaction_id));
                let terminal = matches!(
                    status,
                    CompactionStatus::Completed
                        | CompactionStatus::Failed
                        | CompactionStatus::Cancelled
                );
                send(
                    &self.cx,
                    &self.session_id,
                    translate::compaction_update(&id, status, Some(&message)),
                );
                if terminal {
                    self.active_compaction = None;
                }
            }
            AgentEvent::QuestionRequested { question } => {
                self.flush();
                self.deadline = None;
                spawn_question(
                    self.cx.clone(),
                    self.session_id.clone(),
                    self.question_pending.clone(),
                    self.elicitation.clone(),
                    question,
                );
            }
            AgentEvent::SessionEnded { .. } => {
                self.flush();
                self.deadline = None;
            }
            AgentEvent::Error { message } if !message.contains("run cancelled") => {
                tracing::warn!("agent error: {message}");
            }
            _ => {}
        }
    }

    /// Process events that are already queued. `run_turn` has returned by the
    /// time a flush is requested, so this drains the whole turn instead of
    /// acknowledging a half-rendered answer.
    async fn drain(&mut self, rx: &mut tokio::sync::mpsc::Receiver<AgentEvent>) {
        while let Ok(event) = rx.try_recv() {
            self.handle(event).await;
        }
    }
}

struct PumpConfig {
    log_path: PathBuf,
    session_id: SessionId,
    cx: ConnectionTo<Client>,
    question_pending: Option<QuestionPendingMap>,
    workspace: PathBuf,
    elicitation: Arc<AtomicBool>,
    supports_compaction: bool,
    start_event_id: u64,
}

fn spawn_event_pump(
    mut event_rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    mut flush_rx: tokio::sync::mpsc::Receiver<tokio::sync::oneshot::Sender<()>>,
    config: PumpConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let log_file = {
            if let Some(parent) = config.log_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&config.log_path)
                .await
                .ok()
        };

        let mut state = PumpState {
            log_file,
            event_id: config.start_event_id,
            text: String::new(),
            thought: String::new(),
            saw_token: false,
            deadline: None,
            compaction_id: 0,
            active_compaction: None,
            session_id: config.session_id,
            cx: config.cx,
            question_pending: config.question_pending,
            workspace: config.workspace,
            elicitation: config.elicitation,
            supports_compaction: config.supports_compaction,
            tool_inputs: std::collections::HashMap::new(),
        };

        loop {
            let event = match state.deadline {
                Some(at) => {
                    tokio::select! {
                        event = event_rx.recv() => event,
                        _ = tokio::time::sleep_until(at) => {
                            state.flush();
                            state.deadline = None;
                            continue;
                        }
                        request = flush_rx.recv() => {
                            state.drain(&mut event_rx).await;
                            state.flush();
                            state.deadline = None;
                            if let Some(done) = request {
                                let _ = done.send(());
                            }
                            continue;
                        }
                    }
                }
                None => {
                    tokio::select! {
                        event = event_rx.recv() => event,
                        request = flush_rx.recv() => {
                            state.drain(&mut event_rx).await;
                            state.flush();
                            if let Some(done) = request {
                                let _ = done.send(());
                            }
                            continue;
                        }
                    }
                }
            };

            let Some(event) = event else {
                state.flush();
                break;
            };
            state.handle(event).await;
        }
    })
}

fn flush(
    cx: &ConnectionTo<Client>,
    session_id: &SessionId,
    text: &mut String,
    thought: &mut String,
) {
    if !thought.is_empty() {
        let chunk = std::mem::take(thought);
        send(cx, session_id, translate::thought_chunk(chunk));
    }
    if !text.is_empty() {
        let chunk = std::mem::take(text);
        send(cx, session_id, translate::text_chunk(chunk));
    }
}

fn send(cx: &ConnectionTo<Client>, session_id: &SessionId, update: SessionUpdate) {
    if let Err(error) = cx.send_notification(SessionNotification::new(session_id.clone(), update)) {
        tracing::warn!("failed to send session update: {error}");
    }
}

fn spawn_question(
    cx: ConnectionTo<Client>,
    session_id: SessionId,
    pending: Option<QuestionPendingMap>,
    elicitation: Arc<AtomicBool>,
    question: InteractiveQuestionPayload,
) {
    tokio::spawn(async move {
        let selection = if elicitation.load(Ordering::SeqCst) {
            ask_via_elicitation(&cx, &session_id, &question).await
        } else {
            QuestionSelection::Suggested
        };

        let Some(pending) = pending else {
            return;
        };
        let sender = pending
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&question.question_id));
        if let Some(sender) = sender {
            let _ = sender.send(selection);
        }
    });
}

async fn ask_via_elicitation(
    cx: &ConnectionTo<Client>,
    session_id: &SessionId,
    question: &InteractiveQuestionPayload,
) -> QuestionSelection {
    let schema = ElicitationSchema::new()
        .title(Some(question.prompt.clone()))
        .string("answer", true);
    let scope = ElicitationSessionScope::new(session_id.clone())
        .tool_call_id(Some(ToolCallId::new(question.call_id.clone())));
    let request = CreateElicitationRequest::new(
        ElicitationFormMode::new(scope, schema),
        question.prompt.clone(),
    );

    match cx.send_request(request).block_task().await {
        Ok(response) => match response.action {
            ElicitationAction::Accept(action) => action
                .content
                .and_then(|content| content.get("answer").cloned())
                .map(|value| match value {
                    ElicitationContentValue::String(text) if !text.trim().is_empty() => {
                        QuestionSelection::Custom { text }
                    }
                    _ => QuestionSelection::Suggested,
                })
                .unwrap_or(QuestionSelection::Suggested),
            _ => QuestionSelection::Suggested,
        },
        Err(error) => {
            tracing::warn!("elicitation failed, using suggested answer: {error}");
            QuestionSelection::Suggested
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn linked_files_are_inlined_with_their_relative_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/a.rs"), "fn main() {}\n").expect("write");

        let text = read_linked_file(Path::new("src/a.rs"), dir.path()).await;
        assert!(text.starts_with("```src/a.rs\n"), "got {text:?}");
        assert!(text.contains("fn main() {}"));
        assert!(text.ends_with("```"));
    }

    #[tokio::test]
    async fn linked_files_fall_back_to_a_reference_when_they_cannot_be_inlined() {
        let dir = tempfile::tempdir().expect("tempdir");

        let missing = read_linked_file(Path::new("nope.rs"), dir.path()).await;
        assert_eq!(missing, "file:nope.rs");

        std::fs::write(dir.path().join("bin.dat"), [0xff, 0xfe, 0x00]).expect("write binary");
        let binary = read_linked_file(Path::new("bin.dat"), dir.path()).await;
        assert_eq!(binary, "file:bin.dat");

        let big = "x".repeat((MAX_LINKED_FILE_BYTES + 1) as usize);
        std::fs::write(dir.path().join("big.txt"), big).expect("write big");
        let oversized = read_linked_file(Path::new("big.txt"), dir.path()).await;
        assert_eq!(oversized, "file:big.txt");
    }
}
