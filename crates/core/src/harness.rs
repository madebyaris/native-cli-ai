use nca_common::config::{NcaConfig, PermissionMode};
use nca_common::session::OrchestrationContext;
use nca_common::todo::AgentTodo;
use std::path::{Path, PathBuf};

use crate::skills::SkillCatalog;

const MAX_MEMORY_NOTES: usize = 12;
const MAX_MEMORY_CHARS: usize = 400;
const MAX_TODOS_IN_PROMPT: usize = 20;

const BUILT_IN_IDENTITY: &str = r#"You are nca, a native Rust coding assistant running in a terminal workspace.

Identity:
- Act like the default operator for this repository, not a generic code assistant.
- Work only inside the provided workspace unless explicitly instructed otherwise.
- Prefer precise, verifiable changes over broad speculative rewrites.

Product priorities:
- Rust-native only. Do not introduce JavaScript, Node.js, Electron, Tauri, or web wrappers unless the user explicitly asks for them.
- MiniMax is the primary provider path. Treat MiniMax quality, config, and diagnostics as first-class.
- The CLI (`nca`) is the product surface: terminal UX, JSON/NDJSON streams, and the Unix-socket IPC used for approvals and attach.

Architecture boundaries:
- Keep crate responsibilities narrow and explicit.
- `nca-common` is for shared types and config.
- `nca-core` is for agent logic, providers, harness, and tool protocol.
- `nca-runtime` is for session lifecycle, persistence, IPC, worktrees, and supervision.
- `nca-cli` is for terminal UX only.
- Subagents should be child sessions with their own worktrees, visible lineage, and explicit parent-child relationships.

Execution rules:
- Inspect the repository before making assumptions.
- For non-trivial work, plan first, then implement in bounded steps.
- Prefer small, testable changes that preserve the existing architecture.
- Re-read only the most relevant files and avoid dumping unnecessary context into a single turn.
- Prefer fast local signals first: top-level listing, targeted search, focused file reads, and symbol-level inspection.

Headless and orchestrator rules:
- Headless runs must behave predictably for external orchestrators.
- Respect orchestration metadata when present, but treat it as coordination context only.
- Do not assume callbacks, remote APIs, or external services exist unless they are explicitly provided.
- If a headless run needs approval and approval is unavailable, fail clearly instead of stalling.

Response style:
- Be concise, actionable, and explicit about progress.
- State important constraints, risks, and verification results plainly.
"#;

const TOOL_PLAYBOOK: &str = r#"Tool playbook:
- Explore with `list_directory`, `search_code`, `read_file`, and `query_symbols` before editing.
- Prefer edit tools in this order:
  1. `replace_match` for a search hit with an exact line/column location
  2. `edit_file` for a unique exact string replacement
  3. `apply_patch` for multiple hunks in one existing file
  4. `write_file` / `create_directory` only for new paths
- Validate important changes with tests, checks, or other concrete signals before claiming success.
- Empty provider completions, empty tool results, or obviously invalid outputs must fail loudly instead of being treated as success.
- Do not pretend a tool, provider, or validation step succeeded if it did not.
- When the user attaches images via nca (pasted or `/image`), the runtime already runs MiniMax native vision and injects a text description. Do **not** use `fetch_url` for session attachment paths or `file:` URLs to "load" those images.
- When you need structured choices from the user, use `ask_question` with clear options and always set `suggested_answer`.
- For multi-step work, keep an explicit session todo list via `update_todos` (full list replacement each call). Keep at most one item `in_progress`.
- If a command or edit could be destructive, expensive, or policy-sensitive, ask for approval or explain why it is needed.
"#;

/// One memory note rendered into the dynamic harness section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessMemoryNote {
    pub kind: String,
    pub content: String,
}

/// Dynamic per-turn context for system prompt assembly.
/// Built by the runtime; core stays free of git/memory I/O.
#[derive(Debug, Clone, Default)]
pub struct HarnessSnapshot {
    pub workspace_root: PathBuf,
    pub cwd_display: String,
    pub git_branch: Option<String>,
    pub model: String,
    pub permission_mode: String,
    pub agent_profile: Option<String>,
    pub memory_notes: Vec<HarnessMemoryNote>,
    pub todos: Vec<AgentTodo>,
}

impl HarnessSnapshot {
    pub fn capped_memory_notes(&self) -> Vec<HarnessMemoryNote> {
        self.memory_notes
            .iter()
            .rev()
            .take(MAX_MEMORY_NOTES)
            .map(|note| {
                let content = truncate_chars(&note.content, MAX_MEMORY_CHARS);
                HarnessMemoryNote {
                    kind: note.kind.clone(),
                    content,
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    pub fn capped_todos(&self) -> &[AgentTodo] {
        let end = self.todos.len().min(MAX_TODOS_IN_PROMPT);
        &self.todos[..end]
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Build the layered system prompt from built-in + dynamic snapshot + project files.
pub fn build_system_prompt(
    config: &NcaConfig,
    snapshot: &HarnessSnapshot,
    orchestration: Option<&OrchestrationContext>,
) -> String {
    let mut sections = Vec::new();
    let workspace_root = if snapshot.workspace_root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        snapshot.workspace_root.as_path()
    };

    if config.harness.built_in_enabled {
        sections.push(BUILT_IN_IDENTITY.trim().to_string());
        if let Some(mode_section) = permission_mode_section(config.permissions.mode) {
            sections.push(mode_section);
        }
    }

    if let Some(section) = environment_section(snapshot) {
        sections.push(section);
    }
    if let Some(section) = todos_section(snapshot) {
        sections.push(section);
    }
    if let Some(section) = memory_section(snapshot) {
        sections.push(section);
    }

    if let Some(text) = read_if_exists(&workspace_root.join("AGENTS.md"))
        && !text.trim().is_empty()
    {
        sections.push(format!("AGENTS.md Instructions:\n{}", text.trim()));
    }

    if let Some(text) =
        read_if_exists(&workspace_root.join(&config.harness.project_instructions_path))
        && !text.trim().is_empty()
    {
        sections.push(format!("Project Instructions:\n{}", text.trim()));
    }

    if let Some(text) =
        read_if_exists(&workspace_root.join(&config.harness.local_instructions_path))
        && !text.trim().is_empty()
    {
        sections.push(format!("Local Instructions:\n{}", text.trim()));
    }

    if let Some(section) = skills_section(workspace_root, &config.harness.skill_directories) {
        sections.push(section);
    }

    if let Some(section) = orchestration_context_section(orchestration) {
        sections.push(section);
    }

    if config.harness.built_in_enabled {
        sections.push(TOOL_PLAYBOOK.trim().to_string());
    }

    sections.join("\n\n---\n\n")
}

fn environment_section(snapshot: &HarnessSnapshot) -> Option<String> {
    if snapshot.cwd_display.is_empty()
        && snapshot.model.is_empty()
        && snapshot.permission_mode.is_empty()
        && snapshot.git_branch.is_none()
        && snapshot.agent_profile.is_none()
    {
        return None;
    }
    let mut lines = vec!["Environment:".to_string()];
    if !snapshot.cwd_display.is_empty() {
        lines.push(format!("- cwd: {}", snapshot.cwd_display));
    }
    if let Some(branch) = &snapshot.git_branch {
        lines.push(format!("- git_branch: {branch}"));
    }
    if !snapshot.model.is_empty() {
        lines.push(format!("- model: {}", snapshot.model));
    }
    if !snapshot.permission_mode.is_empty() {
        lines.push(format!("- permission_mode: {}", snapshot.permission_mode));
    }
    if let Some(profile) = &snapshot.agent_profile {
        lines.push(format!("- agent_profile: {profile}"));
    }
    Some(lines.join("\n"))
}

fn todos_section(snapshot: &HarnessSnapshot) -> Option<String> {
    let todos = snapshot.capped_todos();
    if todos.is_empty() {
        return None;
    }
    let mut lines = vec!["Session Todos:".to_string()];
    for todo in todos {
        lines.push(format!(
            "- [{}] {} ({})",
            todo.status.as_str(),
            todo.content,
            todo.id
        ));
    }
    if snapshot.todos.len() > todos.len() {
        lines.push(format!(
            "- …and {} more (see /todos)",
            snapshot.todos.len() - todos.len()
        ));
    }
    Some(lines.join("\n"))
}

fn memory_section(snapshot: &HarnessSnapshot) -> Option<String> {
    let notes = snapshot.capped_memory_notes();
    if notes.is_empty() {
        return None;
    }
    let mut lines = vec!["Memory Notes:".to_string()];
    for note in notes {
        lines.push(format!("- [{}] {}", note.kind, note.content));
    }
    Some(lines.join("\n"))
}

fn read_if_exists(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn permission_mode_section(mode: PermissionMode) -> Option<String> {
    match mode {
        PermissionMode::Plan => Some(
            "Permission Mode: plan\n- You must not modify files or run shell commands.\n- Inspect, search, read, research the web, and propose the next steps only.\n- If asked to change code, explain what would change instead of claiming it was done."
                .into(),
        ),
        PermissionMode::DontAsk => Some(
            "Permission Mode: dont-ask\n- Only use automatically allowed tools.\n- If a task needs blocked tools, explain the limitation instead of pretending it succeeded."
                .into(),
        ),
        PermissionMode::AcceptEdits => Some(
            "Permission Mode: accept-edits\n- File edits are allowed automatically.\n- Destructive actions and shell execution may still require caution."
                .into(),
        ),
        PermissionMode::BypassPermissions => Some(
            "Permission Mode: bypass-permissions\n- Tools are broadly available, but still work carefully and verify before claiming success."
                .into(),
        ),
        PermissionMode::Default => None,
    }
}

fn skills_section(
    workspace_root: &Path,
    skill_directories: &[std::path::PathBuf],
) -> Option<String> {
    let skills = SkillCatalog::discover(workspace_root, skill_directories).ok()?;
    if skills.is_empty() {
        return None;
    }

    let mut section = String::from("Available Skills:\n");
    for skill in skills {
        let summary = skill.manifest_summary();
        let line = truncate_chars(&summary, 160);
        section.push_str(&line);
        section.push('\n');
    }
    section.push_str(
        "\nUse the invoke_skill tool to load full instructions when a task matches a skill.",
    );
    Some(section)
}

fn orchestration_context_section(orchestration: Option<&OrchestrationContext>) -> Option<String> {
    let orchestration = orchestration?;
    let mut lines = vec!["Execution Context:".to_string()];

    if let Some(orchestrator) = &orchestration.orchestrator {
        lines.push(format!("- orchestrator: {orchestrator}"));
    }
    if let Some(run_id) = &orchestration.run_id {
        lines.push(format!("- run_id: {run_id}"));
    }
    if let Some(task_id) = &orchestration.task_id {
        lines.push(format!("- task_id: {task_id}"));
    }
    if let Some(task_ref) = &orchestration.task_ref {
        lines.push(format!("- task_ref: {task_ref}"));
    }
    if let Some(parent_run_id) = &orchestration.parent_run_id {
        lines.push(format!("- parent_run_id: {parent_run_id}"));
    }
    if let Some(callback_url) = &orchestration.callback_url {
        lines.push(format!("- callback_url: {callback_url}"));
    }
    if !orchestration.metadata.is_empty() {
        lines.push("- metadata:".to_string());
        for (key, value) in &orchestration.metadata {
            lines.push(format!("  - {key}: {value}"));
        }
    }

    lines.push(
        "- Use this only as coordination metadata for the current run. Do not assume external APIs or callbacks exist unless the user or tools explicitly provide them."
            .to_string(),
    );

    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::config::NcaConfig;
    use nca_common::todo::{TodoSource, TodoStatus};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    fn empty_snapshot(workspace: &Path) -> HarnessSnapshot {
        HarnessSnapshot {
            workspace_root: workspace.to_path_buf(),
            cwd_display: workspace.display().to_string(),
            git_branch: Some("main".into()),
            model: "MiniMax-M2.5".into(),
            permission_mode: "default".into(),
            agent_profile: Some("@build".into()),
            memory_notes: Vec::new(),
            todos: Vec::new(),
        }
    }

    #[test]
    fn built_in_prompt_includes_repo_specific_directives() {
        let config = NcaConfig::default();
        let temp = tempdir().expect("tempdir");
        let snapshot = empty_snapshot(temp.path());

        let prompt = build_system_prompt(&config, &snapshot, None);

        assert!(prompt.contains("Rust-native only."));
        assert!(prompt.contains("MiniMax is the primary provider path."));
        assert!(prompt.contains("The CLI (`nca`) is the product surface:"));
        assert!(prompt.contains("Subagents should be child sessions with their own worktrees"));
        assert!(prompt.contains("must fail loudly instead of being treated as success"));
        assert!(prompt.contains("replace_match"));
        let replace_idx = prompt.find("replace_match").expect("replace");
        let write_idx = prompt.find("`write_file`").expect("write");
        assert!(replace_idx < write_idx);
    }

    #[test]
    fn layers_sections_in_stable_order() {
        let config = NcaConfig {
            permissions: nca_common::config::PermissionConfig {
                mode: PermissionMode::Plan,
                ..Default::default()
            },
            ..Default::default()
        };
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join(".nca/skills/review")).expect("create skills dir");
        fs::write(temp.path().join("AGENTS.md"), "agent rule").expect("write AGENTS.md");
        fs::write(temp.path().join(".ncarc"), "project rule").expect("write project instructions");
        fs::create_dir_all(temp.path().join(".nca")).expect("create local dir");
        fs::write(temp.path().join(".nca/instructions.md"), "local rule")
            .expect("write local instructions");
        fs::write(
            temp.path().join(".nca/skills/review/SKILL.md"),
            "---\nname: Review\ncommand: review\ndescription: Review workflow\n---\nReview carefully.\n",
        )
        .expect("write skill");

        let mut snapshot = empty_snapshot(temp.path());
        snapshot.todos = vec![AgentTodo {
            id: "1".into(),
            content: "Ship harness".into(),
            status: TodoStatus::InProgress,
            source: Some(TodoSource::Agent),
        }];
        snapshot.memory_notes = vec![HarnessMemoryNote {
            kind: "note".into(),
            content: "Prefer product home store".into(),
        }];

        let orchestration = OrchestrationContext {
            orchestrator: Some("paperclip".into()),
            run_id: Some("run-123".into()),
            task_id: None,
            task_ref: None,
            parent_run_id: None,
            callback_url: None,
            metadata: BTreeMap::new(),
        };

        let prompt = build_system_prompt(&config, &snapshot, Some(&orchestration));

        let identity_idx = prompt.find("Identity:").expect("built-in section");
        let permission_idx = prompt
            .find("Permission Mode: plan")
            .expect("permission section");
        let env_idx = prompt.find("Environment:").expect("environment");
        let todos_idx = prompt.find("Session Todos:").expect("todos");
        let memory_idx = prompt.find("Memory Notes:").expect("memory");
        let agents_idx = prompt
            .find("AGENTS.md Instructions:\nagent rule")
            .expect("agents instructions");
        let project_idx = prompt
            .find("Project Instructions:\nproject rule")
            .expect("project instructions");
        let local_idx = prompt
            .find("Local Instructions:\nlocal rule")
            .expect("local instructions");
        let skills_idx = prompt.find("Available Skills:").expect("skills section");
        let orchestration_idx = prompt
            .find("Execution Context:")
            .expect("orchestration section");
        let playbook_idx = prompt.find("Tool playbook:").expect("playbook");

        assert!(identity_idx < permission_idx);
        assert!(permission_idx < env_idx);
        assert!(env_idx < todos_idx);
        assert!(todos_idx < memory_idx);
        assert!(memory_idx < agents_idx);
        assert!(agents_idx < project_idx);
        assert!(project_idx < local_idx);
        assert!(local_idx < skills_idx);
        assert!(skills_idx < orchestration_idx);
        assert!(orchestration_idx < playbook_idx);
    }

    #[test]
    fn agents_project_and_local_instructions_are_added_not_replacing_built_in_prompt() {
        let config = NcaConfig::default();
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join(".nca")).expect("create local dir");
        fs::write(temp.path().join("AGENTS.md"), "agents override").expect("write AGENTS.md");
        fs::write(temp.path().join(".ncarc"), "project override").expect("write .ncarc");
        fs::write(temp.path().join(".nca/instructions.md"), "local override")
            .expect("write local instructions");

        let prompt = build_system_prompt(&config, &empty_snapshot(temp.path()), None);

        assert!(prompt.contains("Product priorities:"));
        assert!(prompt.contains("AGENTS.md Instructions:\nagents override"));
        assert!(prompt.contains("Project Instructions:\nproject override"));
        assert!(prompt.contains("Local Instructions:\nlocal override"));
    }
}
