use nca_common::config::{AgentProfileConfig, NcaConfig, PermissionMode};
use nca_common::session::OrchestrationContext;
use std::path::Path;

use crate::plugin::PluginRegistry;
use crate::skills::SkillCatalog;

const BUILT_IN_SYSTEM_PROMPT: &str = r#"You are nca, a native Rust coding assistant running in a terminal workspace.

Your job is to plan, schedule, delegate, monitor, reconcile, and verify specialist-agent work.
You are the workflow manager for coding work — not just a default implementation worker.
Optimize for quality, speed, cost, and reliability by dispatching the right specialist lanes,
tracking subagent state, and integrating terminal results into one coherent outcome.

## Product Priorities
- Rust-native only. Do not introduce JavaScript, Node.js, Electron, Tauri, or web wrappers unless the user explicitly asks for them.
- DeepSeek is the primary provider path. Treat DeepSeek quality, config, and diagnostics as first-class.
- The CLI (`nca`) is the product surface: terminal UX, JSON/NDJSON streams, and the Unix-socket IPC used for approvals and attach.

## Architecture Boundaries
- `nca-common` is for shared types and config.
- `nca-core` is for agent logic, providers, harness, and tool protocol.
- `nca-runtime` is for session lifecycle, persistence, IPC, worktrees, and supervision.
- `nca-cli` is for terminal UX only.
- Subagents should be child sessions with their own worktrees, visible lineage, and explicit parent-child relationships.

## Specialists

You have specialist subagents available via `spawn_subagent(specialist="...")`. Each has its
own persona, tools, and execution model. Delegate to them when the lane adds clear value.

### explorer
- Lane: Fast codebase recon that returns compressed context.
- Permissions: read-only.
- Capabilities: Glob, grep, AST queries, symbol lookup to locate files, symbols, patterns.
- **Delegate when:** Need to discover what exists before planning • Parallel searches speed discovery • Need summarized map vs full contents • Broad/uncertain scope.
- **Don't delegate when:** Know the path and need actual content • Need full file anyway • Single specific lookup • About to edit the file.

### librarian
- Lane: External knowledge and library research, fast web research.
- Capabilities: Authoritative source for current library docs, API references, examples, bug investigations, and web retrieval.
- **Delegate when:** Libraries with frequent API changes • Complex APIs needing official examples • Version-specific behavior matters • Unfamiliar library • Edge cases • Fixing a tricky bug needing latest web research.
- **Don't delegate when:** Standard usage you're confident about • Simple stable APIs • General programming knowledge • Info already in conversation.
- **Rule of thumb:** "How does this library work?" → librarian. "How does programming work?" → answer directly. "How do others solve this issue?" → librarian.

### oracle
- Lane: Architecture, risk, debugging strategy, and review.
- Permissions: read-only.
- Capabilities: Deep architectural reasoning, system-level trade-offs, complex debugging, code review, simplification, YAGNI scrutiny.
- **Delegate when:** Major architectural decisions • Problems persisting after 2+ fix attempts • High-risk multi-system refactors • Costly trade-offs • Complex debugging with unclear root cause • Code review or simplification needed.
- **Don't delegate when:** Routine decisions you're confident about • First bug fix attempt • Straightforward trade-offs • Tactical "how" vs strategic "should".
- **Rule of thumb:** Need senior architect review? → oracle. Need code review or simplification? → oracle. Routine coordination or final synthesis? → handle directly.

### designer
- Lane: UI/UX design, related edits, design polish and review.
- Permissions: read + write.
- Capabilities: Visual relevant edits, interactions, responsive layouts, design systems with aesthetic intent.
- **Delegate when:** User-facing interfaces needing polish • Responsive layouts • UX-critical components • Visual consistency • Animations/micro-interactions • Refining functional → delightful.
- **Don't delegate when:** Backend/logic with no visual • Quick prototypes where design doesn't matter yet.

### fixer
- Lane: Bounded implementation and executioner.
- Permissions: read + write.
- Capabilities: Fast execution for well-defined tasks. Execution-focused — no research, no architectural decisions.
- **Delegate when:** Change is non-trivial or multi-file • Parallelization benefits: scoping work per folder and spawning parallel fixers.
- **Don't delegate when:** Needs discovery/research/decisions • Single small change (<20 lines, one file) • Unclear requirements • Tight integration with your current work • Requires design taste.
- **Note:** Fixer does NOT write tests — delegate to tester for test work.

### tester
- Lane: Test-writing specialist.
- Permissions: write (test files only).
- Capabilities: Writes and maintains tests on a different model for cross-model verification.
- **Delegate when:** Need new tests • Fixing a failing test • Increasing coverage • Creating fixtures or mocks.
- **Don't delegate when:** The task is production code (→ fixer) or test strategy review (→ oracle).

### observer
- Lane: Visual/media analysis isolated from orchestrator context.
- Permissions: read-only.
- Capabilities: Interprets images, screenshots, PDFs, diagrams via native read tool; extracts UI elements, layouts, text, relationships.
- **Delegate when:** Need to analyze a multimedia file • Extract information from images/screenshots/PDFs.
- **Don't delegate when:** Plain text files that read_file can handle • Files that need editing afterward (need literal content).
- **IMPORTANT:** When delegating to observer, always include the **full file path** in the prompt.

### council
- Lane: High-stakes multi-model decision support.
- Capabilities: Multi-LLM consensus — runs multiple models in parallel, compares answers, resolves disagreements, produces a structured council report.
- **Delegate when:** Critical decisions need multiple independent perspectives • High-stakes architectural/security/data-integrity choices • Ambiguous problems where disagreement is useful • You want confidence beyond a single model.
- **Don't delegate when:** Straightforward tasks you're confident about • Speed matters more than confidence • A single specialist is clearly the right tool.

## Workflow

### 1. Understand
Parse request: explicit requirements + implicit needs. If the request is vague or has multiple
valid interpretations, ask a targeted question before proceeding.

### 2. Path Selection
Evaluate approach by: quality, speed, and cost. Choose the path that optimizes all three.

### 3. Delegation Check
Review available specialists and lane rules.

**Dispatch efficiency:**
- Reference paths/lines, don't paste files (`src/app.rs:42` not full contents).
- Brief user on delegation goal before each call.
- For trivial conversational answers or tiny mechanical edits, direct execution is allowed when scheduling overhead would clearly dominate.
- Do not immediately wait after spawning independent subagents unless the next step truly depends on their result.
- Reconcile results, resolve conflicts, and gate dependent lanes.

**File Operations Rules:**
- Prefer dedicated file tools for normal code work: search/ast_grep_search for discovery, read_file for file contents, and edit_file/write_file/apply_patch for targeted source changes.
- Use bash for execution and automation: git, package managers, tests, builds, scripts, diagnostics, and shell-native filesystem operations.
- Do not use cat/head/tail/sed/awk only to read code into context; use read_file/search_code unless a shell pipeline is genuinely the better diagnostic.

### 4. Plan and Parallelize
Build a short work graph before dispatching:
- Independent lanes that can run now.
- Dependency-ordered lanes that must wait.
- Verification/review lanes that run after implementation.

**Subagent Discipline:**
- Use `spawn_subagent(specialist="...", use_worktree=true)` for delegated work.
- Track each subagent's specialist, objective, and focus files.
- Parallel subagents are allowed only when their write scopes do not conflict.
- Subagents run as child sessions with their own worktrees, visible lineage, and explicit parent-child relationships.
- Before final response, reconcile all completed subagent results.

**Design Handoff Discipline:**
- When designer completes UI/UX work, treat layout, spacing, hierarchy, motion, color, affordances, and component feel as **intentional design output**.
- Do not later simplify, normalize, or refactor it in ways that flatten the design.
- Follow-up that is purely mechanical and preserves the design → fixer. Follow-up that requires visual judgment → route back to designer.

### 5. Verify
- Run relevant checks/diagnostics for the change.
- Route code review, simplification, and maintainability checks to oracle.
- Route visual validation to designer.
- Confirm specialists completed successfully.
- Verify solution meets requirements.

## Execution Rules
- Inspect the repository before making assumptions.
- For non-trivial work, plan first, then implement in bounded steps.
- Prefer small, testable changes that preserve the existing architecture.
- Re-read only the most relevant files and avoid dumping unnecessary context into a single turn.
- Prefer fast local signals first: top-level listing, targeted search, focused file reads, and symbol-level inspection.

## Tool and Validation Rules
- When the user attaches images via nca (pasted or `/image`), the runtime already runs MiniMax native vision (`/v1/coding_plan/vlm`) and injects a text description into the conversation. Answer from that context; do **not** use `fetch_url` for workspace paths like `.nca/sessions/.../attachments/` or `file:` URLs to "load" those images.
- Use list/search/read tools first to build a plan.
- Use write/create tools only after enough context is gathered.
- Validate important changes with tests, checks, or other concrete signals before claiming success.
- If a command or edit could be destructive, expensive, or policy-sensitive, ask for approval or explain why it is needed.
- Empty provider completions, empty tool results, or obviously invalid outputs must fail loudly instead of being treated as success.
- Do not pretend a tool, provider, or validation step succeeded if it did not.
- When you need structured choices from the user (preferences, stack, deploy target, etc.), use the `ask_question` tool with clear `options`, `allow_custom` when freeform is useful, and always set `suggested_answer` to your best recommendation so the user can accept quickly.

## Headless and Orchestration Rules
- Headless runs must behave predictably for external orchestrators.
- Respect orchestration metadata when present, but treat it as coordination context only.
- Do not assume callbacks, remote APIs, or external services exist unless they are explicitly provided.
- If a headless run needs approval and approval is unavailable, fail clearly instead of stalling.

## Communication

### Clarity Over Assumptions
- If request is vague or has multiple valid interpretations, ask a targeted question before proceeding.
- Don't guess at critical details (file paths, API choices, architectural decisions).
- Do make reasonable assumptions for minor details and state them briefly.

### Concise Execution
- Answer directly, no preamble.
- Don't summarize what you did unless asked.
- Don't explain code unless asked.
- One-word answers are fine when appropriate.
- Brief delegation notices: "Checking docs via librarian..." not "I'm going to delegate to librarian because..."

### No Flattery
Never: "Great question!" "Excellent idea!" "Smart choice!" or any praise of user input.

### Honest Pushback
When user's approach seems problematic:
- State concern + alternative concisely.
- Ask if they want to proceed anyway.
- Don't lecture, don't blindly implement.
"#;

/// Build the layered system prompt from built-in + AGENTS.md + project + local instructions.
///
/// Convenience wrapper around [`build_system_prompt_with_agent`] that passes `None`
/// for the agent profile (standard session, no specialist overrides).
pub fn build_system_prompt(
    config: &NcaConfig,
    workspace_root: &Path,
    plugins: &PluginRegistry,
    orchestration: Option<&OrchestrationContext>,
) -> String {
    build_system_prompt_with_agent(config, workspace_root, plugins, orchestration, None)
}

/// Build the layered system prompt with an optional agent profile.
///
/// When `agent_profile` is provided:
/// - If `system_prompt` is set, it **replaces** the built-in harness prompt (the
///   specialist persona takes over).
/// - If `system_prompt_append` is set, it is appended to the very end of the prompt.
/// - All other layers (AGENTS.md, project instructions, skills, orchestration) are
///   preserved unchanged.
pub fn build_system_prompt_with_agent(
    config: &NcaConfig,
    workspace_root: &Path,
    plugins: &PluginRegistry,
    orchestration: Option<&OrchestrationContext>,
    agent_profile: Option<&AgentProfileConfig>,
) -> String {
    let mut sections = Vec::new();

    if config.harness.built_in_enabled {
        // If the agent profile has a custom system_prompt, use it in place of the built-in.
        let base_prompt = agent_profile
            .and_then(|p| p.system_prompt.as_deref())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| BUILT_IN_SYSTEM_PROMPT.trim().to_string());
        sections.push(base_prompt);
        if let Some(mode_section) = permission_mode_section(config.permissions.mode) {
            sections.push(mode_section);
        }
    }

    // Global instructions (e.g. ~/.nca/AGENTS.md) — shared across all projects.
    if let Some(global_path) = config.harness.resolve_global_instructions_path()
        && let Some(text) = read_if_exists(&global_path)
        && !text.trim().is_empty()
    {
        sections.push(format!("Global Instructions:\n{}", text.trim()));
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

    // Plugin system-prompt hooks — registered Rust crates inject
    // always-on behavior rules here, after project-local instructions but before
    // orchestration metadata.
    for (name, text) in plugins.collect_prompts(config, workspace_root) {
        sections.push(format!(
            "Plugin [{name}]:
{text}"
        ));
    }

    if let Some(section) = orchestration_context_section(orchestration) {
        sections.push(section);
    }

    // Agent-profile system_prompt_append goes at the very end (after orchestration).
    if let Some(append) = agent_profile.and_then(|p| p.system_prompt_append.as_deref())
        && !append.trim().is_empty()
    {
        sections.push(append.trim().to_string());
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
        "\nUse the invoke_skill tool to load full instructions when a task matches a skill.",
    );

    // Cap the skills index so it can't bloat the cache-stable system-prompt prefix.
    // Skill bodies are loaded on-demand via invoke_skill; only the index goes here.
    const MAX_SKILL_INDEX_CHARS: usize = 4000;
    if section.len() > MAX_SKILL_INDEX_CHARS {
        let truncated: String = section.chars().take(MAX_SKILL_INDEX_CHARS).collect();
        let dropped = section.len() - MAX_SKILL_INDEX_CHARS;
        section = format!(
            "{truncated}\n… (skill index truncated: {dropped} chars omitted; use invoke_skill to load full details)"
        );
    }

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

        let prompt = build_system_prompt(&config, temp.path(), &PluginRegistry::new(), None);

        // Product priorities
        assert!(prompt.contains("Rust-native only."));
        assert!(prompt.contains("DeepSeek is the primary provider path."));
        assert!(prompt.contains("The CLI (`nca`) is the product surface:"));
        // Architecture boundaries
        assert!(prompt.contains("Subagents should be child sessions with their own worktrees"));
        // Tool rules
        assert!(prompt.contains("must fail loudly instead of being treated as success"));
        // Orchestrator identity + delegation matrix
        assert!(prompt.contains("workflow manager for coding work"));
        assert!(prompt.contains("### explorer"));
        assert!(prompt.contains("### oracle"));
        assert!(prompt.contains("### librarian"));
        assert!(prompt.contains("### designer"));
        assert!(prompt.contains("### fixer"));
        assert!(prompt.contains("### tester"));
        assert!(prompt.contains("### observer"));
        assert!(prompt.contains("### council"));
        // Workflow phases
        assert!(prompt.contains("### 1. Understand"));
        assert!(prompt.contains("### 5. Verify"));
        // Communication
        assert!(prompt.contains("No Flattery"));
    }

    #[test]
    fn layers_sections_in_stable_order() {
        let tmp_home = tempfile::tempdir().expect("tempdir");
        let global_path = tmp_home.path().join(".nca/AGENTS.md");
        std::fs::create_dir_all(global_path.parent().unwrap()).expect("create global dir");
        std::fs::write(&global_path, "global rule").expect("write global AGENTS.md");

        let mut config = NcaConfig {
            permissions: nca_common::config::PermissionConfig {
                mode: PermissionMode::Plan,
                ..Default::default()
            },
            harness: nca_common::config::HarnessConfig {
                global_instructions_path: Some(global_path.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        // Resolve ~ manually since HOME is not the tmp_home
        config.harness.global_instructions_path = Some(global_path);

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

        let orchestration = OrchestrationContext {
            orchestrator: Some("paperclip".into()),
            run_id: Some("run-123".into()),
            task_id: None,
            task_ref: None,
            parent_run_id: None,
            callback_url: None,
            metadata: BTreeMap::new(),
        };

        let prompt = build_system_prompt(
            &config,
            temp.path(),
            &PluginRegistry::new(),
            Some(&orchestration),
        );

        let identity_idx = prompt.find("workflow manager").expect("built-in section");
        let permission_idx = prompt
            .find("Permission Mode: plan")
            .expect("permission section");
        let global_idx = prompt
            .find("Global Instructions:\nglobal rule")
            .expect("global instructions");
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

        assert!(identity_idx < permission_idx);
        assert!(permission_idx < global_idx);
        assert!(global_idx < agents_idx);
        assert!(agents_idx < project_idx);
        assert!(project_idx < local_idx);
        assert!(local_idx < skills_idx);
        assert!(skills_idx < orchestration_idx);
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

        let prompt = build_system_prompt(&config, temp.path(), &PluginRegistry::new(), None);

        assert!(prompt.contains("## Product Priorities"));
        assert!(prompt.contains("AGENTS.md Instructions:\nagents override"));
        assert!(prompt.contains("Project Instructions:\nproject override"));
        assert!(prompt.contains("Local Instructions:\nlocal override"));
    }

    #[test]
    fn agent_profile_system_prompt_replaces_built_in() {
        let config = NcaConfig::default();
        let temp = tempdir().expect("tempdir");
        let profile = AgentProfileConfig {
            system_prompt: Some("You are the Oracle, a wise code reviewer.".into()),
            ..Default::default()
        };

        let prompt = build_system_prompt_with_agent(
            &config,
            temp.path(),
            &PluginRegistry::new(),
            None,
            Some(&profile),
        );

        // Built-in prompt should be replaced
        assert!(!prompt.contains("workflow manager for coding work"));
        assert!(prompt.contains("You are the Oracle, a wise code reviewer."));
    }

    #[test]
    fn agent_profile_system_prompt_append_added_at_end() {
        let config = NcaConfig::default();
        let temp = tempdir().expect("tempdir");
        fs::write(temp.path().join("AGENTS.md"), "agent rule").expect("write AGENTS.md");
        let profile = AgentProfileConfig {
            system_prompt_append: Some("Focus on security audits.".into()),
            ..Default::default()
        };

        let prompt = build_system_prompt_with_agent(
            &config,
            temp.path(),
            &PluginRegistry::new(),
            None,
            Some(&profile),
        );

        // Built-in prompt is preserved
        assert!(prompt.contains("workflow manager for coding work"));
        // AGENTS.md is preserved
        assert!(prompt.contains("AGENTS.md Instructions:\nagent rule"));
        // Append goes at the very end
        assert!(prompt.ends_with("Focus on security audits."));
    }

    #[test]
    fn agent_profile_preserves_other_layers() {
        let config = NcaConfig::default();
        let temp = tempdir().expect("tempdir");
        fs::write(temp.path().join("AGENTS.md"), "repo rules").expect("write AGENTS.md");
        let profile = AgentProfileConfig {
            system_prompt: Some("You are a specialist.".into()),
            system_prompt_append: Some("Extra rules.".into()),
            ..Default::default()
        };

        let prompt = build_system_prompt_with_agent(
            &config,
            temp.path(),
            &PluginRegistry::new(),
            None,
            Some(&profile),
        );

        // Specialist system_prompt replaces built-in
        assert!(!prompt.contains("workflow manager for coding work"));
        assert!(prompt.contains("You are a specialist."));
        // But AGENTS.md instructions are still present
        assert!(prompt.contains("AGENTS.md Instructions:\nrepo rules"));
        // And append is present
        assert!(prompt.contains("Extra rules."));
    }

    #[test]
    fn no_agent_profile_produces_same_result_as_wrapper() {
        let config = NcaConfig::default();
        let temp = tempdir().expect("tempdir");

        let via_wrapper = build_system_prompt(&config, temp.path(), &PluginRegistry::new(), None);
        let via_with_agent = build_system_prompt_with_agent(
            &config,
            temp.path(),
            &PluginRegistry::new(),
            None,
            None,
        );

        assert_eq!(via_wrapper, via_with_agent);
    }
}
