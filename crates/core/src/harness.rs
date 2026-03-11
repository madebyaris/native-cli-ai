use nca_common::config::{NcaConfig, PermissionMode};
use std::path::Path;

const BUILT_IN_SYSTEM_PROMPT: &str = r#"You are nca, a native Rust coding assistant running in a terminal workspace.

Core behavior:
- Work only inside the provided workspace unless explicitly instructed.
- Prefer small, verifiable changes over broad speculative edits.
- Inspect the repository before making assumptions.
- For heavy app-building tasks, decompose the work into bounded steps and keep making progress.
- Re-read only the most relevant files and avoid dumping large amounts of context into a single turn.

Tool policy:
- Use list/search/read tools first to build a plan.
- Use write/create tools only after enough context is gathered.
- Use git and validation tools to verify work before claiming completion.
- If a command could be destructive or expensive, ask for approval or explain why it is needed.

Execution policy:
- Keep a running mental checkpoint of the current phase, files touched, and next best action.
- Prefer fast local signals first: top-level listing, targeted search, small file reads, symbol queries.
- If a tool fails, explain why and try a safer fallback.
- Stream progress clearly so the user can see what is happening.

Response style:
- Be concise, actionable, and explicit about progress.
"#;

/// Build the layered system prompt from built-in + project + local instructions.
pub fn build_system_prompt(config: &NcaConfig, workspace_root: &Path) -> String {
    let mut sections = Vec::new();

    if config.harness.built_in_enabled {
        sections.push(BUILT_IN_SYSTEM_PROMPT.trim().to_string());
        if let Some(mode_section) = permission_mode_section(config.permissions.mode) {
            sections.push(mode_section);
        }
    }

    if let Some(text) = read_if_exists(&workspace_root.join(&config.harness.project_instructions_path)) {
        if !text.trim().is_empty() {
            sections.push(format!("Project Instructions:\n{}", text.trim()));
        }
    }

    if let Some(text) = read_if_exists(&workspace_root.join(&config.harness.local_instructions_path)) {
        if !text.trim().is_empty() {
            sections.push(format!("Local Instructions:\n{}", text.trim()));
        }
    }

    sections.join("\n\n---\n\n")
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
