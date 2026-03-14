use crate::workspace_registry::WorkspaceRegistry;
use chrono::Utc;
use nca_common::orchestration::{
    AgentProfile, AgentProfileId, AgentProfileStatus, Company, CompanyId, CompanyStatus,
    DesktopMode, DesktopModePreference, GovernancePolicy, LinkRunRequest, NewAgentProfile,
    NewCompany, NewProject, NewTodo, OrchestrationSnapshot, Project, ProjectId, ProjectStatus,
    RunLink, RunLinkId, Todo, TodoId, TodoPriority, TodoStatus,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct OrchestratorStore {
    path: PathBuf,
}

impl OrchestratorStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".nca").join("orchestrator.db")
    }

    pub fn default() -> Self {
        Self::new(Self::default_path())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_snapshot(&self) -> Result<OrchestrationSnapshot, String> {
        let conn = self.open()?;
        Ok(OrchestrationSnapshot {
            mode: self.load_mode_with_conn(&conn)?,
            companies: self.list_companies_with_conn(&conn)?,
            projects: self.list_projects_with_conn(&conn)?,
            todos: self.list_todos_with_conn(&conn)?,
            agents: self.list_agents_with_conn(&conn)?,
            run_links: self.list_run_links_with_conn(&conn)?,
        })
    }

    pub fn load_mode(&self) -> Result<DesktopModePreference, String> {
        let conn = self.open()?;
        self.load_mode_with_conn(&conn)
    }

    pub fn save_mode(&self, mode: DesktopMode) -> Result<DesktopModePreference, String> {
        let conn = self.open()?;
        let pref = DesktopModePreference {
            mode,
            updated_at: Utc::now(),
        };
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('desktop_mode', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![to_json(&pref)?],
        )
        .map_err(|err| err.to_string())?;
        Ok(pref)
    }

    pub fn create_company(&self, input: NewCompany) -> Result<Company, String> {
        let conn = self.open()?;
        let now = Utc::now();
        let company = Company {
            id: CompanyId::new(new_id("cmp")),
            name: input.name.trim().to_string(),
            description: clean_opt(input.description),
            status: CompanyStatus::Active,
            budget: Default::default(),
            governance: GovernancePolicy::default(),
            created_at: now,
            updated_at: now,
        };
        conn.execute(
            "INSERT INTO companies (id, name, description, status, budget_json, governance_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                company.id.0,
                company.name,
                company.description,
                status_to_str_company(company.status),
                to_json(&company.budget)?,
                to_json(&company.governance)?,
                company.created_at.to_rfc3339(),
                company.updated_at.to_rfc3339(),
            ],
        )
        .map_err(|err| err.to_string())?;
        Ok(company)
    }

    pub fn create_project(&self, input: NewProject) -> Result<Project, String> {
        let conn = self.open()?;
        let now = Utc::now();
        let project = Project {
            id: ProjectId::new(new_id("prj")),
            company_id: input.company_id,
            name: input.name.trim().to_string(),
            slug: slugify(&input.slug, &input.name),
            description: clean_opt(input.description),
            workspace_root: input.workspace_root.clone(),
            status: ProjectStatus::Active,
            created_at: now,
            updated_at: now,
        };
        conn.execute(
            "INSERT INTO projects (id, company_id, name, slug, description, workspace_root, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                project.id.0,
                project.company_id.0,
                project.name,
                project.slug,
                project.description,
                path_to_db(project.workspace_root.as_ref()),
                status_to_str_project(project.status),
                project.created_at.to_rfc3339(),
                project.updated_at.to_rfc3339(),
            ],
        )
        .map_err(|err| err.to_string())?;

        if let Some(path) = &project.workspace_root {
            let mut registry = WorkspaceRegistry::load();
            registry.add(path);
            registry.save()?;
        }

        Ok(project)
    }

    pub fn create_todo(&self, input: NewTodo) -> Result<Todo, String> {
        let conn = self.open()?;
        let now = Utc::now();
        let todo = Todo {
            id: TodoId::new(new_id("todo")),
            project_id: input.project_id,
            title: input.title.trim().to_string(),
            description: clean_opt(input.description),
            status: TodoStatus::Backlog,
            priority: input.priority,
            assigned_agent_id: None,
            acceptance_criteria: clean_list(input.acceptance_criteria),
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        conn.execute(
            "INSERT INTO todos (id, project_id, title, description, status, priority, assigned_agent_id, acceptance_json, tags_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9, ?10)",
            params![
                todo.id.0,
                todo.project_id.0,
                todo.title,
                todo.description,
                status_to_str_todo(todo.status),
                priority_to_str(todo.priority),
                to_json(&todo.acceptance_criteria)?,
                to_json(&todo.tags)?,
                todo.created_at.to_rfc3339(),
                todo.updated_at.to_rfc3339(),
            ],
        )
        .map_err(|err| err.to_string())?;
        Ok(todo)
    }

    pub fn create_agent_profile(&self, input: NewAgentProfile) -> Result<AgentProfile, String> {
        let conn = self.open()?;
        let now = Utc::now();
        let agent = AgentProfile {
            id: AgentProfileId::new(new_id("agt")),
            company_id: input.company_id,
            project_id: input.project_id,
            name: input.name.trim().to_string(),
            role: input.role.trim().to_string(),
            model: clean_opt(input.model),
            status: AgentProfileStatus::Idle,
            workspace_root: input.workspace_root,
            prompt_hint: clean_opt(input.prompt_hint),
            allowed_tools: Vec::new(),
            permission_mode: None,
            created_at: now,
            updated_at: now,
        };
        conn.execute(
            "INSERT INTO agents (id, company_id, project_id, name, role, model, status, workspace_root, prompt_hint, allowed_tools_json, permission_mode, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                agent.id.0,
                agent.company_id.as_ref().map(|id| id.0.as_str()),
                agent.project_id.as_ref().map(|id| id.0.as_str()),
                agent.name,
                agent.role,
                agent.model,
                status_to_str_agent(agent.status),
                path_to_db(agent.workspace_root.as_ref()),
                agent.prompt_hint,
                to_json(&agent.allowed_tools)?,
                agent.permission_mode.map(permission_mode_to_str),
                agent.created_at.to_rfc3339(),
                agent.updated_at.to_rfc3339(),
            ],
        )
        .map_err(|err| err.to_string())?;
        Ok(agent)
    }

    pub fn assign_todo(
        &self,
        todo_id: &TodoId,
        agent_id: Option<&AgentProfileId>,
    ) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE todos SET assigned_agent_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                agent_id.map(|id| id.0.as_str()),
                Utc::now().to_rfc3339(),
                todo_id.0
            ],
        )
        .map_err(|err| err.to_string())?;
        Ok(())
    }

    pub fn update_todo_status(&self, todo_id: &TodoId, status: TodoStatus) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE todos SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                status_to_str_todo(status),
                Utc::now().to_rfc3339(),
                todo_id.0
            ],
        )
        .map_err(|err| err.to_string())?;
        Ok(())
    }

    pub fn link_run(&self, request: LinkRunRequest) -> Result<RunLink, String> {
        let conn = self.open()?;
        let now = Utc::now();
        let run = RunLink {
            id: RunLinkId::new(new_id("run")),
            todo_id: request.todo_id,
            agent_id: request.agent_id,
            session_id: request.session_id,
            workspace_root: request.workspace_root,
            worktree_path: request.worktree_path,
            branch: request.branch,
            parent_session_id: request.parent_session_id,
            status: request.status,
            created_at: now,
            updated_at: now,
        };
        conn.execute(
            "INSERT INTO run_links (id, todo_id, agent_id, session_id, workspace_root, worktree_path, branch, parent_session_id, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                run.id.0,
                run.todo_id.0,
                run.agent_id.as_ref().map(|id| id.0.as_str()),
                run.session_id,
                run.workspace_root.to_string_lossy().to_string(),
                path_to_db(run.worktree_path.as_ref()),
                run.branch,
                run.parent_session_id,
                session_status_to_str(run.status.clone()),
                run.created_at.to_rfc3339(),
                run.updated_at.to_rfc3339(),
            ],
        )
        .map_err(|err| err.to_string())?;
        Ok(run)
    }

    pub fn touch_run_status(
        &self,
        session_id: &str,
        status: nca_common::session::SessionStatus,
    ) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE run_links SET status = ?1, updated_at = ?2 WHERE session_id = ?3",
            params![
                session_status_to_str(status),
                Utc::now().to_rfc3339(),
                session_id
            ],
        )
        .map_err(|err| err.to_string())?;
        Ok(())
    }

    fn open(&self) -> Result<Connection, String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let conn = Connection::open(&self.path).map_err(|err| err.to_string())?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS settings (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS companies (
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               description TEXT,
               status TEXT NOT NULL,
               budget_json TEXT NOT NULL,
               governance_json TEXT NOT NULL,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS projects (
               id TEXT PRIMARY KEY,
               company_id TEXT NOT NULL,
               name TEXT NOT NULL,
               slug TEXT NOT NULL,
               description TEXT,
               workspace_root TEXT,
               status TEXT NOT NULL,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               FOREIGN KEY(company_id) REFERENCES companies(id) ON DELETE CASCADE
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_company_slug ON projects(company_id, slug);
             CREATE TABLE IF NOT EXISTS todos (
               id TEXT PRIMARY KEY,
               project_id TEXT NOT NULL,
               title TEXT NOT NULL,
               description TEXT,
               status TEXT NOT NULL,
               priority TEXT NOT NULL,
               assigned_agent_id TEXT,
               acceptance_json TEXT NOT NULL,
               tags_json TEXT NOT NULL,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS idx_todos_project_status ON todos(project_id, status);
             CREATE TABLE IF NOT EXISTS agents (
               id TEXT PRIMARY KEY,
               company_id TEXT,
               project_id TEXT,
               name TEXT NOT NULL,
               role TEXT NOT NULL,
               model TEXT,
               status TEXT NOT NULL,
               workspace_root TEXT,
               prompt_hint TEXT,
               allowed_tools_json TEXT NOT NULL,
               permission_mode TEXT,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               FOREIGN KEY(company_id) REFERENCES companies(id) ON DELETE CASCADE,
               FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS idx_agents_company_project ON agents(company_id, project_id);
             CREATE TABLE IF NOT EXISTS run_links (
               id TEXT PRIMARY KEY,
               todo_id TEXT NOT NULL,
               agent_id TEXT,
               session_id TEXT NOT NULL,
               workspace_root TEXT NOT NULL,
               worktree_path TEXT,
               branch TEXT,
               parent_session_id TEXT,
               status TEXT NOT NULL,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               FOREIGN KEY(todo_id) REFERENCES todos(id) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS idx_run_links_todo ON run_links(todo_id);
             CREATE INDEX IF NOT EXISTS idx_run_links_session ON run_links(session_id);",
        )
        .map_err(|err| err.to_string())?;
        Ok(conn)
    }

    fn load_mode_with_conn(&self, conn: &Connection) -> Result<DesktopModePreference, String> {
        let raw = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'desktop_mode'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| err.to_string())?;

        raw.map(|value| from_json::<DesktopModePreference>(&value))
            .transpose()
            .map_err(|err| err.to_string())?
            .map(Ok)
            .unwrap_or_else(|| Ok(DesktopModePreference::default()))
    }

    fn list_companies_with_conn(&self, conn: &Connection) -> Result<Vec<Company>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, status, budget_json, governance_json, created_at, updated_at
                 FROM companies ORDER BY updated_at DESC",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Company {
                    id: CompanyId::new(row.get::<_, String>(0)?),
                    name: row.get(1)?,
                    description: row.get(2)?,
                    status: company_status_from_str(&row.get::<_, String>(3)?),
                    budget: from_json(&row.get::<_, String>(4)?).unwrap_or_default(),
                    governance: from_json(&row.get::<_, String>(5)?).unwrap_or_default(),
                    created_at: parse_ts(&row.get::<_, String>(6)?),
                    updated_at: parse_ts(&row.get::<_, String>(7)?),
                })
            })
            .map_err(|err| err.to_string())?;
        collect_rows(rows)
    }

    fn list_projects_with_conn(&self, conn: &Connection) -> Result<Vec<Project>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, company_id, name, slug, description, workspace_root, status, created_at, updated_at
                 FROM projects ORDER BY updated_at DESC",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Project {
                    id: ProjectId::new(row.get::<_, String>(0)?),
                    company_id: CompanyId::new(row.get::<_, String>(1)?),
                    name: row.get(2)?,
                    slug: row.get(3)?,
                    description: row.get(4)?,
                    workspace_root: db_to_path(row.get::<_, Option<String>>(5)?),
                    status: project_status_from_str(&row.get::<_, String>(6)?),
                    created_at: parse_ts(&row.get::<_, String>(7)?),
                    updated_at: parse_ts(&row.get::<_, String>(8)?),
                })
            })
            .map_err(|err| err.to_string())?;
        collect_rows(rows)
    }

    fn list_todos_with_conn(&self, conn: &Connection) -> Result<Vec<Todo>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, project_id, title, description, status, priority, assigned_agent_id, acceptance_json, tags_json, created_at, updated_at
                 FROM todos ORDER BY updated_at DESC",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Todo {
                    id: TodoId::new(row.get::<_, String>(0)?),
                    project_id: ProjectId::new(row.get::<_, String>(1)?),
                    title: row.get(2)?,
                    description: row.get(3)?,
                    status: todo_status_from_str(&row.get::<_, String>(4)?),
                    priority: todo_priority_from_str(&row.get::<_, String>(5)?),
                    assigned_agent_id: row.get::<_, Option<String>>(6)?.map(AgentProfileId::new),
                    acceptance_criteria: from_json(&row.get::<_, String>(7)?).unwrap_or_default(),
                    tags: from_json(&row.get::<_, String>(8)?).unwrap_or_default(),
                    created_at: parse_ts(&row.get::<_, String>(9)?),
                    updated_at: parse_ts(&row.get::<_, String>(10)?),
                })
            })
            .map_err(|err| err.to_string())?;
        collect_rows(rows)
    }

    fn list_agents_with_conn(&self, conn: &Connection) -> Result<Vec<AgentProfile>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, company_id, project_id, name, role, model, status, workspace_root, prompt_hint, allowed_tools_json, permission_mode, created_at, updated_at
                 FROM agents ORDER BY updated_at DESC",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AgentProfile {
                    id: AgentProfileId::new(row.get::<_, String>(0)?),
                    company_id: row.get::<_, Option<String>>(1)?.map(CompanyId::new),
                    project_id: row.get::<_, Option<String>>(2)?.map(ProjectId::new),
                    name: row.get(3)?,
                    role: row.get(4)?,
                    model: row.get(5)?,
                    status: agent_status_from_str(&row.get::<_, String>(6)?),
                    workspace_root: db_to_path(row.get::<_, Option<String>>(7)?),
                    prompt_hint: row.get(8)?,
                    allowed_tools: from_json(&row.get::<_, String>(9)?).unwrap_or_default(),
                    permission_mode: row
                        .get::<_, Option<String>>(10)?
                        .and_then(permission_mode_from_str),
                    created_at: parse_ts(&row.get::<_, String>(11)?),
                    updated_at: parse_ts(&row.get::<_, String>(12)?),
                })
            })
            .map_err(|err| err.to_string())?;
        collect_rows(rows)
    }

    fn list_run_links_with_conn(&self, conn: &Connection) -> Result<Vec<RunLink>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, todo_id, agent_id, session_id, workspace_root, worktree_path, branch, parent_session_id, status, created_at, updated_at
                 FROM run_links ORDER BY updated_at DESC",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(RunLink {
                    id: RunLinkId::new(row.get::<_, String>(0)?),
                    todo_id: TodoId::new(row.get::<_, String>(1)?),
                    agent_id: row.get::<_, Option<String>>(2)?.map(AgentProfileId::new),
                    session_id: row.get(3)?,
                    workspace_root: PathBuf::from(row.get::<_, String>(4)?),
                    worktree_path: db_to_path(row.get::<_, Option<String>>(5)?),
                    branch: row.get(6)?,
                    parent_session_id: row.get(7)?,
                    status: session_status_from_str(&row.get::<_, String>(8)?),
                    created_at: parse_ts(&row.get::<_, String>(9)?),
                    updated_at: parse_ts(&row.get::<_, String>(10)?),
                })
            })
            .map_err(|err| err.to_string())?;
        collect_rows(rows)
    }
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, String> {
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|err| err.to_string())?);
    }
    Ok(items)
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|err| err.to_string())
}

fn from_json<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(value)
}

fn parse_ts(value: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|ts| ts.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn clean_opt(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn clean_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect()
}

fn path_to_db(path: Option<&PathBuf>) -> Option<String> {
    path.map(|value| value.to_string_lossy().to_string())
}

fn db_to_path(value: Option<String>) -> Option<PathBuf> {
    value.map(PathBuf::from)
}

fn new_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}-{now:x}-{counter:x}")
}

fn slugify(slug: &str, fallback_name: &str) -> String {
    let source = if slug.trim().is_empty() {
        fallback_name
    } else {
        slug
    };
    let mut out = String::new();
    let mut last_dash = false;
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn status_to_str_company(value: CompanyStatus) -> &'static str {
    match value {
        CompanyStatus::Active => "active",
        CompanyStatus::Archived => "archived",
    }
}

fn company_status_from_str(value: &str) -> CompanyStatus {
    match value {
        "archived" => CompanyStatus::Archived,
        _ => CompanyStatus::Active,
    }
}

fn status_to_str_project(value: ProjectStatus) -> &'static str {
    match value {
        ProjectStatus::Active => "active",
        ProjectStatus::Paused => "paused",
        ProjectStatus::Archived => "archived",
    }
}

fn project_status_from_str(value: &str) -> ProjectStatus {
    match value {
        "paused" => ProjectStatus::Paused,
        "archived" => ProjectStatus::Archived,
        _ => ProjectStatus::Active,
    }
}

fn status_to_str_todo(value: TodoStatus) -> &'static str {
    match value {
        TodoStatus::Backlog => "backlog",
        TodoStatus::Ready => "ready",
        TodoStatus::InProgress => "in_progress",
        TodoStatus::InReview => "in_review",
        TodoStatus::Blocked => "blocked",
        TodoStatus::Done => "done",
        TodoStatus::Cancelled => "cancelled",
    }
}

fn todo_status_from_str(value: &str) -> TodoStatus {
    match value {
        "ready" => TodoStatus::Ready,
        "in_progress" => TodoStatus::InProgress,
        "in_review" => TodoStatus::InReview,
        "blocked" => TodoStatus::Blocked,
        "done" => TodoStatus::Done,
        "cancelled" => TodoStatus::Cancelled,
        _ => TodoStatus::Backlog,
    }
}

fn priority_to_str(value: TodoPriority) -> &'static str {
    match value {
        TodoPriority::Low => "low",
        TodoPriority::Medium => "medium",
        TodoPriority::High => "high",
        TodoPriority::Critical => "critical",
    }
}

fn todo_priority_from_str(value: &str) -> TodoPriority {
    match value {
        "low" => TodoPriority::Low,
        "high" => TodoPriority::High,
        "critical" => TodoPriority::Critical,
        _ => TodoPriority::Medium,
    }
}

fn status_to_str_agent(value: AgentProfileStatus) -> &'static str {
    match value {
        AgentProfileStatus::Active => "active",
        AgentProfileStatus::Idle => "idle",
        AgentProfileStatus::Disabled => "disabled",
        AgentProfileStatus::Archived => "archived",
    }
}

fn agent_status_from_str(value: &str) -> AgentProfileStatus {
    match value {
        "active" => AgentProfileStatus::Active,
        "disabled" => AgentProfileStatus::Disabled,
        "archived" => AgentProfileStatus::Archived,
        _ => AgentProfileStatus::Idle,
    }
}

fn permission_mode_to_str(value: nca_common::config::PermissionMode) -> &'static str {
    match value {
        nca_common::config::PermissionMode::Default => "default",
        nca_common::config::PermissionMode::Plan => "plan",
        nca_common::config::PermissionMode::AcceptEdits => "accept-edits",
        nca_common::config::PermissionMode::DontAsk => "dont-ask",
        nca_common::config::PermissionMode::BypassPermissions => "bypass-permissions",
    }
}

fn permission_mode_from_str(value: String) -> Option<nca_common::config::PermissionMode> {
    match value.as_str() {
        "default" => Some(nca_common::config::PermissionMode::Default),
        "plan" => Some(nca_common::config::PermissionMode::Plan),
        "accept-edits" => Some(nca_common::config::PermissionMode::AcceptEdits),
        "dont-ask" => Some(nca_common::config::PermissionMode::DontAsk),
        "bypass-permissions" => Some(nca_common::config::PermissionMode::BypassPermissions),
        _ => None,
    }
}

fn session_status_to_str(value: nca_common::session::SessionStatus) -> &'static str {
    match value {
        nca_common::session::SessionStatus::Running => "running",
        nca_common::session::SessionStatus::Completed => "completed",
        nca_common::session::SessionStatus::Error => "error",
        nca_common::session::SessionStatus::Cancelled => "cancelled",
    }
}

fn session_status_from_str(value: &str) -> nca_common::session::SessionStatus {
    match value {
        "running" => nca_common::session::SessionStatus::Running,
        "error" => nca_common::session::SessionStatus::Error,
        "cancelled" => nca_common::session::SessionStatus::Cancelled,
        _ => nca_common::session::SessionStatus::Completed,
    }
}

#[cfg(test)]
mod tests {
    use super::OrchestratorStore;
    use nca_common::orchestration::{DesktopMode, NewCompany, NewProject, NewTodo};
    use tempfile::tempdir;

    #[test]
    fn store_roundtrip_persists_entities() {
        let dir = tempdir().expect("tempdir");
        let store = OrchestratorStore::new(dir.path().join("orchestrator.db"));

        let company = store
            .create_company(NewCompany {
                name: "Acme".into(),
                description: Some("Company".into()),
            })
            .expect("create company");
        let project = store
            .create_project(NewProject {
                company_id: company.id.clone(),
                name: "Monitor".into(),
                slug: "monitor".into(),
                description: None,
                workspace_root: None,
            })
            .expect("create project");
        store
            .create_todo(NewTodo {
                project_id: project.id.clone(),
                title: "Ship dashboard".into(),
                description: None,
                priority: Default::default(),
                acceptance_criteria: vec!["cards render".into()],
            })
            .expect("create todo");
        store.save_mode(DesktopMode::ProjectAi).expect("save mode");

        let snapshot = store.load_snapshot().expect("snapshot");
        assert_eq!(snapshot.companies.len(), 1);
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.todos.len(), 1);
        assert_eq!(snapshot.mode.mode, DesktopMode::ProjectAi);
    }
}
