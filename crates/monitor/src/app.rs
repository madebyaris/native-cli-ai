use crate::controller::LiveAttachController;
use crate::workspaces::WorkspaceManager;
use eframe::egui;
use nca_common::config::{NcaConfig, PermissionMode, ProviderKind};
use nca_common::event::{AgentCommand, AgentEvent, EndReason};
use nca_common::message::{Message, Role};
use nca_common::orchestration::{
    AgentProfile, AgentProfileId, Company, CompanyId, DesktopMode, NewAgentProfile, NewCompany,
    NewProject, NewTodo, OrchestrationSnapshot, Project, ProjectId, RunLaunchContext, RunLink,
    Todo, TodoId, TodoPriority, TodoStatus,
};
use nca_common::session::{SessionMeta, SessionStatus};
use nca_runtime::service::{
    OrchestrationService, ServiceSessionHandle, ServiceSessionInfo, ServiceSessionKind,
    ServiceSessionRequest,
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
    pub const ACCENT_BG: Color32 = Color32::from_rgb(15, 30, 55); // accent at ~15%
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
    Dashboard,
    Projects,
    Todos,
    Agents,
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

#[derive(Debug, Clone, Default)]
struct CompanyForm {
    name: String,
    description: String,
}

#[derive(Debug, Clone, Default)]
struct ProjectForm {
    name: String,
    slug: String,
    description: String,
    workspace_root: String,
}

#[derive(Debug, Clone, Default)]
struct TodoForm {
    title: String,
    description: String,
    acceptance_criteria: String,
    priority: TodoPriority,
}

#[derive(Debug, Clone, Default)]
struct AgentForm {
    name: String,
    role: String,
    model: String,
    prompt_hint: String,
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
    input_tokens: u64,
    output_tokens: u64,
    estimated_cost_usd: f64,
    child_session_ids: Vec<String>,
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
            input_tokens: 0,
            output_tokens: 0,
            estimated_cost_usd: 0.0,
            child_session_ids: Vec::new(),
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
    orchestration_service: OrchestrationService,
    orchestration: OrchestrationSnapshot,
    desktop_mode: DesktopMode,
    selected_company_id: Option<CompanyId>,
    selected_project_id: Option<ProjectId>,
    selected_todo_id: Option<TodoId>,
    selected_agent_id: Option<AgentProfileId>,
    selected_session_id: Option<String>,
    workspace_mgr: WorkspaceManager,
    view: View,
    settings_scope: SettingsScope,
    global_settings: NcaConfig,
    project_settings: Option<NcaConfig>,
    composer: ComposerState,
    project_sessions: Vec<SessionMeta>,
    active_session: Option<ActiveSession>,
    company_form: CompanyForm,
    project_form: ProjectForm,
    todo_form: TodoForm,
    agent_form: AgentForm,
    status_message: Option<(String, bool, Instant)>,
}

impl DesktopApp {
    pub fn new() -> Self {
        let orchestration_service = OrchestrationService::default();
        let orchestration = orchestration_service.load_snapshot().unwrap_or_default();
        let mut workspace_mgr = WorkspaceManager::load();
        workspace_mgr.sort_by_recent();
        if !workspace_mgr.workspaces.is_empty() {
            workspace_mgr.select(Some(0));
        }

        let global_settings = NcaConfig::load_global_file().unwrap_or_default();
        let mut app = Self {
            orchestration_service,
            desktop_mode: orchestration.mode.mode,
            orchestration,
            selected_company_id: None,
            selected_project_id: None,
            selected_todo_id: None,
            selected_agent_id: None,
            selected_session_id: None,
            workspace_mgr,
            view: View::Dashboard,
            settings_scope: SettingsScope::Project,
            global_settings,
            project_settings: None,
            composer: ComposerState::default(),
            project_sessions: Vec::new(),
            active_session: None,
            company_form: CompanyForm::default(),
            project_form: ProjectForm::default(),
            todo_form: TodoForm::default(),
            agent_form: AgentForm::default(),
            status_message: None,
        };
        app.sync_orchestration_selection();
        app.reload_selected_workspace_data();
        app
    }

    fn reload_orchestration_data(&mut self) {
        match self.orchestration_service.load_snapshot() {
            Ok(snapshot) => {
                self.desktop_mode = snapshot.mode.mode;
                self.orchestration = snapshot;
                self.sync_orchestration_selection();
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn sync_orchestration_selection(&mut self) {
        if self.orchestration.companies.is_empty() {
            self.selected_company_id = None;
            self.selected_project_id = None;
            self.selected_todo_id = None;
            self.selected_agent_id = None;
            return;
        }

        if self
            .selected_company_id
            .as_ref()
            .is_none_or(|id| !self.orchestration.companies.iter().any(|c| &c.id == id))
        {
            self.selected_company_id = self.orchestration.companies.first().map(|c| c.id.clone());
        }

        let project_ids: Vec<_> = self
            .selected_company_id
            .as_ref()
            .map(|company_id| {
                self.orchestration
                    .projects
                    .iter()
                    .filter(|project| &project.company_id == company_id)
                    .map(|project| project.id.clone())
                    .collect()
            })
            .unwrap_or_default();

        if self
            .selected_project_id
            .as_ref()
            .is_none_or(|id| !project_ids.iter().any(|candidate| candidate == id))
        {
            self.selected_project_id = project_ids.first().cloned();
        }

        let todo_ids: Vec<_> = self
            .selected_project_id
            .as_ref()
            .map(|project_id| {
                self.orchestration
                    .todos
                    .iter()
                    .filter(|todo| &todo.project_id == project_id)
                    .map(|todo| todo.id.clone())
                    .collect()
            })
            .unwrap_or_default();

        if self
            .selected_todo_id
            .as_ref()
            .is_none_or(|id| !todo_ids.iter().any(|candidate| candidate == id))
        {
            self.selected_todo_id = todo_ids.first().cloned();
        }

        let agent_ids: Vec<_> = self
            .selected_project_id
            .as_ref()
            .map(|project_id| {
                self.orchestration
                    .agents
                    .iter()
                    .filter(|agent| agent.project_id.as_ref() == Some(project_id))
                    .map(|agent| agent.id.clone())
                    .collect()
            })
            .unwrap_or_default();

        if self
            .selected_agent_id
            .as_ref()
            .is_none_or(|id| !agent_ids.iter().any(|candidate| candidate == id))
        {
            self.selected_agent_id = agent_ids.first().cloned();
        }
    }

    fn selected_company(&self) -> Option<&Company> {
        let id = self.selected_company_id.as_ref()?;
        self.orchestration
            .companies
            .iter()
            .find(|company| &company.id == id)
    }

    fn selected_project(&self) -> Option<&Project> {
        let id = self.selected_project_id.as_ref()?;
        self.orchestration
            .projects
            .iter()
            .find(|project| &project.id == id)
    }

    fn selected_todo(&self) -> Option<&Todo> {
        let id = self.selected_todo_id.as_ref()?;
        self.orchestration.todos.iter().find(|todo| &todo.id == id)
    }

    fn company_projects(&self) -> Vec<&Project> {
        let Some(company_id) = self.selected_company_id.as_ref() else {
            return Vec::new();
        };
        self.orchestration
            .projects
            .iter()
            .filter(|project| &project.company_id == company_id)
            .collect()
    }

    fn project_todos(&self) -> Vec<&Todo> {
        let Some(project_id) = self.selected_project_id.as_ref() else {
            return Vec::new();
        };
        let mut todos: Vec<_> = self
            .orchestration
            .todos
            .iter()
            .filter(|todo| &todo.project_id == project_id)
            .collect();
        todos.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        todos
    }

    fn project_agents(&self) -> Vec<&AgentProfile> {
        let company_id = self.selected_company_id.as_ref();
        let project_id = self.selected_project_id.as_ref();
        let mut agents: Vec<_> = self
            .orchestration
            .agents
            .iter()
            .filter(|agent| {
                agent.project_id.as_ref() == project_id
                    || (agent.project_id.is_none() && agent.company_id.as_ref() == company_id)
            })
            .collect();
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        agents
    }

    fn project_run_links(&self) -> Vec<&RunLink> {
        let todo_ids: std::collections::BTreeSet<_> = self
            .project_todos()
            .into_iter()
            .map(|todo| todo.id.clone())
            .collect();
        let mut runs: Vec<_> = self
            .orchestration
            .run_links
            .iter()
            .filter(|run| todo_ids.contains(&run.todo_id))
            .collect();
        runs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        runs
    }

    fn set_desktop_mode(&mut self, mode: DesktopMode) {
        if self.desktop_mode == mode {
            return;
        }
        match self.orchestration_service.save_mode(mode) {
            Ok(pref) => {
                self.desktop_mode = pref.mode;
                self.reload_orchestration_data();
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn create_company(&mut self) {
        if self.company_form.name.trim().is_empty() {
            self.set_status("Enter a company name first.", true);
            return;
        }
        let input = NewCompany {
            name: self.company_form.name.clone(),
            description: clean_optional_text(&self.company_form.description),
        };
        match self.orchestration_service.create_company(input) {
            Ok(company) => {
                self.company_form = CompanyForm::default();
                self.reload_orchestration_data();
                self.selected_company_id = Some(company.id);
                self.set_status("Company created.", false);
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn create_project(&mut self) {
        let Some(company_id) = self.selected_company_id.clone() else {
            self.set_status("Create or select a company first.", true);
            return;
        };
        if self.project_form.name.trim().is_empty() {
            self.set_status("Enter a project name first.", true);
            return;
        }
        let workspace_root =
            clean_optional_text(&self.project_form.workspace_root).map(PathBuf::from);
        let input = NewProject {
            company_id,
            name: self.project_form.name.clone(),
            slug: self.project_form.slug.clone(),
            description: clean_optional_text(&self.project_form.description),
            workspace_root,
        };
        match self.orchestration_service.create_project(input) {
            Ok(project) => {
                self.project_form = ProjectForm::default();
                self.reload_orchestration_data();
                self.selected_project_id = Some(project.id);
                self.reload_selected_workspace_data();
                self.set_status("Project created.", false);
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn create_todo(&mut self) {
        let Some(project_id) = self.selected_project_id.clone() else {
            self.set_status("Select a project before creating a todo.", true);
            return;
        };
        if self.todo_form.title.trim().is_empty() {
            self.set_status("Enter a todo title first.", true);
            return;
        }
        let acceptance_criteria = self
            .todo_form
            .acceptance_criteria
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect();
        let input = NewTodo {
            project_id,
            title: self.todo_form.title.clone(),
            description: clean_optional_text(&self.todo_form.description),
            priority: self.todo_form.priority,
            acceptance_criteria,
        };
        match self.orchestration_service.create_todo(input) {
            Ok(todo) => {
                self.todo_form = TodoForm::default();
                self.reload_orchestration_data();
                self.selected_todo_id = Some(todo.id);
                self.set_status("Todo created.", false);
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn create_agent_profile(&mut self) {
        if self.agent_form.name.trim().is_empty() || self.agent_form.role.trim().is_empty() {
            self.set_status("Enter an agent name and role first.", true);
            return;
        }
        let input = NewAgentProfile {
            company_id: self.selected_company_id.clone(),
            project_id: self.selected_project_id.clone(),
            name: self.agent_form.name.clone(),
            role: self.agent_form.role.clone(),
            model: clean_optional_text(&self.agent_form.model),
            workspace_root: self
                .selected_project()
                .and_then(|project| project.workspace_root.clone()),
            prompt_hint: clean_optional_text(&self.agent_form.prompt_hint),
        };
        match self.orchestration_service.create_agent_profile(input) {
            Ok(agent) => {
                self.agent_form = AgentForm::default();
                self.reload_orchestration_data();
                self.selected_agent_id = Some(agent.id);
                self.set_status("Agent created.", false);
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn assign_selected_todo(&mut self, agent_id: Option<AgentProfileId>) {
        let Some(todo_id) = self.selected_todo_id.clone() else {
            return;
        };
        match self
            .orchestration_service
            .assign_todo(&todo_id, agent_id.as_ref())
        {
            Ok(()) => {
                self.reload_orchestration_data();
                self.set_status("Todo assignment updated.", false);
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn update_selected_todo_status(&mut self, status: TodoStatus) {
        let Some(todo_id) = self.selected_todo_id.clone() else {
            return;
        };
        match self
            .orchestration_service
            .update_todo_status(&todo_id, status)
        {
            Ok(()) => {
                self.reload_orchestration_data();
                self.set_status("Todo status updated.", false);
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn select_project_workspace_for_form(&mut self) {
        if let Some(path) = FileDialog::new().pick_folder() {
            self.project_form.workspace_root = path.display().to_string();
        }
    }

    fn launch_selected_todo(&mut self) {
        let Some(todo) = self.selected_todo().cloned() else {
            self.set_status("Select a todo to launch.", true);
            return;
        };
        let mut prompt = format!("Project task: {}\n\n", todo.title);
        if let Some(project) = self.selected_project() {
            prompt.push_str(&format!("Project: {}\n", project.name));
        }
        if let Some(description) = &todo.description {
            prompt.push_str(&format!("\nDescription:\n{}\n", description));
        }
        if !todo.acceptance_criteria.is_empty() {
            prompt.push_str("\nAcceptance criteria:\n");
            for item in &todo.acceptance_criteria {
                prompt.push_str(&format!("- {item}\n"));
            }
        }

        let launch_context = RunLaunchContext {
            todo_id: todo.id.clone(),
            agent_id: todo
                .assigned_agent_id
                .clone()
                .or_else(|| self.selected_agent_id.clone()),
        };
        self.composer.prompt = prompt.clone();
        self.start_session_from_prompt(prompt, Some(launch_context));
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
        self.selected_project()
            .and_then(|project| project.workspace_root.clone())
            .or_else(|| self.workspace_mgr.selected_path().cloned())
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
        let prompt = self.composer.prompt.clone();
        self.start_session_from_prompt(prompt, None);
    }

    fn start_session_from_prompt(
        &mut self,
        prompt: String,
        launch_context: Option<RunLaunchContext>,
    ) {
        let Some(workspace_root) = self.selected_workspace() else {
            self.set_status("Pick a project folder first.", true);
            return;
        };
        if prompt.trim().is_empty() {
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
            initial_prompt: Some(prompt),
            orchestration_context: None,
            launch_context,
            kind: ServiceSessionKind::New { session_id: None },
        }) {
            Ok(handle) => {
                let info = handle.info().clone();
                match attach_controller(&info) {
                    Ok(controller) => {
                        let mut session =
                            ActiveSession::from_loaded(info, controller, Some(handle), Vec::new());
                        session.run_in_progress = true;
                        self.selected_session_id = Some(session.info.session_id.clone());
                        self.active_session = Some(session);
                        self.composer.prompt.clear();
                        self.view = View::Chat;
                        self.reload_orchestration_data();
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
                        self.selected_session_id = Some(meta.id.clone());
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
            orchestration_context: None,
            launch_context: None,
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
                        self.selected_session_id = Some(meta.id.clone());
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
        let mut refresh_sessions = false;
        let mut refresh_orchestration = false;
        {
            let Some(session) = self.active_session.as_mut() else {
                return;
            };
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
                    AgentEvent::CostUpdated {
                        input_tokens,
                        output_tokens,
                        estimated_cost_usd,
                    } => {
                        session.input_tokens = input_tokens;
                        session.output_tokens = output_tokens;
                        session.estimated_cost_usd = estimated_cost_usd;
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
                        session.pending_approvals.retain(|a| a.call_id != call_id);
                        if !approved {
                            session.push_error("Tool approval was denied.".into());
                        }
                    }
                    AgentEvent::SessionEnded { reason } => {
                        session.ended = Some(reason);
                        session.run_in_progress = false;
                        refresh_sessions = true;
                        refresh_orchestration = true;
                    }
                    AgentEvent::Error { message } => {
                        session.push_error(message);
                        session.run_in_progress = false;
                    }
                    AgentEvent::ChildSessionSpawned {
                        child_session_id, ..
                    } => {
                        if !session.child_session_ids.contains(&child_session_id) {
                            session.child_session_ids.push(child_session_id);
                        }
                        refresh_sessions = true;
                    }
                    AgentEvent::ChildSessionCompleted { .. } => {
                        refresh_sessions = true;
                    }
                    AgentEvent::Checkpoint { .. }
                    | AgentEvent::Response { .. }
                    | AgentEvent::TodoStatusChanged { .. }
                    | AgentEvent::TodoAssigned { .. }
                    | AgentEvent::RunLinked { .. }
                    | AgentEvent::DesktopModeChanged { .. } => {}
                }
            }
        }
        if refresh_sessions {
            self.reload_selected_workspace_data();
        }
        if refresh_orchestration {
            self.reload_orchestration_data();
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

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.colored_label(
                        palette::WHITE,
                        egui::RichText::new("nca desktop").strong().size(16.0),
                    );
                });
                ui.add_space(10.0);

                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(12.0, 0.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let is_project = self.desktop_mode == DesktopMode::ProjectAi;
                            let is_company = self.desktop_mode == DesktopMode::CompanyAi;
                            if mode_pill(ui, is_project, "Project AI").clicked() {
                                self.set_desktop_mode(DesktopMode::ProjectAi);
                            }
                            if mode_pill(ui, is_company, "Company AI").clicked() {
                                self.set_desktop_mode(DesktopMode::CompanyAi);
                            }
                        });
                    });
                ui.add_space(14.0);
                draw_separator(ui);
                ui.add_space(8.0);
                let mut nav_items = vec![
                    (View::Dashboard, "Dashboard"),
                    (View::Projects, "Projects"),
                    (View::Todos, "Todos"),
                    (View::Agents, "Agents"),
                    (View::Chat, "Chat"),
                    (View::Settings, "Settings"),
                ];
                if self.desktop_mode == DesktopMode::ProjectAi {
                    nav_items.remove(0);
                }
                for (view, label) in nav_items {
                    if draw_nav_link(ui, self.view == view, label).clicked() {
                        self.view = view;
                    }
                }

                ui.add_space(16.0);
                draw_separator(ui);
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 52.0).max(20.0))
                    .show(ui, |ui| {
                        section_label(ui, "COMPANIES");
                        if self.orchestration.companies.is_empty() {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.colored_label(palette::TEXT_DIM, "Create your first company.");
                            });
                        } else {
                            let companies = self.orchestration.companies.clone();
                            for company in companies {
                                if draw_entity_tile(
                                    ui,
                                    self.selected_company_id.as_ref() == Some(&company.id),
                                    &company.name,
                                    company.description.as_deref().unwrap_or(""),
                                )
                                .clicked()
                                {
                                    self.selected_company_id = Some(company.id.clone());
                                    self.sync_orchestration_selection();
                                    self.reload_selected_workspace_data();
                                }
                            }
                        }

                        ui.add_space(10.0);
                        section_label(ui, "PROJECTS");
                        if self.company_projects().is_empty() {
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.colored_label(palette::TEXT_DIM, "No projects in this company.");
                            });
                        } else {
                            let projects: Vec<_> =
                                self.company_projects().into_iter().cloned().collect();
                            for project in projects {
                                let subtitle = project
                                    .workspace_root
                                    .as_ref()
                                    .map(|path| truncate_path(&path.display().to_string(), 28))
                                    .unwrap_or_else(|| "no workspace linked".into());
                                if draw_entity_tile(
                                    ui,
                                    self.selected_project_id.as_ref() == Some(&project.id),
                                    &project.name,
                                    &subtitle,
                                )
                                .clicked()
                                {
                                    self.selected_project_id = Some(project.id.clone());
                                    self.sync_orchestration_selection();
                                    self.reload_selected_workspace_data();
                                }
                            }
                        }

                        ui.add_space(10.0);
                        section_label(ui, "AGENTS");
                        let agents: Vec<_> = self.project_agents().into_iter().cloned().collect();
                        if agents.is_empty() {
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.colored_label(palette::TEXT_DIM, "No agents yet.");
                            });
                        } else {
                            for agent in agents {
                                if draw_entity_tile(
                                    ui,
                                    self.selected_agent_id.as_ref() == Some(&agent.id),
                                    &agent.name,
                                    &agent.role,
                                )
                                .clicked()
                                {
                                    self.selected_agent_id = Some(agent.id.clone());
                                }
                            }
                        }
                    });

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    draw_separator(ui);
                    ui.add_space(8.0);
                    let btn = egui::Button::new(
                        egui::RichText::new("Open Folder")
                            .size(12.0)
                            .color(palette::TEXT),
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
            .exact_height(44.0)
            .frame(
                egui::Frame::none()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::symmetric(20.0, 0.0))
                    .stroke(egui::Stroke::new(1.0, palette::BORDER)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let mut crumbs: Vec<(&str, egui::Color32)> = Vec::new();
                    if self.desktop_mode == DesktopMode::CompanyAi {
                        if let Some(company) = self.selected_company() {
                            crumbs.push((&company.name, palette::WHITE));
                        }
                    }
                    if let Some(project) = self.selected_project() {
                        crumbs.push((&project.name, palette::TEXT));
                    }
                    let view_name = match self.view {
                        View::Dashboard => "Dashboard",
                        View::Projects => "Projects",
                        View::Todos => "Todos",
                        View::Agents => "Agents",
                        View::Chat => "Chat",
                        View::Settings => "Settings",
                    };
                    crumbs.push((view_name, palette::TEXT_DIM));

                    for (i, (label, color)) in crumbs.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(4.0);
                            ui.colored_label(
                                palette::TEXT_DIM,
                                egui::RichText::new("/").size(12.0),
                            );
                            ui.add_space(4.0);
                        }
                        ui.colored_label(*color, egui::RichText::new(*label).size(13.0));
                    }

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

    fn show_dashboard_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::symmetric(24.0, 0.0)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add_space(24.0);
                        ui.horizontal_wrapped(|ui| {
                            stat_card(ui, "Projects", &self.company_projects().len().to_string());
                            stat_card(ui, "Open Todos", &self.project_todos().len().to_string());
                            stat_card(ui, "Agents", &self.project_agents().len().to_string());
                            stat_card(ui, "Runs", &self.project_run_links().len().to_string());
                        });
                        ui.add_space(18.0);

                        ui.columns(2, |columns| {
                            columns[0].add_space(4.0);
                            panel_card(&mut columns[0], "Create Company", |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.company_form.name)
                                        .hint_text("Company name"),
                                );
                                ui.add_space(8.0);
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.company_form.description)
                                        .desired_rows(3)
                                        .hint_text("Description"),
                                );
                                ui.add_space(8.0);
                                if ui.button("Create Company").clicked() {
                                    self.create_company();
                                }
                            });

                            panel_card(&mut columns[0], "Create Project", |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.project_form.name)
                                        .hint_text("Project name"),
                                );
                                ui.add_space(8.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.project_form.slug)
                                        .hint_text("Slug"),
                                );
                                ui.add_space(8.0);
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.project_form.description)
                                        .desired_rows(3)
                                        .hint_text("Description"),
                                );
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(
                                            &mut self.project_form.workspace_root,
                                        )
                                        .desired_width(220.0)
                                        .hint_text("/path/to/repo"),
                                    );
                                    if ui.button("Browse").clicked() {
                                        self.select_project_workspace_for_form();
                                    }
                                });
                                ui.add_space(8.0);
                                if ui.button("Create Project").clicked() {
                                    self.create_project();
                                }
                            });
                        });

                        ui.add_space(16.0);
                        ui.columns(2, |columns| {
                            panel_card(&mut columns[0], "Create Todo", |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.todo_form.title)
                                        .hint_text("Todo title"),
                                );
                                ui.add_space(8.0);
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.todo_form.description)
                                        .desired_rows(3)
                                        .hint_text("Description"),
                                );
                                ui.add_space(8.0);
                                ui.label(egui::RichText::new("Acceptance Criteria").size(11.0));
                                ui.add(
                                    egui::TextEdit::multiline(
                                        &mut self.todo_form.acceptance_criteria,
                                    )
                                    .desired_rows(3)
                                    .hint_text("One line per acceptance criterion"),
                                );
                                ui.add_space(8.0);
                                todo_priority_combo(ui, &mut self.todo_form.priority);
                                ui.add_space(8.0);
                                if ui.button("Create Todo").clicked() {
                                    self.create_todo();
                                }
                            });

                            panel_card(&mut columns[1], "Create Agent", |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.agent_form.name)
                                        .hint_text("Agent name"),
                                );
                                ui.add_space(8.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.agent_form.role)
                                        .hint_text("Role"),
                                );
                                ui.add_space(8.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.agent_form.model)
                                        .hint_text("MiniMax-M2.5"),
                                );
                                ui.add_space(8.0);
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.agent_form.prompt_hint)
                                        .desired_rows(3)
                                        .hint_text("Prompt or operating hint"),
                                );
                                ui.add_space(8.0);
                                if ui.button("Create Agent").clicked() {
                                    self.create_agent_profile();
                                }
                            });
                        });

                        ui.add_space(20.0);
                        ui.colored_label(
                            palette::WHITE,
                            egui::RichText::new("Recent Linked Runs")
                                .size(16.0)
                                .strong(),
                        );
                        ui.add_space(10.0);
                        let runs: Vec<_> = self.project_run_links().into_iter().cloned().collect();
                        if runs.is_empty() {
                            ui.colored_label(
                                palette::TEXT_DIM,
                                "No linked runs yet. Launch a selected todo to create one.",
                            );
                        } else {
                            let mut open_chat = None;
                            for run in runs {
                                let todo_title = self
                                    .orchestration
                                    .todos
                                    .iter()
                                    .find(|todo| todo.id == run.todo_id)
                                    .map(|todo| todo.title.clone())
                                    .unwrap_or_else(|| "Unknown todo".into());
                                let agent_label = run
                                    .agent_id
                                    .as_ref()
                                    .and_then(|id| {
                                        self.orchestration
                                            .agents
                                            .iter()
                                            .find(|agent| &agent.id == id)
                                            .map(|agent| agent.name.clone())
                                    })
                                    .unwrap_or_else(|| "Unassigned".into());
                                panel_card(
                                    ui,
                                    &format!("{todo_title} · {}", run.session_id),
                                    |ui| {
                                        ui.colored_label(
                                            palette::TEXT_DIM,
                                            format!(
                                                "{} · {}",
                                                agent_label,
                                                format_time(&run.updated_at)
                                            ),
                                        );
                                        if let Some(branch) = &run.branch {
                                            ui.colored_label(
                                                palette::TEXT_DIM,
                                                format!("branch: {branch}"),
                                            );
                                        }
                                        if let Some(worktree) = &run.worktree_path {
                                            ui.colored_label(
                                                palette::TEXT_DIM,
                                                truncate_path(&worktree.display().to_string(), 60),
                                            );
                                        }
                                        ui.add_space(6.0);
                                        if ui.button("Open Chat").clicked() {
                                            open_chat = Some(run.session_id.clone());
                                        }
                                    },
                                );
                            }
                            if let Some(session_id) = open_chat {
                                if let Some(meta) = self
                                    .project_sessions
                                    .iter()
                                    .find(|meta| meta.id == session_id)
                                    .cloned()
                                {
                                    self.resume_or_attach_session(meta);
                                }
                            }
                        }
                        ui.add_space(20.0);
                    });
            });
    }

    fn show_todos_view(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("todo_detail")
            .resizable(true)
            .default_width(320.0)
            .frame(
                egui::Frame::none()
                    .fill(palette::CARD)
                    .inner_margin(egui::Margin::same(16.0))
                    .stroke(egui::Stroke::new(1.0, palette::BORDER)),
            )
            .show(ctx, |ui| {
                ui.colored_label(
                    palette::WHITE,
                    egui::RichText::new("Todo Detail").size(15.0).strong(),
                );
                ui.add_space(12.0);
                let todo = self.selected_todo().cloned();
                if let Some(todo) = todo {
                    ui.label(egui::RichText::new(&todo.title).size(14.0).strong());
                    if let Some(desc) = &todo.description {
                        ui.add_space(8.0);
                        ui.colored_label(palette::TEXT_DIM, desc);
                    }
                    ui.add_space(10.0);
                    let mut status = todo.status;
                    todo_status_combo(ui, &mut status);
                    if status != todo.status && ui.button("Save Status").clicked() {
                        self.update_selected_todo_status(status);
                    }
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Assign Agent").size(11.0).strong());
                    for agent in self
                        .project_agents()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>()
                    {
                        let assigned = todo.assigned_agent_id.as_ref() == Some(&agent.id);
                        if ui
                            .selectable_label(assigned, format!("{} · {}", agent.name, agent.role))
                            .clicked()
                        {
                            self.assign_selected_todo(Some(agent.id.clone()));
                        }
                    }
                    if ui.button("Clear Assignment").clicked() {
                        self.assign_selected_todo(None);
                    }
                    ui.add_space(12.0);
                    if ui
                        .add(
                            egui::Button::new("Launch Run")
                                .fill(palette::ACCENT)
                                .min_size(egui::vec2(120.0, 32.0)),
                        )
                        .clicked()
                    {
                        self.launch_selected_todo();
                    }
                    if !todo.acceptance_criteria.is_empty() {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Acceptance").size(11.0).strong());
                        for item in &todo.acceptance_criteria {
                            ui.colored_label(palette::TEXT_DIM, format!("- {item}"));
                        }
                    }
                } else {
                    ui.colored_label(palette::TEXT_DIM, "Select a todo to inspect.");
                }
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::same(16.0)),
            )
            .show(ctx, |ui| {
                let todos: Vec<_> = self.project_todos().into_iter().cloned().collect();
                if todos.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(palette::TEXT_DIM, "No todos for this project yet.");
                    });
                    return;
                }

                let columns = [
                    TodoStatus::Backlog,
                    TodoStatus::InProgress,
                    TodoStatus::InReview,
                    TodoStatus::Done,
                ];
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for status in columns {
                            let items: Vec<_> = todos
                                .iter()
                                .filter(|todo| todo.status == status)
                                .cloned()
                                .collect();
                            panel_card(ui, todo_status_label(status), |ui| {
                                ui.set_width(250.0);
                                if items.is_empty() {
                                    ui.colored_label(palette::TEXT_DIM, "No items");
                                } else {
                                    for todo in &items {
                                        let selected =
                                            self.selected_todo_id.as_ref() == Some(&todo.id);
                                        if draw_entity_tile(
                                            ui,
                                            selected,
                                            &todo.title,
                                            todo.description.as_deref().unwrap_or(""),
                                        )
                                        .clicked()
                                        {
                                            self.selected_todo_id = Some(todo.id.clone());
                                        }
                                    }
                                }
                            });
                        }
                    });
                });
            });
    }

    fn show_agents_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::same(16.0)),
            )
            .show(ctx, |ui| {
                ui.colored_label(
                    palette::WHITE,
                    egui::RichText::new("Agents").size(18.0).strong(),
                );
                ui.add_space(12.0);
                let agents: Vec<_> = self.project_agents().into_iter().cloned().collect();
                if agents.is_empty() {
                    ui.colored_label(palette::TEXT_DIM, "No agents for this scope yet.");
                } else {
                    for agent in agents {
                        let assigned_count = self
                            .project_todos()
                            .into_iter()
                            .filter(|todo| todo.assigned_agent_id.as_ref() == Some(&agent.id))
                            .count();
                        panel_card(ui, &agent.name, |ui| {
                            ui.colored_label(
                                palette::TEXT_DIM,
                                format!(
                                    "{} · {}",
                                    agent.role,
                                    agent
                                        .model
                                        .clone()
                                        .unwrap_or_else(|| "default model".into())
                                ),
                            );
                            ui.colored_label(
                                palette::TEXT_DIM,
                                format!("{assigned_count} assigned todos"),
                            );
                            if ui.button("Select Agent").clicked() {
                                self.selected_agent_id = Some(agent.id.clone());
                                self.view = View::Todos;
                            }
                        });
                    }
                }
            });
    }

    // -----------------------------------------------------------------------
    // Chat view — matches dashboar-detail.html
    // -----------------------------------------------------------------------
    fn show_chat_view(&mut self, ctx: &egui::Context) {
        let has_session = self.active_session.is_some() || self.selected_session_id.is_some();
        if has_session {
        egui::SidePanel::right("chat_run_detail")
            .default_width(280.0)
            .resizable(true)
            .frame(
                egui::Frame::none()
                    .fill(palette::CARD)
                    .inner_margin(egui::Margin::same(16.0))
                    .stroke(egui::Stroke::new(1.0, palette::BORDER)),
            )
            .show(ctx, |ui| {
                ui.colored_label(
                    palette::WHITE,
                    egui::RichText::new("Run Detail").size(14.0).strong(),
                );
                ui.add_space(12.0);
                let active_session_id = self
                    .active_session
                    .as_ref()
                    .map(|session| session.info.session_id.clone())
                    .or_else(|| self.selected_session_id.clone());
                if let Some(session_id) = active_session_id {
                    ui.colored_label(
                        palette::TEXT,
                        egui::RichText::new(&session_id).monospace().size(11.0),
                    );
                    if let Some(run) = self
                        .orchestration
                        .run_links
                        .iter()
                        .find(|run| run.session_id == session_id)
                    {
                        if let Some(todo) = self
                            .orchestration
                            .todos
                            .iter()
                            .find(|todo| todo.id == run.todo_id)
                        {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Todo").size(11.0).strong());
                            ui.colored_label(palette::TEXT_DIM, &todo.title);
                        }
                        if let Some(agent_id) = &run.agent_id {
                            if let Some(agent) = self
                                .orchestration
                                .agents
                                .iter()
                                .find(|agent| &agent.id == agent_id)
                            {
                                ui.add_space(8.0);
                                ui.label(egui::RichText::new("Agent").size(11.0).strong());
                                ui.colored_label(palette::TEXT_DIM, &agent.name);
                            }
                        }
                        if let Some(branch) = &run.branch {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Branch").size(11.0).strong());
                            ui.colored_label(palette::TEXT_DIM, branch);
                        }
                    }

                    if let Some(active) = self.active_session.as_ref() {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Usage").size(11.0).strong());
                        ui.colored_label(
                            palette::TEXT_DIM,
                            format!(
                                "{} in · {} out · ${:.4}",
                                active.input_tokens,
                                active.output_tokens,
                                active.estimated_cost_usd
                            ),
                        );
                        if let Some(workspace) = active.info.workspace_root.to_str() {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Workspace").size(11.0).strong());
                            ui.colored_label(palette::TEXT_DIM, truncate_path(workspace, 42));
                        }
                        if !active.child_session_ids.is_empty() {
                            ui.add_space(12.0);
                            ui.label(egui::RichText::new("Child Sessions").size(11.0).strong());
                            for child in &active.child_session_ids {
                                ui.colored_label(palette::TEXT_DIM, child);
                            }
                        }
                    }

                    if let Some(workspace_root) = self.selected_workspace() {
                        if let Some(state) = load_session_state(
                            &workspace_root,
                            &self.effective_project_config(),
                            &session_id,
                        ) {
                            if let Some(worktree) = state.meta.worktree_path {
                                ui.add_space(8.0);
                                ui.label(egui::RichText::new("Worktree").size(11.0).strong());
                                ui.colored_label(
                                    palette::TEXT_DIM,
                                    truncate_path(&worktree.display().to_string(), 42),
                                );
                            }
                            if !state.meta.child_session_ids.is_empty() {
                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new("Persisted Lineage").size(11.0).strong(),
                                );
                                for child in state.meta.child_session_ids {
                                    ui.colored_label(palette::TEXT_DIM, child);
                                }
                            }
                        }
                    }
                } else {
                    ui.colored_label(palette::TEXT_DIM, "Open a session to inspect its lineage.");
                }
            });
        }

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
                                            egui::RichText::new("● Agent is working...").size(12.0),
                                        );
                                    });
                                    ui.add_space(8.0);
                                }

                                if let Some(reason) = &session.ended {
                                    ui.add_space(8.0);
                                    ui.colored_label(
                                        palette::TEXT_DIM,
                                        egui::RichText::new(format!("Session ended: {:?}", reason))
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
                                            .desired_width(
                                                (ui.available_width() - 160.0).max(100.0),
                                            )
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
                                        session.controller.send_command(&AgentCommand::Cancel);
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
                    scope_tab(
                        ui,
                        &mut self.settings_scope,
                        SettingsScope::Project,
                        "Project",
                    );
                    scope_tab(
                        ui,
                        &mut self.settings_scope,
                        SettingsScope::Global,
                        "Global",
                    );
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
            View::Dashboard => self.show_dashboard_view(ctx),
            View::Projects => self.show_projects_view(ctx),
            View::Todos => self.show_todos_view(ctx),
            View::Agents => self.show_agents_view(ctx),
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
        egui::Label::new(
            egui::RichText::new(label)
                .size(13.0)
                .strong()
                .color(text_color),
        )
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

fn mode_pill(ui: &mut egui::Ui, active: bool, label: &str) -> egui::Response {
    let text_color = if active { palette::WHITE } else { palette::TEXT_DIM };
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(11.5).color(text_color))
            .fill(if active {
                palette::ACCENT_BG
            } else {
                palette::CARD
            })
            .stroke(egui::Stroke::new(
                1.0,
                if active {
                    palette::ACCENT
                } else {
                    palette::BORDER
                },
            ))
            .rounding(6.0)
            .min_size(egui::vec2(0.0, 26.0)),
    )
}

fn draw_nav_link(ui: &mut egui::Ui, is_active: bool, label: &str) -> egui::Response {
    let text_color = if is_active {
        palette::WHITE
    } else {
        palette::TEXT
    };
    let bg = if is_active {
        palette::ACCENT_BG
    } else {
        egui::Color32::TRANSPARENT
    };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
    let inner = rect.shrink2(egui::vec2(10.0, 1.0));

    ui.painter().rect_filled(inner, 6.0, bg);
    if is_active {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(inner.left_top(), egui::vec2(3.0, inner.height())),
            2.0,
            palette::ACCENT,
        );
    }

    ui.painter().text(
        egui::pos2(inner.left() + 22.0, inner.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.5),
        text_color,
    );

    response
}

fn section_label(ui: &mut egui::Ui, label: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.colored_label(
            palette::TEXT_DIM,
            egui::RichText::new(label).size(10.5).strong(),
        );
    });
    ui.add_space(4.0);
}

fn draw_entity_tile(
    ui: &mut egui::Ui,
    selected: bool,
    title: &str,
    subtitle: &str,
) -> egui::Response {
    let (border_color, bg_color) = if selected {
        (palette::ACCENT, palette::ACCENT_BG)
    } else {
        (palette::BORDER, palette::CARD)
    };
    let outer_rect = ui.available_rect_before_wrap();
    let desired = egui::vec2((outer_rect.width() - 24.0).max(40.0), 44.0);
    let resp = ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 48.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(12.0);
            let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
            ui.painter()
                .rect(rect, 6.0, bg_color, egui::Stroke::new(1.0, border_color));
            let title_color = if selected {
                palette::WHITE
            } else {
                palette::TEXT
            };
            ui.painter().text(
                rect.left_top() + egui::vec2(10.0, 7.0),
                egui::Align2::LEFT_TOP,
                truncate_path(title, 26),
                egui::FontId::proportional(12.5),
                title_color,
            );
            if !subtitle.is_empty() {
                ui.painter().text(
                    rect.left_top() + egui::vec2(10.0, 25.0),
                    egui::Align2::LEFT_TOP,
                    truncate_path(subtitle, 30),
                    egui::FontId::proportional(10.5),
                    palette::TEXT_DIM,
                );
            }
            resp
        },
    );
    ui.add_space(2.0);
    resp.inner
}

fn stat_card(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::none()
        .fill(palette::CARD)
        .rounding(10.0)
        .stroke(egui::Stroke::new(1.0, palette::BORDER))
        .inner_margin(egui::Margin::symmetric(16.0, 12.0))
        .show(ui, |ui| {
            ui.set_min_width(140.0);
            ui.colored_label(
                palette::TEXT_DIM,
                egui::RichText::new(label).size(11.0).strong(),
            );
            ui.add_space(4.0);
            ui.colored_label(
                palette::WHITE,
                egui::RichText::new(value).size(22.0).strong(),
            );
        });
}

fn panel_card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(palette::CARD)
        .rounding(12.0)
        .stroke(egui::Stroke::new(1.0, palette::BORDER))
        .inner_margin(egui::Margin::symmetric(16.0, 14.0))
        .show(ui, |ui| {
            ui.colored_label(
                palette::WHITE,
                egui::RichText::new(title).size(14.0).strong(),
            );
            ui.add_space(10.0);
            add_contents(ui);
        });
    ui.add_space(10.0);
}

fn todo_priority_combo(ui: &mut egui::Ui, priority: &mut TodoPriority) {
    egui::ComboBox::from_id_salt("todo_priority")
        .selected_text(match priority {
            TodoPriority::Low => "Priority: Low",
            TodoPriority::Medium => "Priority: Medium",
            TodoPriority::High => "Priority: High",
            TodoPriority::Critical => "Priority: Critical",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(priority, TodoPriority::Low, "Low");
            ui.selectable_value(priority, TodoPriority::Medium, "Medium");
            ui.selectable_value(priority, TodoPriority::High, "High");
            ui.selectable_value(priority, TodoPriority::Critical, "Critical");
        });
}

fn todo_status_combo(ui: &mut egui::Ui, status: &mut TodoStatus) {
    egui::ComboBox::from_id_salt("todo_status")
        .selected_text(todo_status_label(*status))
        .show_ui(ui, |ui| {
            for candidate in [
                TodoStatus::Backlog,
                TodoStatus::Ready,
                TodoStatus::InProgress,
                TodoStatus::InReview,
                TodoStatus::Blocked,
                TodoStatus::Done,
                TodoStatus::Cancelled,
            ] {
                ui.selectable_value(status, candidate, todo_status_label(candidate));
            }
        });
}

fn todo_status_label(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Backlog => "Backlog",
        TodoStatus::Ready => "Ready",
        TodoStatus::InProgress => "In Progress",
        TodoStatus::InReview => "In Review",
        TodoStatus::Blocked => "Blocked",
        TodoStatus::Done => "Done",
        TodoStatus::Cancelled => "Cancelled",
    }
}

fn clean_optional_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
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

fn load_session_state(
    workspace_root: &Path,
    config: &NcaConfig,
    session_id: &str,
) -> Option<nca_common::session::SessionState> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let store = SessionStore::new(workspace_root.join(&config.session.history_dir));
    runtime.block_on(store.load(session_id)).ok()
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
