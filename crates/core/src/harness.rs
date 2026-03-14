use nca_common::config::{NcaConfig, PermissionMode};
use nca_common::session::OrchestrationContext;
use std::path::Path;

use crate::skills::SkillCatalog;

const BUILT_IN_SYSTEM_PROMPT: &str = r#"You are nca, a native Rust coding assistant running in a terminal workspace.

Identity:
- Act like the default operator for this repository, not a generic code assistant.
- Work only inside the provided workspace unless explicitly instructed otherwise.
- Prefer precise, verifiable changes over broad speculative rewrites.

Product priorities:
- Rust-native only. Do not introduce JavaScript, Node.js, Electron, Tauri, or web wrappers unless the user explicitly asks for them.
- MiniMax is the primary provider path. Treat MiniMax quality, config, and diagnostics as first-class.
- The desktop monitor app (`nca-monitor`) is the primary user experience. The CLI (`nca`) is secondary and mainly for power users, orchestration, and debugging.
- For desktop UX, prefer egui/eframe patterns that fit the existing native architecture.

Architecture boundaries:
- Keep crate responsibilities narrow and explicit.
- `nca-common` is for shared types and config.
- `nca-core` is for agent logic, providers, harness, and tool protocol.
- `nca-runtime` is for session lifecycle, persistence, IPC, worktrees, and supervision.
- `nca-cli` is for terminal UX only.
- `nca-monitor` must depend on `nca-common` and `nca-runtime`, and must not import `nca-core` or `nca-cli`.
- Subagents should be child sessions with their own worktrees, visible lineage, and explicit parent-child relationships.

Execution rules:
- Inspect the repository before making assumptions.
- For non-trivial work, plan first, then implement in bounded steps.
- Prefer small, testable changes that preserve the existing architecture.
- Re-read only the most relevant files and avoid dumping unnecessary context into a single turn.
- Prefer fast local signals first: top-level listing, targeted search, focused file reads, and symbol-level inspection.

Tool and validation rules:
- Use list/search/read tools first to build a plan.
- Use write/create tools only after enough context is gathered.
- Validate important changes with tests, checks, or other concrete signals before claiming success.
- If a command or edit could be destructive, expensive, or policy-sensitive, ask for approval or explain why it is needed.
- Empty provider completions, empty tool results, or obviously invalid outputs must fail loudly instead of being treated as success.
- Do not pretend a tool, provider, or validation step succeeded if it did not.

Headless and orchestrator rules:
- Headless runs must behave predictably for external orchestrators.
- Respect orchestration metadata when present, but treat it as coordination context only.
- Do not assume callbacks, remote APIs, or external services exist unless they are explicitly provided.
- If a headless run needs approval and approval is unavailable, fail clearly instead of stalling.

Response style:
- Be concise, actionable, and explicit about progress.
- State important constraints, risks, and verification results plainly.
"#;

/// Build the layered system prompt from built-in + project + local instructions.
pub fn build_system_prompt(
    config: &NcaConfig,
    workspace_root: &Path,
    orchestration: Option<&OrchestrationContext>,
) -> String {
    let mut sections = Vec::new();

    if config.harness.built_in_enabled {
        sections.push(BUILT_IN_SYSTEM_PROMPT.trim().to_string());
        if let Some(mode_section) = permission_mode_section(config.permissions.mode) {
            sections.push(mode_section);
        }
    }

    if let Some(text) =
        read_if_exists(&workspace_root.join(&config.harness.project_instructions_path))
    {
        if !text.trim().is_empty() {
            sections.push(format!("Project Instructions:\n{}", text.trim()));
        }
    }

    if let Some(text) =
        read_if_exists(&workspace_root.join(&config.harness.local_instructions_path))
    {
        if !text.trim().is_empty() {
            sections.push(format!("Local Instructions:\n{}", text.trim()));
        }
    }

    if let Some(section) = skills_section(workspace_root, &config.harness.skill_directories) {
        sections.push(section);
    }

    if let Some(section) = orchestration_context_section(orchestration) {
        sections.push(section);
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
        section.push_str(&skill.manifest_summary());
        section.push('\n');
    }
    section.push_str(
        "\nUse these skill summaries when relevant. Full skill instructions are loaded only when explicitly invoked by the user or REPL.",
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
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn built_in_prompt_includes_repo_specific_directives() {
        let config = NcaConfig::default();
        let temp = tempdir().expect("tempdir");

        let prompt = build_system_prompt(&config, temp.path(), None);

        assert!(prompt.contains("Rust-native only."));
        assert!(prompt.contains("MiniMax is the primary provider path."));
        assert!(
            prompt.contains(
                "The desktop monitor app (`nca-monitor`) is the primary user experience."
            )
        );
        assert!(prompt.contains("Subagents should be child sessions with their own worktrees"));
        assert!(prompt.contains("must fail loudly instead of being treated as success"));
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
        fs::write(temp.path().join(".ncarc"), "project rule").expect("write project instructions");
        fs::create_dir_all(temp.path().join(".nca")).expect("create local dir");
        fs::write(temp.path().join(".nca/instructions.md"), "local rule")
            .expect("write local instructions");
        fs::write(
            temp.path().join(".nca/skills/review/SKILL.md"),
            "---\nname: Review\ncommand: review\ndescription: Review workflow\n---\nReview carefully.\n",
        )
        .expect("write skill");

        let orchestration = OrchestrationContext {
            orchestrator: Some("paperclip".into()),
            run_id: Some("run-123".into()),
            task_id: None,
            task_ref: None,
            parent_run_id: None,
            callback_url: None,
            metadata: BTreeMap::new(),
        };

        let prompt = build_system_prompt(&config, temp.path(), Some(&orchestration));

        let identity_idx = prompt.find("Identity:").expect("built-in section");
        let permission_idx = prompt
            .find("Permission Mode: plan")
            .expect("permission section");
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

        assert!(identity_idx < permission_idx);
        assert!(permission_idx < project_idx);
        assert!(project_idx < local_idx);
        assert!(local_idx < skills_idx);
        assert!(skills_idx < orchestration_idx);
    }

    #[test]
    fn project_and_local_instructions_are_added_not_replacing_built_in_prompt() {
        let config = NcaConfig::default();
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join(".nca")).expect("create local dir");
        fs::write(temp.path().join(".ncarc"), "project override").expect("write .ncarc");
        fs::write(temp.path().join(".nca/instructions.md"), "local override")
            .expect("write local instructions");

        let prompt = build_system_prompt(&config, temp.path(), None);

        assert!(prompt.contains("Product priorities:"));
        assert!(prompt.contains("Project Instructions:\nproject override"));
        assert!(prompt.contains("Local Instructions:\nlocal override"));
    }
}
