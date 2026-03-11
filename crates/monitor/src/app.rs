use eframe::egui;
use nca_common::event::AgentCommand;

use crate::controller::LiveAttachController;
use crate::ingest::{reduce_event, replay_events_file};
use crate::panels::review::{ReviewAction, ReviewState};
use crate::panels::{log, review, sessions, stats, terminal, timeline, tools};
use crate::session_index::{IndexedSession, SessionIndex};
use crate::state::{AppState, TaskCard, TaskStatus};
use crate::workspaces::WorkspaceManager;

/// View the app is currently showing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum View {
    Home,
    Session,
    NewSession,
}

/// State for the new-session creation dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionPreset {
    Default,
    Plan,
    AcceptEdits,
    DontAsk,
    BypassPermissions,
}

impl PermissionPreset {
    fn as_arg(self) -> &'static str {
        match self {
            PermissionPreset::Default => "default",
            PermissionPreset::Plan => "plan",
            PermissionPreset::AcceptEdits => "accept-edits",
            PermissionPreset::DontAsk => "dont-ask",
            PermissionPreset::BypassPermissions => "bypass-permissions",
        }
    }

    fn label(self) -> &'static str {
        match self {
            PermissionPreset::Default => "Default",
            PermissionPreset::Plan => "Plan (read-only)",
            PermissionPreset::AcceptEdits => "Accept edits",
            PermissionPreset::DontAsk => "Don't ask (deny writes/exec)",
            PermissionPreset::BypassPermissions => "Bypass permissions",
        }
    }
}

impl Default for PermissionPreset {
    fn default() -> Self {
        Self::AcceptEdits
    }
}

#[derive(Debug)]
struct NewSessionForm {
    workspace_idx: Option<usize>,
    prompt: String,
    model_override: String,
    safe_mode: bool,
    permission_mode: PermissionPreset,
}

impl Default for NewSessionForm {
    fn default() -> Self {
        Self {
            workspace_idx: None,
            prompt: String::new(),
            model_override: String::new(),
            safe_mode: false,
            permission_mode: PermissionPreset::AcceptEdits,
        }
    }
}

pub struct MonitorApp {
    app_state: AppState,
    session_index: SessionIndex,
    live_attach: Option<LiveAttachController>,
    workspace_mgr: WorkspaceManager,
    view: View,
    add_workspace_path: String,
    prompt_input: String,
    new_session_form: NewSessionForm,
    status_message: Option<(String, std::time::Instant)>,
    review_state: ReviewState,
}

impl MonitorApp {
    pub fn new() -> Self {
        let mut workspace_mgr = WorkspaceManager::load();
        workspace_mgr.sort_by_recent();

        let paths = workspace_mgr.workspace_paths();
        let session_index = if paths.is_empty() {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            SessionIndex::new(cwd)
        } else {
            SessionIndex::multi(paths)
        };

        Self {
            app_state: AppState::new(),
            session_index,
            live_attach: None,
            workspace_mgr,
            view: View::Home,
            add_workspace_path: String::new(),
            prompt_input: String::new(),
            new_session_form: NewSessionForm::default(),
            status_message: None,
            review_state: ReviewState::default(),
        }
    }

    fn sync_workspaces_to_index(&mut self) {
        let paths = self.workspace_mgr.workspace_paths();
        if paths.is_empty() {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            self.session_index.set_workspaces(vec![cwd]);
        } else {
            self.session_index.set_workspaces(paths);
        }
    }

    fn apply_live_events(&mut self) {
        let Some(ref attach) = self.live_attach else {
            return;
        };
        let events = attach.drain();
        if let Some(ref mut vm) = self.app_state.session_vm {
            for event in events {
                let (te, pa, tu, cu, err, se, resolve) = reduce_event(&event);
                if let Some(call_id) = resolve {
                    vm.resolve_approval(&call_id);
                }
                vm.apply(te, pa, tu, cu, err, se);
            }
        }
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), std::time::Instant::now()));
    }

    fn show_status_bar(&self, ui: &mut egui::Ui) {
        if let Some((ref msg, at)) = self.status_message {
            if at.elapsed().as_secs() < 5 {
                ui.colored_label(egui::Color32::from_rgb(200, 200, 80), msg);
            }
        }
    }

    fn show_new_session(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("New Session");
            ui.add_space(8.0);

            let ws_names: Vec<(usize, String, String)> = self
                .workspace_mgr
                .workspaces
                .iter()
                .enumerate()
                .map(|(i, w)| (i, w.name.clone(), w.path.display().to_string()))
                .collect();

            ui.label("Workspace:");
            for (i, name, path_str) in &ws_names {
                let selected = self.new_session_form.workspace_idx == Some(*i);
                if ui
                    .selectable_label(selected, format!("{name} ({path_str})"))
                    .clicked()
                {
                    self.new_session_form.workspace_idx = Some(*i);
                }
            }

            ui.add_space(8.0);
            ui.label("Prompt:");
            ui.text_edit_multiline(&mut self.new_session_form.prompt);

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Model (optional):");
                ui.text_edit_singleline(&mut self.new_session_form.model_override);
            });

            ui.checkbox(
                &mut self.new_session_form.safe_mode,
                "Safe mode (read-only)",
            );
            ui.add_space(4.0);
            egui::ComboBox::from_label("Permission mode")
                .selected_text(self.new_session_form.permission_mode.label())
                .show_ui(ui, |ui| {
                    for mode in [
                        PermissionPreset::AcceptEdits,
                        PermissionPreset::Default,
                        PermissionPreset::Plan,
                        PermissionPreset::DontAsk,
                        PermissionPreset::BypassPermissions,
                    ] {
                        ui.selectable_value(
                            &mut self.new_session_form.permission_mode,
                            mode,
                            mode.label(),
                        );
                    }
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_start = self.new_session_form.workspace_idx.is_some()
                    && !self.new_session_form.prompt.is_empty();

                if ui
                    .add_enabled(can_start, egui::Button::new("Start session"))
                    .clicked()
                {
                    if let Some(ws_idx) = self.new_session_form.workspace_idx {
                        if let Some(ws) = self.workspace_mgr.workspaces.get(ws_idx) {
                            let workspace = ws.path.clone();
                            let prompt = self.new_session_form.prompt.clone();
                            let model = if self.new_session_form.model_override.is_empty() {
                                None
                            } else {
                                Some(self.new_session_form.model_override.clone())
                            };
                            let safe_mode = self.new_session_form.safe_mode;
                            let permission_mode = self.new_session_form.permission_mode;

                            match spawn_nca_run(
                                &workspace,
                                &prompt,
                                model.as_deref(),
                                safe_mode,
                                permission_mode,
                            ) {
                                Ok(session_id) => {
                                    self.set_status(format!(
                                        "Session {session_id} started. It will appear shortly."
                                    ));
                                    self.new_session_form = NewSessionForm::default();
                                    self.view = View::Home;
                                }
                                Err(error) => {
                                    self.set_status(format!("Failed to start session: {error}"));
                                }
                            }
                        }
                    }
                }

                if ui.button("Cancel").clicked() {
                    self.new_session_form = NewSessionForm::default();
                    self.view = View::Home;
                }
            });
        });
    }

    fn show_home(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("nca conductor");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("+ New Session").clicked() {
                        self.view = View::NewSession;
                    }
                });
            });
            self.show_status_bar(ui);
            ui.add_space(8.0);

            ui.heading("Workspaces");
            ui.separator();

            let ws_snapshot: Vec<(usize, String, String)> = self
                .workspace_mgr
                .workspaces
                .iter()
                .enumerate()
                .map(|(i, w)| (i, w.name.clone(), w.path.display().to_string()))
                .collect();
            let selected_ws = self.workspace_mgr.selected_workspace;

            let mut select_idx = None;
            let mut remove_idx = None;
            for (i, name, path_str) in &ws_snapshot {
                ui.horizontal(|ui| {
                    let is_selected = selected_ws == Some(*i);
                    if ui
                        .selectable_label(is_selected, format!("{name} \u{2014} {path_str}"))
                        .clicked()
                    {
                        select_idx = Some(*i);
                    }
                    if ui.small_button("\u{2717}").clicked() {
                        remove_idx = Some(*i);
                    }
                });
            }
            if let Some(idx) = select_idx {
                self.workspace_mgr.select(Some(idx));
            }
            if let Some(idx) = remove_idx {
                self.workspace_mgr.remove_workspace(idx);
                self.sync_workspaces_to_index();
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.add_workspace_path);
                if ui.button("Add workspace").clicked() && !self.add_workspace_path.is_empty() {
                    let path = std::path::PathBuf::from(&self.add_workspace_path);
                    self.workspace_mgr.add_workspace(path);
                    self.add_workspace_path.clear();
                    self.sync_workspaces_to_index();
                }
                if ui.button("Browse\u{2026}").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.workspace_mgr.add_workspace(path);
                        self.sync_workspaces_to_index();
                    }
                }
            });

            ui.add_space(16.0);

            let sessions = self.session_index.maybe_refresh().to_vec();
            self.build_task_cards(&sessions);

            // Summary bar
            let live_count = sessions.iter().filter(|s| s.is_live).count();
            let error_count = sessions
                .iter()
                .filter(|s| s.status_display() == "error")
                .count();
            let total = sessions.len();

            ui.horizontal(|ui| {
                ui.label(format!("{total} sessions"));
                ui.separator();
                ui.colored_label(
                    egui::Color32::from_rgb(80, 200, 120),
                    format!("{live_count} active"),
                );
                ui.separator();
                if error_count > 0 {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 80, 80),
                        format!("{error_count} errors"),
                    );
                } else {
                    ui.label("0 errors");
                }
            });

            ui.add_space(8.0);
            self.show_dashboard(ui);
        });
    }

    fn build_task_cards(&mut self, sessions: &[IndexedSession]) {
        self.app_state.task_cards = sessions
            .iter()
            .map(|s| {
                let status = if s.is_live {
                    TaskStatus::Running
                } else {
                    match s.status_display() {
                        "done" => TaskStatus::Completed,
                        "error" => TaskStatus::Error,
                        "cancelled" => TaskStatus::Cancelled,
                        _ => TaskStatus::Completed,
                    }
                };
                TaskCard {
                    session_id: s.id.clone(),
                    workspace_name: s.workspace_name(),
                    workspace_path: s.workspace.clone(),
                    model: s.model_display(),
                    status,
                    branch: s.meta.as_ref().and_then(|m| m.branch.clone()),
                    pending_approvals: 0,
                    error_count: if s.status_display() == "error" { 1 } else { 0 },
                    updated_at: s.updated_display(),
                    parent_session_id: s.meta.as_ref().and_then(|m| m.parent_session_id.clone()),
                    child_session_ids: s
                        .meta
                        .as_ref()
                        .map(|m| m.child_session_ids.clone())
                        .unwrap_or_default(),
                    spawn_reason: s.meta.as_ref().and_then(|m| m.spawn_reason.clone()),
                    current_action: s
                        .last_action
                        .clone()
                        .unwrap_or_else(|| format!("status: {}", s.status_display())),
                }
            })
            .collect();
    }

    fn show_dashboard(&mut self, ui: &mut egui::Ui) {
        let cards = self.app_state.task_cards.clone();

        if cards.is_empty() {
            ui.label("No agent runs yet. Start a new session to begin.");
            return;
        }

        let top_level: Vec<_> = cards
            .iter()
            .filter(|c| c.parent_session_id.is_none())
            .collect();
        let child_cards: Vec<_> = cards
            .iter()
            .filter(|c| c.parent_session_id.is_some())
            .collect();

        let running: Vec<_> = top_level
            .iter()
            .filter(|c| c.status == TaskStatus::Running || c.status == TaskStatus::WaitingApproval)
            .copied()
            .collect();
        let completed: Vec<_> = top_level
            .iter()
            .filter(|c| c.status == TaskStatus::Completed)
            .copied()
            .collect();
        let failed: Vec<_> = top_level
            .iter()
            .filter(|c| c.status == TaskStatus::Error || c.status == TaskStatus::Cancelled)
            .copied()
            .collect();

        if !running.is_empty() {
            ui.heading("Active Runs");
            ui.separator();
            for card in &running {
                self.show_task_card(ui, card);
                self.show_child_cards(ui, card, &child_cards);
            }
            ui.add_space(8.0);
        }

        if !failed.is_empty() {
            ui.heading("Failed / Cancelled");
            ui.separator();
            for card in &failed {
                self.show_task_card(ui, card);
                self.show_child_cards(ui, card, &child_cards);
            }
            ui.add_space(8.0);
        }

        if !completed.is_empty() {
            egui::CollapsingHeader::new(format!("Completed ({})", completed.len()))
                .default_open(false)
                .show(ui, |ui| {
                    for card in &completed {
                        self.show_task_card(ui, card);
                        self.show_child_cards(ui, card, &child_cards);
                    }
                });
        }

        if !child_cards.is_empty() {
            let orphan_children: Vec<_> = child_cards
                .iter()
                .filter(|c| {
                    !top_level
                        .iter()
                        .any(|p| p.child_session_ids.contains(&c.session_id))
                })
                .copied()
                .collect();
            if !orphan_children.is_empty() {
                egui::CollapsingHeader::new(format!("Sub-agent runs ({})", orphan_children.len()))
                    .default_open(true)
                    .show(ui, |ui| {
                        for card in &orphan_children {
                            self.show_task_card(ui, card);
                        }
                    });
            }
        }
    }

    fn show_child_cards(
        &mut self,
        ui: &mut egui::Ui,
        parent: &TaskCard,
        all_children: &[&TaskCard],
    ) {
        let children: Vec<_> = all_children
            .iter()
            .filter(|c| parent.child_session_ids.contains(&c.session_id))
            .collect();
        if children.is_empty() {
            return;
        }
        ui.indent(format!("children_{}", parent.session_id), |ui| {
            for child in &children {
                ui.horizontal(|ui| {
                    ui.label("\u{2514}");
                    self.show_task_card_inline(ui, child);
                });
            }
        });
    }

    fn show_task_card_inline(&mut self, ui: &mut egui::Ui, card: &TaskCard) {
        let status_color = match card.status {
            TaskStatus::Running => egui::Color32::from_rgb(80, 200, 120),
            TaskStatus::WaitingApproval => egui::Color32::from_rgb(200, 180, 60),
            TaskStatus::Completed => egui::Color32::from_rgb(150, 150, 150),
            TaskStatus::Error => egui::Color32::from_rgb(220, 80, 80),
            TaskStatus::Cancelled => egui::Color32::from_rgb(180, 120, 60),
            TaskStatus::Queued => egui::Color32::from_rgb(100, 160, 220),
        };
        ui.colored_label(status_color, card.status.label());
        let short_id = &card.session_id[..card.session_id.len().min(16)];
        if ui.link(short_id).clicked() {
            self.app_state.select_session(Some(card.session_id.clone()));
            self.view = View::Session;
        }
        ui.label(&card.model);
        if let Some(reason) = &card.spawn_reason {
            let short_reason: String = reason.chars().take(40).collect();
            ui.label(format!("| {short_reason}"));
        }
        if let Some(branch) = &card.branch {
            ui.label(format!("\u{2192} {branch}"));
        }
        if !card.current_action.is_empty() {
            ui.label(format!("| {}", card.current_action));
        }
    }

    fn show_task_card(&mut self, ui: &mut egui::Ui, card: &TaskCard) {
        let status_color = match card.status {
            TaskStatus::Running => egui::Color32::from_rgb(80, 200, 120),
            TaskStatus::WaitingApproval => egui::Color32::from_rgb(200, 180, 60),
            TaskStatus::Completed => egui::Color32::from_rgb(150, 150, 150),
            TaskStatus::Error => egui::Color32::from_rgb(220, 80, 80),
            TaskStatus::Cancelled => egui::Color32::from_rgb(180, 120, 60),
            TaskStatus::Queued => egui::Color32::from_rgb(100, 160, 220),
        };

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(status_color, card.status.label());
                ui.separator();
                let short_id = &card.session_id[..card.session_id.len().min(16)];
                if ui.link(short_id).clicked() {
                    self.app_state.select_session(Some(card.session_id.clone()));
                    self.view = View::Session;
                }
                ui.separator();
                ui.label(&card.workspace_name);
                ui.label(&card.model);
                if let Some(branch) = &card.branch {
                    ui.label(format!("\u{2192} {branch}"));
                }
                ui.label(crate::panels::truncate_chars(&card.current_action, 80));
                if !card.child_session_ids.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(100, 160, 220),
                        format!("{} children", card.child_session_ids.len()),
                    );
                }
                if let Some(ref parent_id) = card.parent_session_id {
                    let short_parent = &parent_id[..parent_id.len().min(12)];
                    ui.label(format!("(child of {short_parent})"));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(&card.updated_at);
                    if card.pending_approvals > 0 {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 180, 60),
                            format!("{} pending", card.pending_approvals),
                        );
                    }
                });
            });
        });
    }

    fn load_review_for_session(&mut self, session: &crate::session_index::IndexedSession) {
        self.review_state.clear();

        let meta = match session.meta.as_ref() {
            Some(m) => m,
            None => return,
        };

        let wt_path = match meta.worktree_path.as_ref() {
            Some(p) => p.clone(),
            None => return,
        };
        let base = match meta.base_branch.as_ref() {
            Some(b) => b.clone(),
            None => return,
        };

        self.review_state.worktree_path = Some(wt_path.clone());
        self.review_state.base_branch = Some(base.clone());
        self.review_state.branch_name = meta.branch.clone();

        let mgr = nca_runtime::worktree::WorktreeManager::new(&session.workspace);
        let files = mgr.changed_files(&wt_path, &base);
        self.review_state.changed_files = files
            .into_iter()
            .map(|f| crate::panels::files::FileEntry {
                path: f.path,
                change_type: f.change_type.to_string(),
            })
            .collect();

        let (ahead, behind) = mgr.ahead_behind(&wt_path, &base);
        self.review_state.ahead = ahead;
        self.review_state.behind = behind;
    }

    fn load_selected_diff(&mut self) {
        let wt_path = match self.review_state.worktree_path.clone() {
            Some(p) => p,
            None => return,
        };
        let base = match self.review_state.base_branch.clone() {
            Some(b) => b,
            None => return,
        };
        let file = match self.review_state.selected_file.clone() {
            Some(f) => f,
            None => return,
        };

        let sessions = self.session_index.maybe_refresh().to_vec();
        let workspace = sessions
            .iter()
            .find(|s| s.meta.as_ref().and_then(|m| m.worktree_path.as_ref()) == Some(&wt_path))
            .map(|s| s.workspace.clone())
            .unwrap_or_else(|| wt_path.clone());

        let mgr = nca_runtime::worktree::WorktreeManager::new(&workspace);
        self.review_state.current_diff = mgr.file_diff(&wt_path, &base, &file);
    }

    fn do_merge(&mut self) {
        let wt_path = match self.review_state.worktree_path.clone() {
            Some(p) => p,
            None => return,
        };
        let base = match self.review_state.base_branch.clone() {
            Some(b) => b,
            None => return,
        };

        let sessions = self.session_index.maybe_refresh().to_vec();
        if let Some(session) = sessions
            .iter()
            .find(|s| s.meta.as_ref().and_then(|m| m.worktree_path.as_ref()) == Some(&wt_path))
        {
            let mgr = nca_runtime::worktree::WorktreeManager::new(&session.workspace);
            match mgr.merge_into_base(&session.id, &base) {
                Ok(()) => {
                    self.review_state.merge_status = Some("Merged successfully.".to_string());
                    let _ = mgr.remove_worktree(&session.id, true);
                }
                Err(e) => {
                    self.review_state.merge_status = Some(format!("Merge failed: {e}"));
                }
            }
        }
    }

    fn do_discard(&mut self) {
        let wt_path = match self.review_state.worktree_path.clone() {
            Some(p) => p,
            None => return,
        };

        let sessions = self.session_index.maybe_refresh().to_vec();
        if let Some(session) = sessions
            .iter()
            .find(|s| s.meta.as_ref().and_then(|m| m.worktree_path.as_ref()) == Some(&wt_path))
        {
            let mgr = nca_runtime::worktree::WorktreeManager::new(&session.workspace);
            let _ = mgr.remove_worktree(&session.id, true);
            self.review_state.clear();
            self.set_status("Worktree discarded.");
        }
    }

    fn refresh_review(&mut self) {
        let selected_id = self.app_state.selected_session_id.clone();
        if let Some(ref id) = selected_id {
            let sessions = self.session_index.maybe_refresh().to_vec();
            if let Some(s) = sessions.iter().find(|x| &x.id == id) {
                self.load_review_for_session(s);
            }
        }
    }

    fn show_session_view(&mut self, ctx: &egui::Context) {
        let sessions = self.session_index.maybe_refresh().to_vec();

        let selected_id = self.app_state.selected_session_id.clone();
        if let Some(ref id) = selected_id {
            if let Some(s) = sessions.iter().find(|x| &x.id == id) {
                let needs_load = self
                    .app_state
                    .loaded_session_id
                    .as_ref()
                    .map(|loaded| loaded != id)
                    .unwrap_or(true);
                if needs_load {
                    if let Some(attach) = self.live_attach.take() {
                        attach.stop();
                    }
                    let events_path = s.events_path();
                    let vm = replay_events_file(&events_path);
                    self.app_state.set_session_vm(vm);
                    self.app_state.loaded_session_id = Some(id.clone());
                    if let Some(socket_path) = s.socket_path() {
                        self.live_attach = Some(LiveAttachController::attach(socket_path));
                    }
                    self.load_review_for_session(s);
                }
            }
        } else {
            if let Some(attach) = self.live_attach.take() {
                attach.stop();
            }
            self.app_state.clear_session_vm();
        }

        self.apply_live_events();

        // Left sidebar: session list for the selected workspace
        egui::SidePanel::left("sessions_panel")
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("\u{2190} Home").clicked() {
                        self.view = View::Home;
                    }
                    ui.heading("Sessions");
                });
                ui.separator();

                if let Some(selected) = sessions::show(ui, &sessions, selected_id.as_deref()) {
                    self.app_state.select_session(Some(selected));
                }
            });

        // Approval bar
        if let (Some(vm), Some(attach)) = (&self.app_state.session_vm, &self.live_attach) {
            let pending = vm.pending_approvals.clone();
            if !pending.is_empty() {
                egui::TopBottomPanel::bottom("approval_bar")
                    .min_height(60.0)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            for approval in &pending {
                                ui.label(format!("{}: {}", approval.tool, approval.description));
                                if ui.button("Approve").clicked() {
                                    attach.send_command(&AgentCommand::ApproveToolCall {
                                        call_id: approval.call_id.clone(),
                                    });
                                }
                                if ui.button("Deny").clicked() {
                                    attach.send_command(&AgentCommand::DenyToolCall {
                                        call_id: approval.call_id.clone(),
                                    });
                                }
                            }
                        });
                    });
            }
        }

        // Prompt composer at the bottom for live sessions
        if let Some(ref _id) = selected_id {
            let is_live = sessions
                .iter()
                .find(|x| Some(&x.id) == selected_id.as_ref())
                .map(|s| s.is_live)
                .unwrap_or(false);
            if is_live {
                egui::TopBottomPanel::bottom("prompt_bar")
                    .min_height(40.0)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            let response = ui.text_edit_singleline(&mut self.prompt_input);
                            if (ui.button("Send").clicked()
                                || (response.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                                && !self.prompt_input.is_empty()
                            {
                                if let Some(ref attach) = self.live_attach {
                                    attach.send_command(&AgentCommand::SendMessage {
                                        content: self.prompt_input.clone(),
                                    });
                                    self.set_status("Prompt sent to live session");
                                    self.prompt_input.clear();
                                }
                            }
                            if let Some(ref attach) = self.live_attach {
                                if ui.button("Cancel").clicked() {
                                    attach.send_command(&AgentCommand::Cancel);
                                    self.set_status("Cancel signal sent");
                                }
                            }
                        });
                    });
            }
        }

        // Central panel
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(id) = &selected_id {
                if let Some(s) = sessions.iter().find(|x| &x.id == id) {
                    ui.horizontal(|ui| {
                        ui.heading(format!("Session: {}", &id[..id.len().min(24)]));
                        ui.label(format!("[{}]", s.workspace_name()));
                        if s.is_live {
                            ui.colored_label(
                                egui::Color32::from_rgb(80, 200, 120),
                                "\u{25CF} live",
                            );
                        } else {
                            ui.label("offline");
                            if ui.button("Reconnect").clicked() {
                                if let Some(socket_path) = s.socket_path() {
                                    if let Some(attach) = self.live_attach.take() {
                                        attach.stop();
                                    }
                                    self.live_attach =
                                        Some(LiveAttachController::attach(socket_path));
                                    self.set_status("Reconnected to live session");
                                } else {
                                    self.set_status("No active socket for this session.");
                                }
                            }
                        }
                    });
                    self.show_status_bar(ui);
                    if let Some(vm) = self.app_state.session_vm.as_ref() {
                        if let Some(action) = vm.current_action.as_ref() {
                            ui.label(format!(
                                "Current action: {}",
                                crate::panels::truncate_chars(action, 120)
                            ));
                        }
                    }
                }
                let has_vm = self.app_state.session_vm.is_some();
                if has_vm {
                    let vm = self.app_state.session_vm.as_ref().unwrap();
                    let status = sessions
                        .iter()
                        .find(|x| &x.id == id)
                        .map(|s| s.status_display())
                        .unwrap_or("\u{2014}");
                    let model = sessions
                        .iter()
                        .find(|x| &x.id == id)
                        .map(|s| s.model_display())
                        .unwrap_or_else(|| {
                            vm.model.clone().unwrap_or_else(|| "\u{2014}".to_string())
                        });

                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        stats::show(ui, vm, status, &model);
                    });
                    ui.add_space(4.0);

                    if let Some(s) = sessions.iter().find(|x| &x.id == id) {
                        let has_lineage = s.meta.as_ref().map_or(false, |m| {
                            m.parent_session_id.is_some() || !m.child_session_ids.is_empty()
                        });
                        if has_lineage {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.label("Session Lineage");
                                if let Some(ref meta) = s.meta {
                                    if let Some(ref parent_id) = meta.parent_session_id {
                                        ui.horizontal(|ui| {
                                            ui.label("Parent:");
                                            let short = &parent_id[..parent_id.len().min(20)];
                                            if ui.link(short).clicked() {
                                                self.app_state
                                                    .select_session(Some(parent_id.clone()));
                                            }
                                        });
                                        if let Some(ref reason) = meta.spawn_reason {
                                            ui.label(format!("Spawn reason: {reason}"));
                                        }
                                    }
                                    if !meta.child_session_ids.is_empty() {
                                        ui.label(format!(
                                            "Children ({}):",
                                            meta.child_session_ids.len()
                                        ));
                                        for child_id in &meta.child_session_ids {
                                            ui.horizontal(|ui| {
                                                ui.label("  \u{2514}");
                                                let short = &child_id[..child_id.len().min(20)];
                                                let child_status = sessions
                                                    .iter()
                                                    .find(|x| &x.id == child_id)
                                                    .map(|x| x.status_display())
                                                    .unwrap_or("unknown");
                                                if ui
                                                    .link(format!("{short} [{child_status}]"))
                                                    .clicked()
                                                {
                                                    self.app_state
                                                        .select_session(Some(child_id.clone()));
                                                }
                                            });
                                        }
                                    }
                                }
                            });
                            ui.add_space(4.0);
                        }
                    }

                    ui.horizontal(|ui| {
                        let vm = self.app_state.session_vm.as_ref().unwrap();
                        ui.vertical(|ui| {
                            ui.heading("Timeline");
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .max_height(300.0)
                                .show(ui, |ui| timeline::show(ui, &vm.timeline));
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            ui.heading("Tool calls");
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .max_height(300.0)
                                .show(ui, |ui| tools::show(ui, &vm.tool_calls));
                        });
                    });
                    ui.add_space(6.0);
                    egui::CollapsingHeader::new("Live terminal")
                        .default_open(true)
                        .show(ui, |ui| {
                            let vm = self.app_state.session_vm.as_ref().unwrap();
                            terminal::show(ui, vm);
                        });

                    let review_action = egui::CollapsingHeader::new("Review workbench")
                        .default_open(false)
                        .show(ui, |ui| review::show(ui, &mut self.review_state))
                        .body_returned
                        .unwrap_or(ReviewAction::None);

                    match review_action {
                        ReviewAction::LoadDiff => self.load_selected_diff(),
                        ReviewAction::Merge => self.do_merge(),
                        ReviewAction::Discard => self.do_discard(),
                        ReviewAction::Refresh => self.refresh_review(),
                        ReviewAction::None => {}
                    }

                    if let Some(vm) = self.app_state.session_vm.as_ref() {
                        egui::CollapsingHeader::new("Log viewer")
                            .default_open(false)
                            .show(ui, |ui| {
                                log::show(ui, &vm.timeline);
                            });
                    }
                }
            } else {
                ui.heading("Select a session from the sidebar.");
            }
        });
    }
}

impl eframe::App for MonitorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match self.view {
            View::Home => self.show_home(ctx),
            View::Session => self.show_session_view(ctx),
            View::NewSession => self.show_new_session(ctx),
        }
    }
}

/// Spawn an nca run as a background process. The session will appear in
/// the session index once it writes its metadata to disk.
fn spawn_nca_run(
    workspace: &std::path::Path,
    prompt: &str,
    model: Option<&str>,
    safe_mode: bool,
    permission_mode: PermissionPreset,
) -> Result<String, String> {
    let exe = match std::env::current_exe()
        .ok()
        .and_then(|p| {
            let dir = p.parent()?;
            let nca = dir.join("nca");
            if nca.exists() { Some(nca) } else { None }
        })
        .or_else(|| which_nca())
    {
        Some(exe) => exe,
        None => return Err("nca binary not found in PATH".into()),
    };

    let sessions_dir = workspace.join(".nca/sessions");
    std::fs::create_dir_all(&sessions_dir).map_err(|err| err.to_string())?;

    let session_id = format!("session-{}", chrono::Utc::now().timestamp_millis());
    let spawn_log = sessions_dir.join(format!("{session_id}.spawn.log"));

    let stdout = std::fs::File::create(&spawn_log).map_err(|err| err.to_string())?;
    let stderr = stdout.try_clone().map_err(|err| err.to_string())?;

    let mut cmd = std::process::Command::new(exe);
    cmd.current_dir(workspace)
        .arg("serve")
        .arg("--prompt")
        .arg(prompt)
        .arg("--stream")
        .arg("ndjson")
        .arg("--session-id")
        .arg(&session_id)
        .arg("--permission-mode")
        .arg(permission_mode.as_arg());

    if safe_mode {
        cmd.arg("--safe");
    }
    if let Some(model) = model {
        cmd.arg("--model").arg(model);
    }

    cmd.stdout(stdout).stderr(stderr);
    cmd.spawn().map_err(|err| err.to_string())?;
    Ok(session_id)
}

fn which_nca() -> Option<std::path::PathBuf> {
    std::env::var("PATH").ok().and_then(|paths| {
        for dir in paths.split(':') {
            let candidate = std::path::PathBuf::from(dir).join("nca");
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    })
}
