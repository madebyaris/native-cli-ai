//! JSON-RPC method handlers and the session registry.

use crate::history;
use crate::options;
use crate::session::{SessionSlot, TurnError};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, AvailableCommandsUpdate, CancelNotification, CurrentModeUpdate,
    DeleteSessionRequest, DeleteSessionResponse, Implementation, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest,
    PromptResponse, SessionCapabilities, SessionConfigOption, SessionDeleteCapabilities, SessionId,
    SessionInfo, SessionListCapabilities, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse,
};
use agent_client_protocol::{Client, ConnectionTo, Error, Responder};
use nca_common::config::{NcaConfig, PermissionMode};
use nca_runtime::session_store::SessionStore;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::Mutex;

const DEFAULT_MAX_LIVE_SESSIONS: usize = 3;

/// How long a session response waits for the model catalog when none is saved
/// yet (the first run of the day). Clients such as T3 Code read the model list
/// from this response only, so a short wait here beats a wrong picker. With a
/// saved list the response does not wait at all.
const CATALOG_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

pub struct AcpOptions {
    pub model: Option<String>,
    pub safe: bool,
    pub permission_mode: Option<PermissionMode>,
}

pub struct AcpServer {
    base_config: NcaConfig,
    model_override: Option<String>,
    permission_override: Option<PermissionMode>,
    safe: bool,
    sessions: Mutex<HashMap<String, Arc<SessionSlot>>>,
    known_sessions: Mutex<HashMap<String, PathBuf>>,
    max_live: usize,
    elicitation: Arc<AtomicBool>,
    supports_compaction: Arc<AtomicBool>,
}

impl AcpServer {
    pub fn new(mut config: NcaConfig, options: AcpOptions) -> Self {
        if let Some(model) = options.model.as_deref() {
            config.apply_model_override(model);
        }
        if let Some(mode) = options.permission_mode {
            config.permissions.mode = mode;
        }
        let max_live = std::env::var("NCA_ACP_MAX_LIVE_SESSIONS")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_LIVE_SESSIONS);
        Self {
            base_config: config,
            model_override: options.model,
            permission_override: options.permission_mode,
            safe: options.safe,
            sessions: Mutex::new(HashMap::new()),
            known_sessions: Mutex::new(HashMap::new()),
            max_live,
            elicitation: Arc::new(AtomicBool::new(false)),
            supports_compaction: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Config for one workspace: the workspace's own `.nca/config.toml` layered
    /// over the global file, with the command-line overrides re-applied.
    fn config_for(&self, cwd: &Path) -> NcaConfig {
        let mut config = match NcaConfig::load_for_workspace(cwd) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(error = %error, workspace = %cwd.display(), "workspace config load failed; using startup config");
                self.base_config.clone()
            }
        };
        if let Some(model) = self.model_override.as_deref() {
            config.apply_model_override(model);
        }
        if let Some(mode) = self.permission_override {
            config.permissions.mode = mode;
        }
        config
    }

    pub async fn initialize(
        &self,
        request: InitializeRequest,
        responder: Responder<InitializeResponse>,
    ) -> Result<(), Error> {
        if let Some(client) = request.client_info.as_ref() {
            crate::translate::set_client_name(&client.name);
        }
        let supports_elicitation = request
            .client_capabilities
            .elicitation
            .as_ref()
            .is_some_and(|capabilities| capabilities.supports_form());
        self.elicitation
            .store(supports_elicitation, Ordering::SeqCst);
        let supports_compaction = request
            .client_capabilities
            .session
            .as_ref()
            .and_then(|session| session.compaction.as_ref())
            .is_some();
        self.supports_compaction
            .store(supports_compaction, Ordering::SeqCst);

        // Warm the provider's model catalog while the client sets up, so the
        // first `session/new` can list the provider's models right away.
        let warm = self.base_config.clone();
        tokio::spawn(async move {
            nca_runtime::model_limits_api::fetch_provider_model_ids(&warm).await;
        });

        let capabilities = AgentCapabilities::new()
            .load_session(true)
            .prompt_capabilities(PromptCapabilities::new().image(true).embedded_context(true))
            .session_capabilities(
                SessionCapabilities::new()
                    .list(Some(SessionListCapabilities::new()))
                    .delete(Some(SessionDeleteCapabilities::new())),
            );

        let response = InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(capabilities)
            .agent_info(Some(
                Implementation::new("nca", env!("CARGO_PKG_VERSION")).title("nca"),
            ));
        responder.respond(response)
    }

    pub async fn new_session(
        &self,
        request: NewSessionRequest,
        responder: Responder<NewSessionResponse>,
        cx: ConnectionTo<Client>,
    ) -> Result<(), Error> {
        if !request.mcp_servers.is_empty() {
            tracing::warn!(
                count = request.mcp_servers.len(),
                "client mcpServers are not connected; nca uses its own MCP config"
            );
        }
        match self.create_session(request.cwd, cx.clone()).await {
            Ok(slot) => {
                let (_, _, mode) = slot.config_snapshot().await;
                slot.load_model_catalog(CATALOG_WAIT).await;
                let response = NewSessionResponse::new(slot.id.clone())
                    .modes(options::session_modes(mode))
                    .config_options(slot.config_options().await);
                responder.respond(response)?;
                send_available_commands(&cx, &slot.id);
                slot.refresh_model_catalog();
                Ok(())
            }
            Err(message) => responder.respond_with_internal_error(message),
        }
    }

    pub async fn prompt(
        &self,
        request: PromptRequest,
        responder: Responder<PromptResponse>,
        cx: ConnectionTo<Client>,
    ) -> Result<(), Error> {
        let Some(slot) = self.session(&request.session_id).await else {
            return responder.respond_with_internal_error("unknown session");
        };
        if slot
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return responder
                .respond_with_internal_error("another prompt is already running on this session");
        }
        slot.touch();

        let prompt = request.prompt;
        cx.spawn(async move {
            let result = slot.run_prompt(prompt).await;
            slot.busy.store(false, Ordering::SeqCst);
            match result {
                Ok(stop_reason) => {
                    let _ = responder.respond(PromptResponse::new(stop_reason));
                }
                Err(TurnError {
                    message,
                    stop_reason: Some(stop_reason),
                }) => {
                    let _ = responder.respond(PromptResponse::new(stop_reason));
                    tracing::warn!("turn ended early: {message}");
                }
                Err(TurnError { message, .. }) => {
                    let _ = responder.respond_with_internal_error(message);
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    pub async fn cancel(&self, notification: CancelNotification) -> Result<(), Error> {
        if let Some(slot) = self.session(&notification.session_id).await {
            slot.cancel().await;
        }
        Ok(())
    }

    pub async fn load_session(
        &self,
        request: LoadSessionRequest,
        responder: Responder<LoadSessionResponse>,
        cx: ConnectionTo<Client>,
    ) -> Result<(), Error> {
        let session_id = request.session_id.clone();
        match self.load(request, cx.clone()).await {
            Ok((config, model, mode)) => {
                let slot = self.session(&session_id).await;
                if let Some(slot) = &slot {
                    slot.load_model_catalog(CATALOG_WAIT).await;
                }
                let config_options = match &slot {
                    Some(slot) => slot.config_options().await,
                    None => options::config_options(&config, &model, mode, &[]),
                };
                let response = LoadSessionResponse::new()
                    .modes(options::session_modes(mode))
                    .config_options(config_options);
                responder.respond(response)?;
                if let Some(slot) = slot {
                    send_available_commands(&cx, &slot.id);
                    slot.refresh_model_catalog();
                }
                Ok(())
            }
            Err(message) => responder.respond_with_internal_error(message),
        }
    }

    async fn load(
        &self,
        request: LoadSessionRequest,
        cx: ConnectionTo<Client>,
    ) -> Result<(NcaConfig, String, PermissionMode), String> {
        let cwd = canonicalize(&request.cwd)?;
        let session_id = request.session_id.0.to_string();
        let store = self.store(&cwd);
        let state = store
            .load(&session_id)
            .await
            .map_err(|e| format!("cannot load session {session_id}: {e}"))?;
        if canonicalize(&state.meta.workspace)? != cwd {
            return Err(format!(
                "session {session_id} belongs to {}, not {}",
                state.meta.workspace.display(),
                cwd.display()
            ));
        }

        let slot = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .entry(session_id.clone())
                .or_insert_with(|| {
                    let config = self.config_for(&cwd);
                    SessionSlot::new(
                        session_id.clone(),
                        cwd.clone(),
                        config,
                        self.safe,
                        cx.clone(),
                        self.elicitation.clone(),
                        self.supports_compaction.load(Ordering::SeqCst),
                    )
                })
                .clone()
        };
        self.known_sessions
            .lock()
            .await
            .insert(session_id.clone(), cwd.clone());

        let log_path = self
            .store(&cwd)
            .sessions_dir()
            .join(format!("{session_id}.events.jsonl"));
        let last_id = history::replay(&log_path, &request.session_id, &cx, &cwd).await?;
        slot.resume_existing(last_id).await?;
        slot.touch();
        self.enforce_capacity(&slot.id).await;

        let (config, model, mode) = slot.config_snapshot().await;
        Ok((config, model, mode))
    }

    pub async fn list_sessions(
        &self,
        request: ListSessionsRequest,
        responder: Responder<ListSessionsResponse>,
    ) -> Result<(), Error> {
        match self.list(request).await {
            Ok(sessions) => responder.respond(ListSessionsResponse::new(sessions)),
            Err(message) => responder.respond_with_internal_error(message),
        }
    }

    async fn list(&self, request: ListSessionsRequest) -> Result<Vec<SessionInfo>, String> {
        let cwd = match request.cwd {
            Some(cwd) => canonicalize(&cwd)?,
            None => canonicalize(&std::env::current_dir().map_err(|e| e.to_string())?)?,
        };
        let store = self.store(&cwd);
        let ids = store.list().await.map_err(|e| e.to_string())?;
        let mut sessions = Vec::new();
        for id in ids {
            let Ok(snapshot) = store.load_snapshot(&id).await else {
                continue;
            };
            if canonicalize(&snapshot.workspace).ok() != Some(cwd.clone()) {
                continue;
            }
            sessions.push((
                snapshot.updated_at,
                SessionInfo::new(snapshot.id, cwd.clone())
                    .title(snapshot.title)
                    .updated_at(Some(snapshot.updated_at.to_rfc3339())),
            ));
        }
        {
            let mut known = self.known_sessions.lock().await;
            for (_, info) in &sessions {
                known.insert(info.session_id.0.to_string(), cwd.clone());
            }
        }
        sessions.sort_by_key(|(updated_at, _)| std::cmp::Reverse(*updated_at));
        Ok(sessions
            .into_iter()
            .map(|(_, info)| info)
            .take(100)
            .collect())
    }

    pub async fn set_mode(
        &self,
        request: SetSessionModeRequest,
        responder: Responder<SetSessionModeResponse>,
        cx: ConnectionTo<Client>,
    ) -> Result<(), Error> {
        let Some(mode) = options::permission_mode_for(request.mode_id.0.as_ref()) else {
            return responder
                .respond_with_internal_error(format!("unknown mode `{}`", request.mode_id.0));
        };
        let Some(slot) = self.session(&request.session_id).await else {
            return responder.respond_with_internal_error("unknown session");
        };
        if let Err(message) = slot.set_mode(mode).await {
            return responder.respond_with_internal_error(message);
        }
        let _ = cx.send_notification(SessionNotification::new(
            request.session_id.clone(),
            agent_client_protocol::schema::v1::SessionUpdate::CurrentModeUpdate(
                CurrentModeUpdate::new(request.mode_id.clone()),
            ),
        ));
        responder.respond(SetSessionModeResponse::new())
    }

    pub async fn set_config_option(
        &self,
        request: SetSessionConfigOptionRequest,
        responder: Responder<SetSessionConfigOptionResponse>,
        cx: ConnectionTo<Client>,
    ) -> Result<(), Error> {
        let Some(slot) = self.session(&request.session_id).await else {
            return responder.respond_with_internal_error("unknown session");
        };
        let Some(value) = request.value.as_value_id().map(|id| id.0.to_string()) else {
            return responder.respond_with_internal_error("expected a string option value");
        };

        let result = match request.config_id.0.as_ref() {
            "mode" => match options::permission_mode_for(&value) {
                Some(mode) => slot.set_mode(mode).await,
                None => Err(format!("unknown mode `{value}`")),
            },
            "model" => slot.set_model(&value).await,
            other => Err(format!("unknown config option `{other}`")),
        };
        if let Err(message) = result {
            return responder.respond_with_internal_error(message);
        }

        let config_options: Vec<SessionConfigOption> = slot.config_options().await;
        let _ = cx.send_notification(SessionNotification::new(
            request.session_id.clone(),
            agent_client_protocol::schema::v1::SessionUpdate::ConfigOptionUpdate(
                agent_client_protocol::schema::v1::ConfigOptionUpdate::new(config_options.clone()),
            ),
        ));
        responder.respond(SetSessionConfigOptionResponse::new(config_options))
    }

    async fn create_session(
        &self,
        cwd: PathBuf,
        cx: ConnectionTo<Client>,
    ) -> Result<Arc<SessionSlot>, String> {
        let cwd = canonicalize(&cwd)?;
        let id = new_session_id();
        let config = self.config_for(&cwd);
        let slot = SessionSlot::new(
            id,
            cwd.clone(),
            config,
            self.safe,
            cx,
            self.elicitation.clone(),
            self.supports_compaction.load(Ordering::SeqCst),
        );
        slot.ensure_supervisor().await?;
        slot.touch();

        {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(slot.id.clone(), slot.clone());
        }
        self.known_sessions
            .lock()
            .await
            .insert(slot.id.clone(), cwd);
        self.enforce_capacity(&slot.id).await;
        Ok(slot)
    }

    /// Remove a session from disk and from the live registry. The client must
    /// call `session/list` first so the server knows the workspace.
    pub async fn delete_session(
        &self,
        request: DeleteSessionRequest,
        responder: Responder<DeleteSessionResponse>,
    ) -> Result<(), Error> {
        let id = request.session_id.0.to_string();
        if let Some(slot) = self.session(&request.session_id).await
            && slot.busy.load(Ordering::SeqCst)
        {
            return responder
                .respond_with_internal_error("cannot delete a session while a prompt is running");
        }

        let slot = self.sessions.lock().await.remove(&id);
        let cwd = match &slot {
            Some(slot) => Some(slot.cwd.clone()),
            None => match self.known_sessions.lock().await.get(&id).cloned() {
                Some(cwd) => Some(cwd),
                None => find_session_workspace(&id),
            },
        };
        let Some(cwd) = cwd else {
            return responder.respond_with_internal_error(format!(
                "unknown session `{id}`; list sessions before deleting"
            ));
        };

        if let Some(slot) = slot {
            slot.discard().await;
        }
        let result = self.store(&cwd).delete(&id).await;
        self.known_sessions.lock().await.remove(&id);
        match result {
            Ok(()) => responder.respond(DeleteSessionResponse::new()),
            Err(message) => responder.respond_with_internal_error(message.to_string()),
        }
    }

    /// Keep the number of live supervisors bounded. Evicted sessions keep their
    /// slot and resume from disk on the next prompt.
    async fn enforce_capacity(&self, keep: &str) {
        let slots: Vec<Arc<SessionSlot>> = {
            let sessions = self.sessions.lock().await;
            sessions.values().cloned().collect()
        };

        let mut live = Vec::new();
        for slot in &slots {
            match slot.supervisor.try_lock() {
                Ok(guard) if guard.is_some() => {
                    live.push((slot.last_used.load(Ordering::Relaxed), slot.clone()));
                }
                Err(_) => live.push((slot.last_used.load(Ordering::Relaxed), slot.clone())),
                _ => {}
            }
        }
        if live.len() <= self.max_live {
            return;
        }

        live.sort_by_key(|(used, _)| *used);
        let mut remaining = live.len();
        for (_, slot) in live {
            if remaining <= self.max_live {
                break;
            }
            if slot.id == keep || slot.busy.load(Ordering::SeqCst) {
                continue;
            }
            slot.evict().await;
            remaining -= 1;
        }
    }

    async fn session(&self, id: &SessionId) -> Option<Arc<SessionSlot>> {
        self.sessions.lock().await.get(id.0.as_ref()).cloned()
    }

    fn store(&self, cwd: &Path) -> SessionStore {
        let config = self.config_for(cwd);
        SessionStore::new(nca_common::config::resolve_sessions_dir(&config, cwd))
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|e| format!("invalid workspace path {}: {e}", path.display()))
}

fn send_available_commands(cx: &ConnectionTo<Client>, session_id: &str) {
    let _ = cx.send_notification(SessionNotification::new(
        SessionId::new(session_id.to_string()),
        SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(
            options::available_commands(),
        )),
    ));
}

/// `session/delete` has no `cwd`. If the client did not list or load the
/// session first, look through the per-workspace session folders under the
/// product home. Custom absolute history dirs are not covered; those clients
/// must list first.
fn find_session_workspace(session_id: &str) -> Option<PathBuf> {
    let home = nca_common::config::nca_product_home()?;
    let entries = std::fs::read_dir(home.join("workspaces")).ok()?;
    for entry in entries.flatten() {
        let path = entry
            .path()
            .join("sessions")
            .join(format!("{session_id}.json"));
        let Ok(json) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<nca_common::session::SessionState>(&json) else {
            continue;
        };
        return Some(state.meta.workspace);
    }
    None
}

fn new_session_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "acp-{}-{}-{}",
        chrono::Utc::now().timestamp_micros(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
