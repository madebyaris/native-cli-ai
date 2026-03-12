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

// ---------------------------------------------------------------------------
// Color palette — matches the dark developer aesthetic from the HTML templates
// ---------------------------------------------------------------------------
mod palette {
    use eframe::egui::Color32;

    pub const BG: Color32 = Color32::from_rgb(10, 10, 10); // #0a0a0a
    pub const SIDEBAR: Color32 = Color32::from_rgb(17, 17, 17); // #111111
    pub const CARD: Color32 = Color32::from_rgb(26, 26, 26); // #1a1a1a
    pub const BORDER: Color32 = Color32::from_rgb(45, 45, 45); // #2d2d2d
    pub const ACCENT: Color32 = Color32::from_rgb(0, 112, 243); // #0070f3
    pub const TEXT_DIM: Color32 = Color32::from_rgb(136, 136, 136); // #888888
    pub const TEXT: Color32 = Color32::from_rgb(220, 220, 220);
    pub const WHITE: Color32 = Color32::from_rgb(240, 240, 240);
    pub const SUCCESS: Color32 = Color32::from_rgb(16, 185, 129); // #10b981
    pub const WARNING: Color32 = Color32::from_rgb(245, 158, 11); // #f59e0b
    pub const ERROR: Color32 = Color32::from_rgb(239, 68, 68);
    #[allow(dead_code)]
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(0, 112, 243);
    pub const ACCENT_BG: Color32 = Color32::from_rgb(10, 22, 40); // accent at ~10%
    pub const USER_BUBBLE: Color32 = Color32::from_rgb(0, 90, 200); // slightly muted accent
    pub const ASSISTANT_BUBBLE: Color32 = Color32::from_rgb(30, 30, 30);
    pub const TOOL_BUBBLE: Color32 = Color32::from_rgb(20, 20, 20);
    pub const ERROR_BUBBLE: Color32 = Color32::from_rgb(50, 20, 20);
    pub const INPUT_BG: Color32 = Color32::from_rgb(22, 22, 22);
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

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
            title: "Developer".into(),
            content,
        });
    }

    fn push_assistant(&mut self, content: String) {
        self.streaming_assistant.clear();
        if !content.trim().is_empty() {
            self.transcript.push(ChatEntry {
                role: ChatRole::Assistant,
                title: "Orchestrator".into(),
                content,
            });
        }
    }

    fn push_tool(&mut self, content: String) {
        self.transcript.push(ChatEntry {
            role: ChatRole::Tool,
            title: "System".into(),
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

// ---------------------------------------------------------------------------
// Main app
// ---------------------------------------------------------------------------

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
            let _ =
                std::fs::remove_file(sessions_dir.join(format!("{}.events.jsonl", session_id)));
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
            self.workspace_mgr.sort_by_recent();
            let selected_idx = self
                .workspace_mgr
                .workspaces
                .iter()
                .position(|w| w.path == path);
            self.workspace_mgr.select(selected_idx);
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
                    Err(e) => self.set_status(e, true),
                }
            }
            Err(e) => self.set_status(e, true),
        }
    }

    fn resume_or_attach_session(&mut self, meta: SessionMeta) {
        let transcript =
            load_transcript(&meta.workspace, &self.effective_project_config(), &meta.id);
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
                    Err(e) => self.set_status(e, true),
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
                    Err(e) => self.set_status(e, true),
                }
            }
            Err(e) => self.set_status(e, true),
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
                        if !content.trim().is_empty() {
                            session.push_user(content);
                        }
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
                    session.push_tool(format!("[exec] {tool} {input}"));
                }
                AgentEvent::ToolCallCompleted { output, .. } => {
                    if output.success {
                        session.push_tool("[done] ok".into());
                    } else if let Some(e) = output.error {
                        session.push_error(e);
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
                    session
                        .pending_approvals
                        .retain(|a| a.call_id != call_id);
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

    // -----------------------------------------------------------------------
    // Sidebar — dark, compact, matches dashboard1.html
    // -----------------------------------------------------------------------
    fn show_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(250.0)
            .frame(
                egui::Frame::none()
                    .fill(palette::SIDEBAR)
                    .inner_margin(egui::Margin::same(0.0)),
            )
            .show(ctx, |ui| {
                ui.style_mut().visuals.widgets.noninteractive.bg_fill = palette::SIDEBAR;

                // App header
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.colored_label(palette::WHITE, egui::RichText::new("nca desktop").strong().size(15.0));
                });
                ui.add_space(12.0);

                // Divider
                draw_separator(ui);

                // Nav links
                ui.add_space(8.0);
                let nav_items = [
                    (View::Projects, "Projects"),
                    (View::Chat, "Chat"),
                    (View::Settings, "Settings"),
                ];
                for (view, label) in nav_items {
                    let is_active = self.view == view;
                    let (bg, text_color) = if is_active {
                        (palette::ACCENT_BG, palette::ACCENT)
                    } else {
                        (egui::Color32::TRANSPARENT, palette::TEXT_DIM)
                    };
                    let resp = ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 32.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let rect = ui.max_rect();
                            ui.painter().rect_filled(
                                rect.shrink2(egui::vec2(8.0, 0.0)),
                                6.0,
                                bg,
                            );
                            if is_active {
                                ui.painter().rect_filled(
                                    egui::Rect::from_min_size(
                                        rect.left_top() + egui::vec2(8.0, 0.0),
                                        egui::vec2(3.0, rect.height()),
                                    ),
                                    2.0,
                                    palette::ACCENT,
                                );
                            }
                            ui.add_space(20.0);
                            ui.colored_label(
                                text_color,
                                egui::RichText::new(label).size(13.0).strong(),
                            );
                        },
                    );
                    if resp.response.interact(egui::Sense::click()).clicked() {
                        self.view = view;
                    }
                }

                ui.add_space(16.0);
                draw_separator(ui);
                ui.add_space(8.0);

                // "Your Projects" section header
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.colored_label(
                        palette::TEXT_DIM,
                        egui::RichText::new("YOUR PROJECTS").size(10.0).strong(),
                    );
                });
                ui.add_space(6.0);

                // Project list (scrollable)
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 52.0).max(20.0))
                    .show(ui, |ui| {
                        if self.workspace_mgr.workspaces.is_empty() {
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.colored_label(palette::TEXT_DIM, "No projects yet.");
                            });
                        }

                        let entries: Vec<_> = self
                            .workspace_mgr
                            .workspaces
                            .iter()
                            .enumerate()
                            .map(|(i, w)| (i, w.name.clone(), w.path.display().to_string()))
                            .collect();

                        let mut new_selection = None;
                        let mut remove_idx = None;

                        for (idx, name, path_str) in &entries {
                            let is_selected = self.workspace_mgr.selected_workspace == Some(*idx);
                            let (border_color, bg_color) = if is_selected {
                                (palette::ACCENT, palette::ACCENT_BG)
                            } else {
                                (palette::BORDER, palette::CARD)
                            };

                            let outer_rect = ui.available_rect_before_wrap();
                            let desired = egui::vec2((outer_rect.width() - 16.0).max(40.0), 48.0);

                            let resp = ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), 52.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add_space(8.0);
                                    let (rect, resp) = ui.allocate_exact_size(
                                        desired,
                                        egui::Sense::click(),
                                    );
                                    ui.painter().rect(
                                        rect,
                                        8.0,
                                        bg_color,
                                        egui::Stroke::new(1.0, border_color),
                                    );
                                    let text_color = if is_selected {
                                        palette::WHITE
                                    } else {
                                        palette::TEXT_DIM
                                    };
                                    ui.painter().text(
                                        rect.left_top() + egui::vec2(10.0, 8.0),
                                        egui::Align2::LEFT_TOP,
                                        name,
                                        egui::FontId::proportional(12.0),
                                        text_color,
                                    );
                                    ui.painter().text(
                                        rect.left_top() + egui::vec2(10.0, 26.0),
                                        egui::Align2::LEFT_TOP,
                                        truncate_path(path_str, 30),
                                        egui::FontId::proportional(10.0),
                                        palette::TEXT_DIM,
                                    );
                                    resp
                                },
                            );
                            if resp.inner.clicked() {
                                new_selection = Some(*idx);
                            }
                            if resp.inner.secondary_clicked() {
                                remove_idx = Some(*idx);
                            }
                            ui.add_space(2.0);
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

                // Bottom "Open Folder" button
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    draw_separator(ui);
                    ui.add_space(8.0);
                    let btn = egui::Button::new(
                        egui::RichText::new("Open Folder").size(12.0).color(palette::TEXT),
                    )
                    .fill(palette::CARD)
                    .stroke(egui::Stroke::new(1.0, palette::BORDER))
                    .rounding(6.0)
                    .min_size(egui::vec2(220.0, 34.0));
                    if ui.add(btn).clicked() {
                        self.open_project_dialog();
                    }
                    ui.add_space(4.0);
                });
            });
    }

    // -----------------------------------------------------------------------
    // Top header bar
    // -----------------------------------------------------------------------
    fn show_header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .exact_height(48.0)
            .frame(
                egui::Frame::none()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::symmetric(24.0, 0.0))
                    .stroke(egui::Stroke::new(1.0, palette::BORDER)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    // Breadcrumb
                    ui.colored_label(palette::TEXT_DIM, egui::RichText::new("Project /").size(12.0));
                    ui.add_space(4.0);
                    let project_name = self
                        .selected_workspace()
                        .and_then(|p| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_else(|| "none".into());
                    ui.colored_label(
                        palette::WHITE,
                        egui::RichText::new(project_name).size(13.0).strong(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Status indicator
                        if self
                            .active_session
                            .as_ref()
                            .map_or(false, |s| s.run_in_progress)
                        {
                            let dot = egui::RichText::new("●").size(10.0).color(palette::SUCCESS);
                            ui.label(dot);
                            ui.colored_label(
                                palette::TEXT_DIM,
                                egui::RichText::new("Agent Running").size(11.0),
                            );
                        } else {
                            let dot = egui::RichText::new("●").size(10.0).color(palette::TEXT_DIM);
                            ui.label(dot);
                            ui.colored_label(
                                palette::TEXT_DIM,
                                egui::RichText::new("Idle").size(11.0),
                            );
                        }

                        // Status toast
                        if let Some((msg, is_err, at)) = &self.status_message {
                            if at.elapsed() < Duration::from_secs(5) {
                                let c = if *is_err {
                                    palette::ERROR
                                } else {
                                    palette::SUCCESS
                                };
                                ui.add_space(16.0);
                                ui.colored_label(c, egui::RichText::new(msg).size(11.0));
                            }
                        }
                    });
                });
            });
    }

    // -----------------------------------------------------------------------
    // Projects view — composer + session cards
    // -----------------------------------------------------------------------
    fn show_projects_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::same(0.0)),
            )
            .show(ctx, |ui| {
                if self.selected_workspace().is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(
                            palette::TEXT_DIM,
                            egui::RichText::new("Select or open a project folder to begin.")
                                .size(16.0),
                        );
                    });
                    return;
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let max_w = 820.0_f32.min((ui.available_width() - 48.0).max(200.0));
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), ui.available_height()),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                ui.set_max_width(max_w);
                                ui.add_space(32.0);

                                // Section: Start a New Chat
                                ui.colored_label(
                                    palette::WHITE,
                                    egui::RichText::new("Start a New Chat").size(18.0).strong(),
                                );
                                ui.colored_label(
                                    palette::TEXT_DIM,
                                    egui::RichText::new(
                                        "Initialize an AI agent to perform tasks within your project directory.",
                                    )
                                    .size(12.0),
                                );
                                ui.add_space(12.0);

                                // Composer card
                                egui::Frame::none()
                                    .fill(palette::CARD)
                                    .rounding(12.0)
                                    .stroke(egui::Stroke::new(1.0, palette::BORDER))
                                    .inner_margin(egui::Margin::same(0.0))
                                    .show(ui, |ui| {
                                        // Textarea
                                        ui.add_space(4.0);
                                        egui::Frame::none()
                                            .inner_margin(egui::Margin::symmetric(20.0, 16.0))
                                            .show(ui, |ui| {
                                                ui.add(
                                                    egui::TextEdit::multiline(
                                                        &mut self.composer.prompt,
                                                    )
                                                    .font(egui::FontId::monospace(13.0))
                                                    .desired_rows(4)
                                                    .desired_width(f32::INFINITY)
                                                    .hint_text("Describe the task you want the agent to work on..."),
                                                );
                                            });

                                        // Config footer
                                        draw_separator(ui);
                                        egui::Frame::none()
                                            .fill(egui::Color32::from_rgba_premultiplied(0, 0, 0, 50))
                                            .inner_margin(egui::Margin::symmetric(20.0, 14.0))
                                            .show(ui, |ui| {
                                                ui.horizontal_wrapped(|ui| {
                                                    // Model
                                                    ui.vertical(|ui| {
                                                        ui.colored_label(
                                                            palette::TEXT_DIM,
                                                            egui::RichText::new("MODEL").size(9.0).strong(),
                                                        );
                                                        ui.add_space(2.0);
                                                        ui.add(
                                                            egui::TextEdit::singleline(
                                                                &mut self.composer.model,
                                                            )
                                                            .desired_width(180.0)
                                                            .hint_text("MiniMax-M2.5"),
                                                        );
                                                    });
                                                    ui.add_space(24.0);

                                                    // Permission mode
                                                    ui.vertical(|ui| {
                                                        ui.colored_label(
                                                            palette::TEXT_DIM,
                                                            egui::RichText::new("PERMISSION MODE")
                                                                .size(9.0)
                                                                .strong(),
                                                        );
                                                        ui.add_space(2.0);
                                                        permission_mode_combo(
                                                            ui,
                                                            &mut self.composer.permission_mode,
                                                        );
                                                    });
                                                    ui.add_space(24.0);

                                                    // Safe mode
                                                    ui.vertical(|ui| {
                                                        ui.add_space(14.0);
                                                        ui.checkbox(
                                                            &mut self.composer.safe_mode,
                                                            egui::RichText::new("Safe Mode (Read-only)")
                                                                .size(11.0)
                                                                .color(palette::TEXT_DIM),
                                                        );
                                                    });

                                                    // Launch button (right side)
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(egui::Align::Center),
                                                        |ui| {
                                                            let btn = egui::Button::new(
                                                                egui::RichText::new("Launch Agent")
                                                                    .size(13.0)
                                                                    .strong()
                                                                    .color(palette::WHITE),
                                                            )
                                                            .fill(palette::ACCENT)
                                                            .rounding(8.0)
                                                            .min_size(egui::vec2(130.0, 36.0));
                                                            if ui.add(btn).clicked() {
                                                                self.start_new_session();
                                                            }
                                                        },
                                                    );
                                                });
                                            });
                                    });

                                ui.add_space(32.0);

                                // Section: Recent Sessions
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        palette::WHITE,
                                        egui::RichText::new("Recent Sessions").size(16.0).strong(),
                                    );
                                });
                                ui.add_space(12.0);

                                if self.project_sessions.is_empty() {
                                    ui.colored_label(
                                        palette::TEXT_DIM,
                                        "No saved sessions for this project yet.",
                                    );
                                } else {
                                    let sessions = self.project_sessions.clone();
                                    let mut delete_id = None;

                                    for meta in &sessions {
                                        let is_running = meta.status == SessionStatus::Running;
                                        let border = if is_running {
                                            palette::ACCENT
                                        } else {
                                            palette::BORDER
                                        };

                                        egui::Frame::none()
                                            .fill(palette::CARD)
                                            .rounding(12.0)
                                            .stroke(egui::Stroke::new(1.0, border))
                                            .inner_margin(egui::Margin::symmetric(20.0, 16.0))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    // Left: accent bar for running
                                                    if is_running {
                                                        let rect = egui::Rect::from_min_size(
                                                            ui.cursor().left_top()
                                                                + egui::vec2(-20.0, -16.0),
                                                            egui::vec2(3.0, ui.available_height() + 32.0),
                                                        );
                                                        ui.painter().rect_filled(
                                                            rect,
                                                            2.0,
                                                            palette::ACCENT,
                                                        );
                                                    }

                                                    ui.vertical(|ui| {
                                                        ui.horizontal(|ui| {
                                                            ui.colored_label(
                                                                if is_running {
                                                                    palette::WHITE
                                                                } else {
                                                                    palette::TEXT_DIM
                                                                },
                                                                egui::RichText::new(&meta.id)
                                                                    .monospace()
                                                                    .size(12.0)
                                                                    .strong(),
                                                            );
                                                            ui.add_space(8.0);
                                                            let (badge_bg, badge_text, badge_label) =
                                                                session_badge(&meta.status);
                                                            egui::Frame::none()
                                                                .fill(badge_bg)
                                                                .rounding(4.0)
                                                                .inner_margin(egui::Margin::symmetric(
                                                                    6.0, 2.0,
                                                                ))
                                                                .show(ui, |ui| {
                                                                    ui.colored_label(
                                                                        badge_text,
                                                                        egui::RichText::new(badge_label)
                                                                            .size(9.0)
                                                                            .strong(),
                                                                    );
                                                                });
                                                        });
                                                        ui.add_space(4.0);
                                                        ui.colored_label(
                                                            palette::TEXT_DIM,
                                                            egui::RichText::new(format!(
                                                                "Updated {}  ·  {}",
                                                                format_time(&meta.updated_at),
                                                                meta.model,
                                                            ))
                                                            .size(11.0),
                                                        );
                                                    });

                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(egui::Align::Center),
                                                        |ui| {
                                                            // Delete button
                                                            let del_btn = egui::Button::new(
                                                                egui::RichText::new("Delete")
                                                                    .size(11.0)
                                                                    .color(palette::ERROR),
                                                            )
                                                            .fill(egui::Color32::TRANSPARENT)
                                                            .stroke(egui::Stroke::NONE)
                                                            .rounding(4.0);
                                                            if ui.add(del_btn).clicked() {
                                                                delete_id = Some(meta.id.clone());
                                                            }

                                                            // Action button
                                                            let (label, fill) = if is_running {
                                                                ("Open Running Chat", palette::ACCENT)
                                                            } else {
                                                                ("Resume in Desktop", palette::CARD)
                                                            };
                                                            let stroke = if is_running {
                                                                egui::Stroke::NONE
                                                            } else {
                                                                egui::Stroke::new(1.0, palette::BORDER)
                                                            };
                                                            let action_btn = egui::Button::new(
                                                                egui::RichText::new(label)
                                                                    .size(11.0)
                                                                    .strong()
                                                                    .color(palette::WHITE),
                                                            )
                                                            .fill(fill)
                                                            .stroke(stroke)
                                                            .rounding(6.0);
                                                            if ui.add(action_btn).clicked() {
                                                                self.resume_or_attach_session(
                                                                    meta.clone(),
                                                                );
                                                            }
                                                        },
                                                    );
                                                });
                                            });
                                        ui.add_space(8.0);
                                    }

                                    if let Some(id) = delete_id {
                                        self.delete_session(&id);
                                    }
                                }

                                ui.add_space(32.0);
                            },
                        );
                    });
            });
    }

    // -----------------------------------------------------------------------
    // Chat view — matches dashboar-detail.html
    // -----------------------------------------------------------------------
    fn show_chat_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::same(0.0)),
            )
            .show(ctx, |ui| {
                let Some(session) = self.active_session.as_mut() else {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(
                            palette::TEXT_DIM,
                            egui::RichText::new(
                                "No active chat. Start a new session from Projects.",
                            )
                            .size(15.0),
                        );
                    });
                    return;
                };

                // Approval bar at top if needed
                if !session.pending_approvals.is_empty() {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(30, 20, 10))
                        .inner_margin(egui::Margin::symmetric(24.0, 10.0))
                        .stroke(egui::Stroke::new(1.0, palette::WARNING))
                        .show(ui, |ui| {
                            let approvals = session.pending_approvals.clone();
                            for a in approvals {
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        palette::WARNING,
                                        egui::RichText::new(format!(
                                            "Approval needed: {} — {}",
                                            a.tool, a.description
                                        ))
                                        .size(12.0),
                                    );
                                    let approve_btn = egui::Button::new(
                                        egui::RichText::new("Approve")
                                            .size(11.0)
                                            .color(palette::WHITE),
                                    )
                                    .fill(palette::SUCCESS)
                                    .rounding(4.0);
                                    if ui.add(approve_btn).clicked() {
                                        session.controller.send_command(
                                            &AgentCommand::ApproveToolCall {
                                                call_id: a.call_id.clone(),
                                            },
                                        );
                                    }
                                    let deny_btn = egui::Button::new(
                                        egui::RichText::new("Deny")
                                            .size(11.0)
                                            .color(palette::WHITE),
                                    )
                                    .fill(palette::ERROR)
                                    .rounding(4.0);
                                    if ui.add(deny_btn).clicked() {
                                        session.controller.send_command(
                                            &AgentCommand::DenyToolCall {
                                                call_id: a.call_id.clone(),
                                            },
                                        );
                                    }
                                });
                            }
                        });
                }

                // Chat transcript
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let max_w = 780.0_f32.min((ui.available_width() - 48.0).max(200.0));
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), ui.available_height()),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                ui.set_max_width(max_w);
                                ui.add_space(24.0);

                                for entry in &session.transcript {
                                    render_chat_entry(ui, entry);
                                }

                                if !session.streaming_assistant.is_empty() {
                                    render_chat_entry(
                                        ui,
                                        &ChatEntry {
                                            role: ChatRole::Assistant,
                                            title: "Orchestrator".into(),
                                            content: session.streaming_assistant.clone(),
                                        },
                                    );
                                }

                                if session.run_in_progress && session.streaming_assistant.is_empty()
                                {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(
                                            palette::ACCENT,
                                            egui::RichText::new("● Agent is working...")
                                                .size(12.0),
                                        );
                                    });
                                    ui.add_space(8.0);
                                }

                                if let Some(reason) = &session.ended {
                                    ui.add_space(8.0);
                                    ui.colored_label(
                                        palette::TEXT_DIM,
                                        egui::RichText::new(format!(
                                            "Session ended: {:?}",
                                            reason
                                        ))
                                        .size(12.0),
                                    );
                                }

                                ui.add_space(16.0);
                            },
                        );
                    });

                // Bottom input bar
                egui::TopBottomPanel::bottom("chat_input")
                    .exact_height(100.0)
                    .frame(
                        egui::Frame::none()
                            .fill(palette::BG)
                            .inner_margin(egui::Margin::symmetric(0.0, 12.0))
                            .stroke(egui::Stroke::new(1.0, palette::BORDER)),
                    )
                    .show_inside(ui, |ui| {
                        let max_w = 780.0_f32.min((ui.available_width() - 48.0).max(200.0));
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), ui.available_height()),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                ui.set_max_width(max_w);
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        palette::ACCENT,
                                        egui::RichText::new("$").monospace().size(14.0).strong(),
                                    );
                                    ui.add_space(4.0);
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut session.composer)
                                            .font(egui::FontId::monospace(13.0))
                                            .desired_width((ui.available_width() - 160.0).max(100.0))
                                            .hint_text("Type a command or message to dispatch..."),
                                    );

                                    let enter_pressed = resp.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter));

                                    let can_send = !session.composer.trim().is_empty();

                                    let send_btn = egui::Button::new(
                                        egui::RichText::new("Dispatch")
                                            .size(12.0)
                                            .strong()
                                            .color(palette::WHITE),
                                    )
                                    .fill(palette::ACCENT)
                                    .rounding(8.0)
                                    .min_size(egui::vec2(90.0, 32.0));

                                                    if ui.add_enabled(can_send, send_btn).clicked()
                                        || (enter_pressed && can_send)
                                    {
                                        session.run_in_progress = true;
                                        session.ended = None;
                                        session.controller.send_command(
                                            &AgentCommand::SendMessage {
                                                content: session.composer.clone(),
                                            },
                                        );
                                        session.composer.clear();
                                    }

                                    let cancel_btn = egui::Button::new(
                                        egui::RichText::new("Cancel")
                                            .size(11.0)
                                            .color(palette::TEXT_DIM),
                                    )
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::new(1.0, palette::BORDER))
                                    .rounding(6.0);
                                    if ui.add(cancel_btn).clicked() {
                                        session
                                            .controller
                                            .send_command(&AgentCommand::Cancel);
                                    }
                                });
                            },
                        );
                    });
            });
    }

    // -----------------------------------------------------------------------
    // Settings view
    // -----------------------------------------------------------------------
    fn show_settings_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::symmetric(32.0, 24.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    scope_tab(ui, &mut self.settings_scope, SettingsScope::Project, "Project");
                    scope_tab(ui, &mut self.settings_scope, SettingsScope::Global, "Global");
                });
                ui.add_space(16.0);

                match self.settings_scope {
                    SettingsScope::Global => {
                        ui.colored_label(
                            palette::WHITE,
                            egui::RichText::new("Global Settings").size(18.0).strong(),
                        );
                        ui.colored_label(
                            palette::TEXT_DIM,
                            egui::RichText::new("Saved to ~/.nca/config.toml").size(11.0),
                        );
                        ui.add_space(12.0);
                        show_config_form(ui, &mut self.global_settings, false);
                        ui.add_space(12.0);
                        let btn = egui::Button::new(
                            egui::RichText::new("Save Global Settings")
                                .size(12.0)
                                .strong()
                                .color(palette::WHITE),
                        )
                        .fill(palette::ACCENT)
                        .rounding(6.0);
                        if ui.add(btn).clicked() {
                            match self.global_settings.save_global() {
                                Ok(()) => self.set_status("Saved global settings.", false),
                                Err(e) => self.set_status(e.to_string(), true),
                            }
                        }
                    }
                    SettingsScope::Project => {
                        let Some(workspace_root) = self.selected_workspace() else {
                            ui.colored_label(palette::TEXT_DIM, "Pick a project folder first.");
                            return;
                        };
                        ui.colored_label(
                            palette::WHITE,
                            egui::RichText::new("Project Settings").size(18.0).strong(),
                        );
                        ui.colored_label(
                            palette::TEXT_DIM,
                            egui::RichText::new(format!(
                                "{} — .nca/config.local.toml",
                                workspace_root.display()
                            ))
                            .size(11.0),
                        );
                        ui.add_space(12.0);
                        if let Some(config) = self.project_settings.as_mut() {
                            show_config_form(ui, config, true);
                            ui.add_space(12.0);
                            let mut save_clicked = false;
                            let mut reset_clicked = false;
                            ui.horizontal(|ui| {
                                let save_btn = egui::Button::new(
                                    egui::RichText::new("Save Project Settings")
                                        .size(12.0)
                                        .strong()
                                        .color(palette::WHITE),
                                )
                                .fill(palette::ACCENT)
                                .rounding(6.0);
                                save_clicked = ui.add(save_btn).clicked();
                                ui.add_space(8.0);
                                let reset_btn = egui::Button::new(
                                    egui::RichText::new("Reset Overrides")
                                        .size(11.0)
                                        .color(palette::TEXT_DIM),
                                )
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::new(1.0, palette::BORDER))
                                .rounding(6.0);
                                reset_clicked = ui.add(reset_btn).clicked();
                            });
                            if save_clicked {
                                match config.save_workspace_file(&workspace_root) {
                                    Ok(()) => {
                                        self.set_status("Saved project settings.", false);
                                        self.reload_selected_workspace_data();
                                    }
                                    Err(e) => self.set_status(e.to_string(), true),
                                }
                            }
                            if reset_clicked {
                                match NcaConfig::clear_workspace_file(&workspace_root) {
                                    Ok(()) => {
                                        self.set_status("Removed project override file.", false);
                                        self.reload_selected_workspace_data();
                                    }
                                    Err(e) => self.set_status(e.to_string(), true),
                                }
                            }
                        }
                    }
                }
            });
    }
}

// ---------------------------------------------------------------------------
// eframe::App
// ---------------------------------------------------------------------------

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply dark visuals globally
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = palette::BG;
        visuals.window_fill = palette::CARD;
        visuals.extreme_bg_color = palette::INPUT_BG;
        visuals.widgets.noninteractive.bg_fill = palette::CARD;
        visuals.widgets.inactive.bg_fill = palette::INPUT_BG;
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(35, 35, 35);
        visuals.widgets.active.bg_fill = palette::ACCENT;
        visuals.selection.bg_fill = palette::ACCENT_BG;
        visuals.selection.stroke = egui::Stroke::new(1.0, palette::ACCENT);
        ctx.set_visuals(visuals);

        self.process_live_events();
        self.show_sidebar(ctx);
        self.show_header(ctx);

        match self.view {
            View::Projects => self.show_projects_view(ctx),
            View::Chat => self.show_chat_view(ctx),
            View::Settings => self.show_settings_view(ctx),
        }

        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------
// Chat rendering
// ---------------------------------------------------------------------------

fn render_chat_entry(ui: &mut egui::Ui, item: &ChatEntry) {
    let is_user = item.role == ChatRole::User;
    let max_w = ui.available_width() * 0.80;

    if is_user {
        // Right-aligned user bubble
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            egui::Frame::none()
                .fill(palette::USER_BUBBLE)
                .rounding(egui::Rounding {
                    nw: 16.0,
                    ne: 4.0, // flat top-right corner like the template
                    sw: 16.0,
                    se: 16.0,
                })
                .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                .show(ui, |ui| {
                    ui.set_max_width(max_w);
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 210, 255),
                        egui::RichText::new(&item.title).size(9.0).strong(),
                    );
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&item.content)
                                .size(13.0)
                                .color(palette::WHITE),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                });
        });
    } else {
        // Left-aligned assistant / tool / error bubble
        let (fill, title_color, is_mono) = match item.role {
            ChatRole::Assistant => (palette::ASSISTANT_BUBBLE, palette::ACCENT, false),
            ChatRole::Tool => (palette::TOOL_BUBBLE, palette::TEXT_DIM, true),
            ChatRole::Error => (palette::ERROR_BUBBLE, palette::ERROR, true),
            ChatRole::User => unreachable!(),
        };

        ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
            egui::Frame::none()
                .fill(fill)
                .rounding(egui::Rounding {
                    nw: 4.0, // flat top-left corner
                    ne: 16.0,
                    sw: 16.0,
                    se: 16.0,
                })
                .stroke(egui::Stroke::new(1.0, palette::BORDER))
                .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                .show(ui, |ui| {
                    ui.set_max_width(max_w);
                    ui.colored_label(
                        title_color,
                        egui::RichText::new(&item.title).size(9.0).strong(),
                    );
                    ui.add_space(4.0);

                    let mut text = egui::RichText::new(&item.content).color(palette::TEXT);
                    if is_mono {
                        text = text.monospace().size(12.0);
                    } else {
                        text = text.size(13.0);
                    }
                    ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Wrap));
                });
        });
    }
    ui.add_space(10.0);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn draw_separator(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let y = rect.top();
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + 8.0, y),
            egui::pos2(rect.right() - 8.0, y),
        ],
        egui::Stroke::new(1.0, palette::BORDER),
    );
    ui.add_space(1.0);
}

fn truncate_path(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        format!("...{}", &s[s.len() - max_chars..])
    }
}

fn format_time(dt: &chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%b %d, %H:%M UTC").to_string()
}

fn session_badge(status: &SessionStatus) -> (egui::Color32, egui::Color32, &'static str) {
    match status {
        SessionStatus::Running => (palette::ACCENT_BG, palette::ACCENT, "RUNNING"),
        SessionStatus::Completed => (
            egui::Color32::from_rgb(20, 20, 20),
            palette::TEXT_DIM,
            "COMPLETED",
        ),
        SessionStatus::Error => (palette::ERROR_BUBBLE, palette::ERROR, "ERROR"),
        SessionStatus::Cancelled => (
            egui::Color32::from_rgb(20, 20, 20),
            palette::WARNING,
            "CANCELLED",
        ),
    }
}

fn scope_tab(ui: &mut egui::Ui, selected: &mut SettingsScope, value: SettingsScope, label: &str) {
    let is_active = *selected == value;
    let (text_color, underline) = if is_active {
        (palette::ACCENT, true)
    } else {
        (palette::TEXT_DIM, false)
    };

    let resp = ui.add(
        egui::Label::new(egui::RichText::new(label).size(13.0).strong().color(text_color))
            .sense(egui::Sense::click()),
    );
    if underline {
        let rect = resp.rect;
        ui.painter().line_segment(
            [
                egui::pos2(rect.left(), rect.bottom() + 2.0),
                egui::pos2(rect.right(), rect.bottom() + 2.0),
            ],
            egui::Stroke::new(2.0, palette::ACCENT),
        );
    }
    if resp.clicked() {
        *selected = value;
    }
    ui.add_space(16.0);
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
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
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

fn workspace_event_log_path(
    workspace_root: &Path,
    config: &NcaConfig,
    session_id: &str,
) -> PathBuf {
    workspace_root
        .join(&config.session.history_dir)
        .join(format!("{session_id}.events.jsonl"))
}

fn message_to_chat_entry(message: &Message) -> Option<ChatEntry> {
    if message.content.trim().is_empty() {
        return None;
    }
    match message.role {
        Role::User => Some(ChatEntry {
            role: ChatRole::User,
            title: "Developer".into(),
            content: message.content.clone(),
        }),
        Role::Assistant => Some(ChatEntry {
            role: ChatRole::Assistant,
            title: "Orchestrator".into(),
            content: message.content.clone(),
        }),
        Role::Tool => Some(ChatEntry {
            role: ChatRole::Tool,
            title: "System".into(),
            content: message.content.clone(),
        }),
        Role::System => None,
    }
}

fn show_config_form(ui: &mut egui::Ui, config: &mut NcaConfig, is_project: bool) {
    egui::Frame::none()
        .fill(palette::CARD)
        .rounding(10.0)
        .stroke(egui::Stroke::new(1.0, palette::BORDER))
        .inner_margin(egui::Margin::symmetric(20.0, 16.0))
        .show(ui, |ui| {
            config_row(ui, "PROVIDER", |ui| {
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
            ui.add_space(8.0);
            config_row(ui, "API KEY", |ui| {
                ui.add(
                    egui::TextEdit::singleline(
                        config
                            .provider
                            .minimax
                            .api_key
                            .get_or_insert_with(String::new),
                    )
                    .password(true)
                    .desired_width(300.0),
                );
            });
            ui.add_space(8.0);
            config_row(ui, "API KEY ENV VAR", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut config.provider.minimax.api_key_env)
                        .desired_width(300.0),
                );
            });
            ui.add_space(8.0);
            config_row(ui, "BASE URL", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut config.provider.minimax.base_url)
                        .desired_width(300.0),
                );
            });
            ui.add_space(8.0);
            config_row(ui, "DEFAULT MODEL", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut config.model.default_model)
                        .desired_width(300.0),
                );
            });
            config.provider.minimax.model = config.model.default_model.clone();
            ui.add_space(8.0);
            config_row(ui, "PERMISSION MODE", |ui| {
                permission_mode_combo(ui, &mut config.permissions.mode);
            });
        });
    ui.add_space(4.0);
    ui.colored_label(
        palette::TEXT_DIM,
        egui::RichText::new(
            "Only MiniMax is implemented. Other providers stay disabled until their runtime support lands.",
        )
        .size(10.0),
    );
}

fn config_row(ui: &mut egui::Ui, label: &str, add_widget: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(130.0, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.colored_label(
                    palette::TEXT_DIM,
                    egui::RichText::new(label).size(9.0).strong(),
                );
            },
        );
        add_widget(ui);
    });
}

fn permission_mode_combo(ui: &mut egui::Ui, mode: &mut PermissionMode) {
    egui::ComboBox::from_id_salt("perm_mode")
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

fn provider_label(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::MiniMax => "MiniMax",
        ProviderKind::OpenRouter => "OpenRouter",
        ProviderKind::Anthropic => "Anthropic",
        ProviderKind::OpenAi => "OpenAI",
    }
}
