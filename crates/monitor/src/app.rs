use crate::controller::LiveAttachController;
use crate::workspaces::WorkspaceManager;
use eframe::egui;
use nca_common::config::{NcaConfig, PermissionMode, ProviderKind};
use nca_common::event::{AgentCommand, AgentEvent, EndReason};
use nca_common::message::{Message, Role};
use nca_common::session::{SessionMeta, SessionStatus};
use nca_runtime::service::{
    ServiceSessionHandle, ServiceSessionInfo, ServiceSessionKind, ServiceSessionRequest,
};
use nca_runtime::session_store::SessionStore;
use rfd::FileDialog;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Projects,
    Chat,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsScope {
    Global,
    Project,
}

#[derive(Debug, Clone)]
struct ComposerState {
    prompt: String,
    model: String,
    safe_mode: bool,
    permission_mode: PermissionMode,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            model: String::new(),
            safe_mode: false,
            permission_mode: PermissionMode::AcceptEdits,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatRole {
    User,
    Assistant,
    Tool,
    Error,
}

#[derive(Debug, Clone)]
struct ChatEntry {
    role: ChatRole,
    title: String,
    content: String,
}

#[derive(Debug, Clone)]
struct PendingApproval {
    call_id: String,
    tool: String,
    description: String,
}

struct ActiveSession {
    _service_handle: Option<ServiceSessionHandle>,
    controller: LiveAttachController,
    info: ServiceSessionInfo,
    transcript: Vec<ChatEntry>,
    pending_approvals: Vec<PendingApproval>,
    composer: String,
    streaming_assistant: String,
    last_error: Option<String>,
    run_in_progress: bool,
    ended: Option<EndReason>,
}

impl ActiveSession {
    fn from_loaded(
        info: ServiceSessionInfo,
        controller: LiveAttachController,
        service_handle: Option<ServiceSessionHandle>,
        transcript: Vec<ChatEntry>,
    ) -> Self {
        Self {
            _service_handle: service_handle,
            controller,
            info,
            transcript,
            pending_approvals: Vec::new(),
            composer: String::new(),
            streaming_assistant: String::new(),
            last_error: None,
            run_in_progress: false,
            ended: None,
        }
    }

    fn push_user(&mut self, content: String) {
        self.transcript.push(ChatEntry {
            role: ChatRole::User,
            title: "You".into(),
            content,
        });
    }

    fn push_assistant(&mut self, content: String) {
        self.streaming_assistant.clear();
        self.transcript.push(ChatEntry {
            role: ChatRole::Assistant,
            title: "Assistant".into(),
            content,
        });
    }

    fn push_tool(&mut self, content: String) {
        self.transcript.push(ChatEntry {
            role: ChatRole::Tool,
            title: "Tool".into(),
            content,
        });
    }

    fn push_error(&mut self, content: String) {
        self.last_error = Some(content.clone());
        self.transcript.push(ChatEntry {
            role: ChatRole::Error,
            title: "Error".into(),
            content,
        });
    }
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        self.controller.stop();
    }
}

pub struct DesktopApp {
    workspace_mgr: WorkspaceManager,
    view: View,
    settings_scope: SettingsScope,
    global_settings: NcaConfig,
    project_settings: Option<NcaConfig>,
    composer: ComposerState,
    project_sessions: Vec<SessionMeta>,
    active_session: Option<ActiveSession>,
    status_message: Option<(String, bool, Instant)>,
}

impl DesktopApp {
    pub fn new() -> Self {
        let mut workspace_mgr = WorkspaceManager::load();
        workspace_mgr.sort_by_recent();
        if !workspace_mgr.workspaces.is_empty() {
            workspace_mgr.select(Some(0));
        }

        let global_settings = NcaConfig::load_global_file().unwrap_or_default();
        let mut app = Self {
            workspace_mgr,
            view: View::Projects,
            settings_scope: SettingsScope::Project,
            global_settings,
            project_settings: None,
            composer: ComposerState::default(),
            project_sessions: Vec::new(),
            active_session: None,
            status_message: None,
        };
        app.reload_selected_workspace_data();
        app
    }

    fn delete_session(&mut self, session_id: &str) {
        if let Some(workspace_root) = self.selected_workspace() {
            let config = self.effective_project_config();
            let sessions_dir = workspace_root.join(&config.session.history_dir);
            
            let _ = std::fs::remove_file(sessions_dir.join(format!("{}.json", session_id)));
            let _ = std::fs::remove_file(sessions_dir.join(format!("{}.events.jsonl", session_id)));
            let _ = std::fs::remove_file(sessions_dir.join(format!("{}.spawn.log", session_id)));
            
            self.reload_selected_workspace_data();
        }
    }

    fn set_status(&mut self, message: impl Into<String>, is_error: bool) {
        self.status_message = Some((message.into(), is_error, Instant::now()));
    }

    fn selected_workspace(&self) -> Option<PathBuf> {
        self.workspace_mgr.selected_path().cloned()
    }

    fn effective_project_config(&self) -> NcaConfig {
        self.project_settings
            .clone()
            .or_else(|| {
                self.selected_workspace()
                    .and_then(|path| NcaConfig::load_for_workspace(&path).ok())
            })
            .unwrap_or_else(|| self.global_settings.clone())
    }

    fn reload_selected_workspace_data(&mut self) {
        let selected = self.selected_workspace();
        self.project_settings = selected
            .as_ref()
            .and_then(|path| NcaConfig::load_for_workspace(path).ok());

        self.project_sessions = selected
            .as_ref()
            .map(|path| load_session_metas(path, &self.effective_project_config()))
            .unwrap_or_default();

        let config = self.effective_project_config();
        if self.composer.model.is_empty() {
            self.composer.model = config.model.default_model;
        }
        self.composer.permission_mode = config.permissions.mode;
    }

    fn open_project_dialog(&mut self) {
        if let Some(path) = FileDialog::new().pick_folder() {
            self.workspace_mgr.add_workspace(path.clone());
            if let Some(idx) = self.workspace_mgr.workspaces.iter().position(|w| w.path == path) {
                self.workspace_mgr.sort_by_recent();
                let selected_idx = self
                    .workspace_mgr
                    .workspaces
                    .iter()
                    .position(|w| w.path == path)
                    .or(Some(idx));
                self.workspace_mgr.select(selected_idx);
            }
            self.reload_selected_workspace_data();
            self.view = View::Projects;
        }
    }

    fn start_new_session(&mut self) {
        let Some(workspace_root) = self.selected_workspace() else {
            self.set_status("Pick a project folder first.", true);
            return;
        };
        if self.composer.prompt.trim().is_empty() {
            self.set_status("Enter a prompt before starting a chat.", true);
            return;
        }

        let mut config = self.effective_project_config();
        let model = if self.composer.model.trim().is_empty() {
            config.model.default_model.clone()
        } else {
            self.composer.model.trim().to_string()
        };
        config.model.default_model = model.clone();
        config.provider.minimax.model = model;
        config.permissions.mode = self.composer.permission_mode;

        match nca_runtime::service::spawn_service_session(ServiceSessionRequest {
            config,
            workspace_root: workspace_root.clone(),
            safe_mode: self.composer.safe_mode,
            initial_prompt: Some(self.composer.prompt.clone()),
            kind: ServiceSessionKind::New { session_id: None },
        }) {
            Ok(handle) => {
                let info = handle.info().clone();
                match attach_controller(&info) {
                    Ok(controller) => {
                        let mut session =
                            ActiveSession::from_loaded(info, controller, Some(handle), Vec::new());
                        session.run_in_progress = true;
                        self.active_session = Some(session);
                        self.composer.prompt.clear();
                        self.view = View::Chat;
                        self.reload_selected_workspace_data();
                    }
                    Err(error) => self.set_status(error, true),
                }
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn resume_or_attach_session(&mut self, meta: SessionMeta) {
        let transcript = load_transcript(&meta.workspace, &self.effective_project_config(), &meta.id);

        if meta.status == SessionStatus::Running {
            if let Some(socket_path) = meta.socket_path.clone() {
                let info = ServiceSessionInfo {
                    session_id: meta.id.clone(),
                    workspace_root: meta.workspace.clone(),
                    model: meta.model.clone(),
                    socket_path: Some(socket_path),
                    event_log_path: workspace_event_log_path(
                        &meta.workspace,
                        &self.effective_project_config(),
                        &meta.id,
                    ),
                };
                match attach_controller(&info) {
                    Ok(controller) => {
                        self.active_session = Some(ActiveSession::from_loaded(
                            info, controller, None, transcript,
                        ));
                        self.view = View::Chat;
                    }
                    Err(error) => self.set_status(error, true),
                }
                return;
            }
        }

        match nca_runtime::service::spawn_service_session(ServiceSessionRequest {
            config: self.effective_project_config(),
            workspace_root: meta.workspace.clone(),
            safe_mode: false,
            initial_prompt: None,
            kind: ServiceSessionKind::Resume {
                session_id: meta.id.clone(),
            },
        }) {
            Ok(handle) => {
                let info = handle.info().clone();
                match attach_controller(&info) {
                    Ok(controller) => {
                        self.active_session = Some(ActiveSession::from_loaded(
                            info,
                            controller,
                            Some(handle),
                            transcript,
                        ));
                        self.view = View::Chat;
                        self.reload_selected_workspace_data();
                    }
                    Err(error) => self.set_status(error, true),
                }
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn process_live_events(&mut self) {
        let Some(session) = self.active_session.as_mut() else {
            return;
        };

        let mut refresh_sessions = false;
        for event in session.controller.drain() {
            match event {
                AgentEvent::SessionStarted {
                    session_id, model, ..
                } => {
                    session.info.session_id = session_id;
                    session.info.model = model;
                    session.ended = None;
                    session.last_error = None;
                }
                AgentEvent::MessageReceived { role, content } => match role.as_str() {
                    "user" => {
                        session.push_user(content);
                        session.run_in_progress = true;
                    }
                    "assistant" => {
                        session.push_assistant(content);
                        session.run_in_progress = false;
                    }
                    _ => {}
                },
                AgentEvent::TokensStreamed { delta } => {
                    session.streaming_assistant.push_str(&delta);
                }
                AgentEvent::ToolCallStarted { tool, input, .. } => {
                    session.push_tool(format!("Started `{tool}` with {input}"));
                }
                AgentEvent::ToolCallCompleted { output, .. } => {
                    if output.success {
                        session.push_tool("Tool completed successfully.".into());
                    } else if let Some(error) = output.error {
                        session.push_error(error);
                    }
                }
                AgentEvent::ApprovalRequested {
                    call_id,
                    tool,
                    description,
                } => {
                    session.pending_approvals.push(PendingApproval {
                        call_id,
                        tool,
                        description,
                    });
                }
                AgentEvent::ApprovalResolved { call_id, approved } => {
                    session.pending_approvals.retain(|item| item.call_id != call_id);
                    if !approved {
                        session.push_error("Tool approval was denied.".into());
                    }
                }
                AgentEvent::SessionEnded { reason } => {
                    session.ended = Some(reason);
                    session.run_in_progress = false;
                    refresh_sessions = true;
                }
                AgentEvent::Error { message } => {
                    session.push_error(message);
                    session.run_in_progress = false;
                }
                AgentEvent::CostUpdated { .. }
                | AgentEvent::Checkpoint { .. }
                | AgentEvent::Response { .. }
                | AgentEvent::ChildSessionSpawned { .. }
                | AgentEvent::ChildSessionCompleted { .. } => {}
            }
        }

        if refresh_sessions {
            self.reload_selected_workspace_data();
        }
    }

    fn show_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("nca desktop");
                ui.separator();
                nav_button(ui, &mut self.view, View::Projects, "Projects");
                nav_button(ui, &mut self.view, View::Chat, "Chat");
                nav_button(ui, &mut self.view, View::Settings, "Settings");
                ui.separator();
                if ui.button("Open Project Folder").clicked() {
                    self.open_project_dialog();
                }
            });

            if let Some((message, is_error, at)) = &self.status_message {
                if at.elapsed() < Duration::from_secs(5) {
                    let color = if *is_error {
                        egui::Color32::from_rgb(220, 120, 120)
                    } else {
                        egui::Color32::from_rgb(120, 210, 150)
                    };
                    ui.colored_label(color, message);
                }
            }
        });
    }

    fn show_workspace_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("workspaces")
            .resizable(true)
            .min_width(240.0)
            .show(ctx, |ui| {
                ui.heading("Projects");
                ui.add_space(8.0);

                if self.workspace_mgr.workspaces.is_empty() {
                    ui.label("No project folders yet.");
                    return;
                }

                let entries: Vec<_> = self
                    .workspace_mgr
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(idx, item)| (idx, item.name.clone(), item.path.clone()))
                    .collect();

                let mut new_selection = None;
                let mut remove_idx = None;

                for (idx, name, path) in entries {
                    ui.horizontal(|ui| {
                        let selected = self.workspace_mgr.selected_workspace == Some(idx);
                        if ui
                            .selectable_label(selected, format!("{name}\n{}", path.display()))
                            .clicked()
                        {
                            new_selection = Some(idx);
                        }
                        if ui.small_button("Remove").clicked() {
                            remove_idx = Some(idx);
                        }
                    });
                    ui.add_space(4.0);
                }

                if let Some(idx) = new_selection {
                    self.workspace_mgr.select(Some(idx));
                    self.reload_selected_workspace_data();
                }

                if let Some(idx) = remove_idx {
                    self.workspace_mgr.remove_workspace(idx);
                    if self.workspace_mgr.selected_workspace.is_none()
                        && !self.workspace_mgr.workspaces.is_empty()
                    {
                        self.workspace_mgr.select(Some(0));
                    }
                    self.reload_selected_workspace_data();
                }
            });
    }

    fn show_projects_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(workspace_root) = self.selected_workspace() else {
                ui.heading("Pick a project to start");
                ui.label("Use the Open Project Folder button to choose a repository or workspace.");
                return;
            };

            ui.heading(
                workspace_root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Project"),
            );
            ui.label(workspace_root.display().to_string());
            ui.add_space(12.0);

            ui.group(|ui| {
                ui.heading("Start A New Chat");
                ui.label("The desktop is now the primary place to launch and continue sessions.");
                ui.add_space(8.0);
                ui.label("Prompt");
                ui.add(
                    egui::TextEdit::multiline(&mut self.composer.prompt)
                        .desired_rows(8)
                        .hint_text("Describe the task you want the agent to work on."),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Model");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.composer.model)
                            .hint_text("MiniMax-M2.5"),
                    );
                });
                ui.checkbox(&mut self.composer.safe_mode, "Safe mode (read-only)");
                permission_mode_combo(ui, &mut self.composer.permission_mode);
                ui.add_space(8.0);
                if ui.button("Start Chat").clicked() {
                    self.start_new_session();
                }
            });

            ui.add_space(16.0);
            ui.heading("Recent Sessions");
            if self.project_sessions.is_empty() {
                ui.label("No saved sessions for this project yet.");
                return;
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let sessions = self.project_sessions.clone();
                    let mut delete_session_id = None;

                    for meta in sessions {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}  {}", meta.id, session_status_label(&meta.status)));
                                ui.separator();
                                ui.label(meta.model.clone());
                            });
                            ui.label(format!("Updated {}", meta.updated_at));
                            ui.horizontal(|ui| {
                                let button = if meta.status == SessionStatus::Running {
                                    "Open Running Chat"
                                } else {
                                    "Resume In Desktop"
                                };
                                if ui.button(button).clicked() {
                                    self.resume_or_attach_session(meta.clone());
                                }
                                if ui.button("Delete").clicked() {
                                    delete_session_id = Some(meta.id.clone());
                                }
                            });
                        });
                        ui.add_space(8.0);
                    }

                    if let Some(id) = delete_session_id {
                        self.delete_session(&id);
                    }
                });
        });
    }

    fn show_chat_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(session) = self.active_session.as_mut() else {
                ui.heading("No Active Chat");
                ui.label("Start a new session from Projects or resume an existing one.");
                return;
            };

            ui.horizontal(|ui| {
                ui.heading(format!("Chat {}", session.info.session_id));
                ui.separator();
                ui.label(format!("Model {}", session.info.model));
            });
            ui.label(session.info.workspace_root.display().to_string());
            ui.add_space(12.0);

            if !session.pending_approvals.is_empty() {
                ui.group(|ui| {
                    ui.heading("Pending Tool Approvals");
                    let approvals = session.pending_approvals.clone();
                    for approval in approvals {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("{}: {}", approval.tool, approval.description));
                            if ui.button("Approve").clicked() {
                                session
                                    .controller
                                    .send_command(&AgentCommand::ApproveToolCall {
                                        call_id: approval.call_id.clone(),
                                    });
                            }
                            if ui.button("Deny").clicked() {
                                session.controller.send_command(&AgentCommand::DenyToolCall {
                                    call_id: approval.call_id.clone(),
                                });
                            }
                        });
                    }
                });
                ui.add_space(8.0);
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for item in &session.transcript {
                        render_chat_entry(ui, item);
                    }
                    if !session.streaming_assistant.is_empty() {
                        render_chat_entry(
                            ui,
                            &ChatEntry {
                                role: ChatRole::Assistant,
                                title: "Assistant".into(),
                                content: session.streaming_assistant.clone(),
                            },
                        );
                    }
                });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            ui.label("Follow-up prompt");
            ui.add(
                egui::TextEdit::multiline(&mut session.composer)
                    .desired_rows(5)
                    .hint_text("Continue the conversation from the desktop."),
            );

            ui.horizontal(|ui| {
                let can_send = !session.composer.trim().is_empty();
                if ui
                    .add_enabled(can_send, egui::Button::new("Send"))
                    .clicked()
                {
                    session.run_in_progress = true;
                    session.ended = None;
                    session.controller.send_command(&AgentCommand::SendMessage {
                        content: session.composer.clone(),
                    });
                    session.composer.clear();
                }

                if ui.button("Cancel Current Run").clicked() {
                    session.controller.send_command(&AgentCommand::Cancel);
                }
            });

            if let Some(reason) = &session.ended {
                ui.label(format!("Session ended: {:?}", reason));
            } else if session.run_in_progress {
                ui.label("Session is running...");
            }

            if let Some(error) = &session.last_error {
                ui.colored_label(egui::Color32::from_rgb(220, 120, 120), error);
            }
        });
    }

    fn show_settings_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                nav_button_scope(ui, &mut self.settings_scope, SettingsScope::Project, "Project");
                nav_button_scope(ui, &mut self.settings_scope, SettingsScope::Global, "Global");
            });
            ui.add_space(12.0);

            match self.settings_scope {
                SettingsScope::Global => {
                    ui.heading("Global Settings");
                    ui.label("Saved to `~/.nca/config.toml`.");
                    ui.add_space(8.0);
                    show_config_form(ui, &mut self.global_settings, false);
                    ui.add_space(8.0);
                    if ui.button("Save Global Settings").clicked() {
                        match self.global_settings.save_global() {
                            Ok(()) => self.set_status("Saved global settings.", false),
                            Err(error) => self.set_status(error.to_string(), true),
                        }
                    }
                }
                SettingsScope::Project => {
                    let Some(workspace_root) = self.selected_workspace() else {
                        ui.heading("Project Settings");
                        ui.label("Pick a project folder first.");
                        return;
                    };
                    ui.heading("Project Settings");
                    ui.label(workspace_root.display().to_string());
                    ui.label("Saved to `.nca/config.local.toml` for this project.");
                    ui.add_space(8.0);
                    if let Some(config) = self.project_settings.as_mut() {
                        show_config_form(ui, config, true);
                        ui.add_space(8.0);
                        let mut save_clicked = false;
                        let mut reset_clicked = false;
                        ui.horizontal(|ui| {
                            save_clicked = ui.button("Save Project Settings").clicked();
                            reset_clicked = ui.button("Reset Project Overrides").clicked();
                        });

                        if save_clicked {
                            match config.save_workspace_file(&workspace_root) {
                                Ok(()) => {
                                    self.set_status("Saved project settings.", false);
                                    self.reload_selected_workspace_data();
                                }
                                Err(error) => self.set_status(error.to_string(), true),
                            }
                        }
                        if reset_clicked {
                            match NcaConfig::clear_workspace_file(&workspace_root) {
                                Ok(()) => {
                                    self.set_status("Removed project override file.", false);
                                    self.reload_selected_workspace_data();
                                }
                                Err(error) => self.set_status(error.to_string(), true),
                            }
                        }
                    }
                }
            }
        });
    }
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_live_events();
        self.show_top_bar(ctx);
        self.show_workspace_sidebar(ctx);

        match self.view {
            View::Projects => self.show_projects_view(ctx),
            View::Chat => self.show_chat_view(ctx),
            View::Settings => self.show_settings_view(ctx),
        }

        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn attach_controller(info: &ServiceSessionInfo) -> Result<LiveAttachController, String> {
    let socket_path = info
        .socket_path
        .clone()
        .ok_or_else(|| "session did not expose a socket path".to_string())?;
    Ok(LiveAttachController::attach(socket_path))
}

fn load_session_metas(workspace_root: &Path, config: &NcaConfig) -> Vec<SessionMeta> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let Ok(rt) = runtime else {
        return Vec::new();
    };

    let store = SessionStore::new(workspace_root.join(&config.session.history_dir));
    let mut sessions = Vec::new();
    if let Ok(ids) = rt.block_on(store.list()) {
        for id in ids {
            if let Ok(state) = rt.block_on(store.load(&id)) {
                sessions.push(state.meta);
            }
        }
    }
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    sessions
}

fn load_transcript(workspace_root: &Path, config: &NcaConfig, session_id: &str) -> Vec<ChatEntry> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let Ok(rt) = runtime else {
        return Vec::new();
    };

    let store = SessionStore::new(workspace_root.join(&config.session.history_dir));
    let Ok(state) = rt.block_on(store.load(session_id)) else {
        return Vec::new();
    };

    state
        .messages
        .iter()
        .filter_map(message_to_chat_entry)
        .collect()
}

fn workspace_event_log_path(workspace_root: &Path, config: &NcaConfig, session_id: &str) -> PathBuf {
    workspace_root
        .join(&config.session.history_dir)
        .join(format!("{session_id}.events.jsonl"))
}

fn message_to_chat_entry(message: &Message) -> Option<ChatEntry> {
    match message.role {
        Role::User => Some(ChatEntry {
            role: ChatRole::User,
            title: "You".into(),
            content: message.content.clone(),
        }),
        Role::Assistant => Some(ChatEntry {
            role: ChatRole::Assistant,
            title: "Assistant".into(),
            content: message.content.clone(),
        }),
        Role::Tool => Some(ChatEntry {
            role: ChatRole::Tool,
            title: "Tool".into(),
            content: message.content.clone(),
        }),
        Role::System => None,
    }
}

fn nav_button(ui: &mut egui::Ui, selected: &mut View, value: View, label: &str) {
    if ui.selectable_label(*selected == value, label).clicked() {
        *selected = value;
    }
}

fn nav_button_scope(
    ui: &mut egui::Ui,
    selected: &mut SettingsScope,
    value: SettingsScope,
    label: &str,
) {
    if ui.selectable_label(*selected == value, label).clicked() {
        *selected = value;
    }
}

fn render_chat_entry(ui: &mut egui::Ui, item: &ChatEntry) {
    let (fill, title_color) = match item.role {
        ChatRole::User => (
            egui::Color32::from_rgb(36, 54, 84),
            egui::Color32::from_rgb(180, 220, 255),
        ),
        ChatRole::Assistant => (
            egui::Color32::from_rgb(36, 66, 52),
            egui::Color32::from_rgb(176, 235, 188),
        ),
        ChatRole::Tool => (
            egui::Color32::from_rgb(58, 58, 58),
            egui::Color32::from_rgb(220, 220, 220),
        ),
        ChatRole::Error => (
            egui::Color32::from_rgb(90, 40, 40),
            egui::Color32::from_rgb(255, 190, 190),
        ),
    };

    egui::Frame::group(ui.style())
        .fill(fill)
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.colored_label(title_color, &item.title);
            ui.label(&item.content);
        });
    ui.add_space(6.0);
}

fn show_config_form(ui: &mut egui::Ui, config: &mut NcaConfig, is_project: bool) {
    ui.horizontal(|ui| {
        ui.label("Provider");
        egui::ComboBox::from_id_salt(("provider", is_project))
            .selected_text(provider_label(&config.provider.default))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut config.provider.default,
                    ProviderKind::MiniMax,
                    "MiniMax",
                );
                ui.add_enabled_ui(false, |ui| {
                    let _ = ui.selectable_label(false, "OpenRouter (coming soon)");
                    let _ = ui.selectable_label(false, "Anthropic (coming soon)");
                    let _ = ui.selectable_label(false, "OpenAI (coming soon)");
                });
            });
    });
    ui.add_space(4.0);
    ui.label("MiniMax API key");
    ui.add(
        egui::TextEdit::singleline(
            config.provider.minimax.api_key.get_or_insert_with(String::new),
        )
        .password(true),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("API key env var");
        ui.text_edit_singleline(&mut config.provider.minimax.api_key_env);
    });
    ui.horizontal(|ui| {
        ui.label("Base URL");
        ui.text_edit_singleline(&mut config.provider.minimax.base_url);
    });
    ui.horizontal(|ui| {
        ui.label("Default model");
        ui.text_edit_singleline(&mut config.model.default_model);
    });
    config.provider.minimax.model = config.model.default_model.clone();
    permission_mode_combo(ui, &mut config.permissions.mode);
    ui.add_space(4.0);
    ui.label("Only MiniMax is implemented right now. Other providers stay disabled until their runtime support lands.");
}

fn provider_label(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::MiniMax => "MiniMax",
        ProviderKind::OpenRouter => "OpenRouter",
        ProviderKind::Anthropic => "Anthropic",
        ProviderKind::OpenAi => "OpenAI",
    }
}

fn permission_mode_combo(ui: &mut egui::Ui, mode: &mut PermissionMode) {
    egui::ComboBox::from_label("Permission mode")
        .selected_text(permission_label(*mode))
        .show_ui(ui, |ui| {
            for candidate in [
                PermissionMode::AcceptEdits,
                PermissionMode::Default,
                PermissionMode::Plan,
                PermissionMode::DontAsk,
                PermissionMode::BypassPermissions,
            ] {
                ui.selectable_value(mode, candidate, permission_label(candidate));
            }
        });
}

fn permission_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "Default",
        PermissionMode::Plan => "Plan (read-only)",
        PermissionMode::AcceptEdits => "Accept edits",
        PermissionMode::DontAsk => "Don't ask",
        PermissionMode::BypassPermissions => "Bypass permissions",
    }
}

fn session_status_label(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "running",
        SessionStatus::Completed => "completed",
        SessionStatus::Error => "error",
        SessionStatus::Cancelled => "cancelled",
    }
}
