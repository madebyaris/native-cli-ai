use crate::file_mentions::expand_at_file_mentions_default;
use crate::prompt::NcaPrompt;
use crate::runner::{SessionRuntime, dispatch_question_answer, dispatch_tool_approval};
use crate::slash_commands::SLASH_COMMANDS;

use crate::tui::app::ApprovalAnswer;
use crate::tui::elm::feedback::{TuiFeedback, TuiFeedbackChannel, TuiFeedbackMsg};
use crate::tui::elm::msg::Msg;
use crate::tui::elm::run::run_nca_model;
use crate::tui::{
    ModelPickerAction, ModelPickerEntry, TuiCmd, git_create_branch, git_current_branch,
    git_list_branches, git_switch_branch, replay_events_to_feedback, spawn_tui_bridge,
};
use nca_common::config::{PermissionMode, ProviderKind};
use nca_common::event::{BusyState, EndReason, QuestionSelection};
use nca_core::skills::SkillCatalog;
use nca_runtime::memory_store::MemoryStore;
use reedline::{
    Emacs, FileBackedHistory, Hinter, KeyCode, KeyModifiers, Reedline, ReedlineEvent, Signal, Vi,
    default_emacs_keybindings,
};
use std::io::Write;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;

/// Where slash-command and preset output goes (TTY transcript vs full-screen TUI).
pub(crate) enum ReplOutput<'a> {
    Stdio,
    Tui(&'a dyn TuiFeedback),
}

impl ReplOutput<'_> {
    fn print(&self, s: &str) {
        match self {
            ReplOutput::Stdio => {
                print!("{s}");
                let _ = std::io::stdout().flush();
            }
            ReplOutput::Tui(fb) => {
                for line in s.split('\n') {
                    fb.push_system(line.to_string());
                }
            }
        }
    }

    fn println(&self, s: &str) {
        self.print(&format!("{s}\n"));
    }

    fn eprintln(&self, s: &str) {
        match self {
            ReplOutput::Stdio => eprintln!("{s}"),
            ReplOutput::Tui(fb) => {
                fb.push_error(format!("[!] {s}"));
            }
        }
    }

    fn clear_screen(&self) {
        match self {
            ReplOutput::Stdio => {
                print!("\x1B[2J\x1B[H");
                std::io::stdout().flush().ok();
            }
            ReplOutput::Tui(fb) => {
                fb.clear_transcript();
            }
        }
    }
}

/// Special input prefixes
#[allow(dead_code)]
const INPUT_PREFIXES: &[&str] = &[
    "!",  // Bash mode - run shell command directly
    "@",  // File reference - fuzzy file search
    "\\", // Multiline continuation
];

/// Session state for REPL
pub struct Repl {
    runtime: SessionRuntime,
    prompt: NcaPrompt,
    run_mode: bool,
    history_path: std::path::PathBuf,
    /// Index into the agent entry list (0 = "orchestrator" default, 1+ = OMO specialists).
    agent_index: usize,
    current_agent_label: String,
}

impl Repl {
    pub fn new(runtime: SessionRuntime, safe_mode: bool, run_mode: bool) -> Self {
        let history_path = runtime.workspace_root().join(".nca/.history");
        let current_agent_label = "@orchestrator".to_string();
        Self {
            runtime,
            prompt: NcaPrompt::new(safe_mode, run_mode),
            run_mode,
            history_path,
            agent_index: 0,
            current_agent_label,
        }
    }

    /// Build the agent entry list: "orchestrator" (default) at index 0, then all
    /// registered agent profiles (OMO specialists + user-configured agents).
    fn agent_entries(&self) -> Vec<String> {
        let mut entries = vec!["orchestrator".to_string()];
        entries.extend(self.runtime.agent_profile_names());
        entries
    }

    /// Extract the agent name (without leading @) from the current label.
    #[allow(dead_code)]
    fn current_agent_name(&self) -> &str {
        self.current_agent_label.trim_start_matches('@')
    }

    /// Build (label, description) pairs for the agent picker popup.
    /// Index 0 is always "orchestrator" (the default harness agent).
    fn build_agent_picker_labels(&self) -> Vec<(String, String)> {
        let entries = self.agent_entries();
        entries
            .iter()
            .map(|name| {
                let desc = if name == "orchestrator" {
                    "Workflow manager — plan, delegate, verify".to_string()
                } else {
                    self.runtime
                        .agent_profile_description(name)
                        .unwrap_or_default()
                };
                (format!("@{name}"), desc)
            })
            .collect()
    }

    /// Run the interactive REPL until the user exits.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let mut editor = self.build_editor()?;

        let _spawn_task = {
            let spawn_rx = self.runtime.take_spawn_rx();
            let event_tx = self.runtime.event_tx();
            if let Some(srx) = spawn_rx {
                Some(nca_runtime::supervisor::spawn_subagent_consumer(
                    srx,
                    self.runtime.session_id().to_string(),
                    self.runtime.workspace_root().to_path_buf(),
                    self.runtime.config().clone(),
                    self.runtime.messages().to_vec(),
                    event_tx,
                ))
            } else {
                None
            }
        };

        if self.run_mode {
            self.print_banner();
        }

        loop {
            // Update prompt with current agent profile
            self.prompt.set_agent(&self.current_agent_label);
            let sig = editor.read_line(&self.prompt);
            match sig {
                Ok(Signal::Success(input)) => {
                    if input.is_empty() {
                        continue;
                    }

                    // Tab switches agent profile (also used by hinter for slash completion)
                    if input == "\t" {
                        self.switch_agent();
                        continue;
                    }

                    // Bash mode: ! prefix runs shell command directly
                    if input.starts_with('!') {
                        let cmd = input.trim_start_matches('!');
                        self.run_bash_command(cmd).await;
                        continue;
                    }

                    // Slash commands
                    if input.starts_with('/') {
                        if !self.handle_command(&input, ReplOutput::Stdio).await? {
                            break;
                        }
                        continue;
                    }

                    let expanded = match expand_at_file_mentions_default(
                        &input,
                        self.runtime.workspace_root(),
                    ) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("file mention expansion: {e}");
                            continue;
                        }
                    };
                    match self.runtime.run_turn(&expanded).await {
                        Ok(output) => {
                            println!("{output}");
                        }
                        Err(err) => {
                            eprintln!("error: {err}");
                        }
                    }
                }
                Ok(Signal::CtrlD) => {
                    // Ctrl+D - exit
                    eprintln!("\n[exit]");
                    break;
                }
                Ok(Signal::CtrlC) => {
                    // Ctrl+C - cancel current or exit
                    eprintln!(
                        "\n[cancel] Press Ctrl+D to exit, or wait for current operation to complete"
                    );
                }
                Err(err) => {
                    eprintln!("read error: {err}");
                    break;
                }
            }
        }

        self.runtime.finish(EndReason::UserExit).await;
        Ok(())
    }

    fn print_banner(&self) {
        eprintln!(
            r#"
╔══════════════════════════════════════════════════════════════╗
║  nca - Native CLI AI                                          ║
║  Interactive terminal mode                                     ║
╠══════════════════════════════════════════════════════════════╣
║  Shortcuts:                                                   ║
║    ! <cmd>   Run shell command (bash mode)                    ║
║    @path     Inline file mentions (expanded before send)      ║
║    / <cmd>   Slash commands                                  ║
║    Tab       Complete /commands or switch agent                  ║
║    Ctrl+D    Exit                                            ║
║    Ctrl+C    Cancel current request                           ║
║    Ctrl+L    Clear screen                                     ║
║    Ctrl+R    Search command history                           ║
╚══════════════════════════════════════════════════════════════╝
"#
        );
    }

    /// Switch to the next agent profile (called on Tab press in stdio mode).
    fn switch_agent(&mut self) {
        let entries = self.agent_entries();
        if entries.len() <= 1 {
            return;
        }
        self.agent_index = (self.agent_index + 1) % entries.len();
        let name = entries[self.agent_index].clone();
        self.current_agent_label = format!("@{name}");
        self.prompt.set_agent(&self.current_agent_label);

        // Apply the agent profile at runtime (None = default harness prompt).
        let profile_name = if self.agent_index == 0 {
            None
        } else {
            Some(name.as_str())
        };
        if let Err(e) = self.runtime.apply_agent_profile(profile_name) {
            eprintln!("\n[agent] failed to switch: {e}");
        }

        eprintln!("\n[agent] Switched to @{name}");
    }

    /// Run a shell command directly (bash mode) - Claude Code style
    /// Output is returned to the conversation context
    async fn run_bash_command(&self, cmd: &str) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            eprintln!("! usage: !<command> [args]");
            return;
        }

        eprintln!("[bash] {cmd}");

        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);

                if !stdout.is_empty() {
                    println!("{stdout}");
                }
                if !stderr.is_empty() {
                    eprintln!("[stderr] {stderr}");
                }
                if out.status.success() {
                    eprintln!("[bash] completed (exit 0)");
                } else {
                    eprintln!("[bash] failed (exit {})", out.status.code().unwrap_or(-1));
                }
            }
            Err(e) => {
                eprintln!("[bash] failed to execute: {e}");
            }
        }
    }

    /// Open the configured external editor (`NCA_EDITOR`, `[ui].editor`, `EDITOR`, `vim`).
    async fn open_external_editor(&self, seed: Option<&str>) -> Option<String> {
        let editor_cmd = self.runtime.config().effective_editor_command();
        let temp_file = format!("nca-prompt-{}.txt", std::process::id());
        let temp_path = std::env::temp_dir().join(&temp_file);
        std::fs::write(&temp_path, seed.unwrap_or("")).ok()?;

        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("{} '{}'", editor_cmd, temp_path.display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(_) => {
                let content = std::fs::read_to_string(&temp_path).ok()?;
                let _ = std::fs::remove_file(&temp_path);
                let content = content.trim().to_string();
                if content.is_empty() {
                    None
                } else {
                    Some(content)
                }
            }
            Err(e) => {
                eprintln!("[editor] Failed to open: {e}");
                None
            }
        }
    }

    async fn apply_provider_in_session(
        &mut self,
        p: ProviderKind,
        out: ReplOutput<'_>,
    ) -> anyhow::Result<()> {
        let mut cfg = self.runtime.config().clone();
        cfg.set_default_provider(p);
        match self.runtime.apply_nca_config(cfg) {
            Ok(()) => {
                if let ReplOutput::Tui(st) = &out {
                    st.set_model(self.runtime.model().to_string());
                }
                match self
                    .runtime
                    .config()
                    .save_workspace_file(self.runtime.workspace_root())
                {
                    Ok(()) => out.println(&format!(
                        "[provider] {} — model {} — saved .nca/config.local.toml",
                        p.display_name(),
                        self.runtime.model()
                    )),
                    Err(e) => out.eprintln(&format!(
                        "[provider] applied but workspace save failed: {e}"
                    )),
                }
            }
            Err(e) => out.eprintln(&format!("[provider] {e}")),
        }
        Ok(())
    }

    async fn save_provider_api_key(
        &mut self,
        p: ProviderKind,
        key: &str,
        out: ReplOutput<'_>,
    ) -> anyhow::Result<()> {
        let mut cfg = self.runtime.config().clone();
        cfg.set_provider_api_key(p, key);
        match self.runtime.apply_nca_config(cfg) {
            Ok(()) => {
                if let Err(e) = self
                    .runtime
                    .config()
                    .save_workspace_file(self.runtime.workspace_root())
                {
                    out.eprintln(&format!("[apikey] applied but workspace save failed: {e}"));
                } else {
                    out.println(&format!("[apikey] saved for {}", p.display_name()));
                }
            }
            Err(e) => out.eprintln(&format!("[apikey] {e}")),
        }
        Ok(())
    }

    fn build_editor(&self) -> anyhow::Result<Reedline> {
        let mut builder = Reedline::create()
            .with_ansi_colors(true)
            .with_hinter(Box::new(SlashHinter {
                skill_directories: self.runtime.config().harness.skill_directories.clone(),
                workspace_root: self.runtime.workspace_root().to_path_buf(),
                plugin_commands: self
                    .runtime
                    .plugin_commands()
                    .into_iter()
                    .flat_map(|(_, cmds)| cmds)
                    .collect(),
                hint_suffix: String::new(),
            }));

        // Try to load history from disk
        if let Some(parent) = self.history_path.parent() {
            std::fs::create_dir_all(parent).ok();
            if let Ok(history) = FileBackedHistory::with_file(100, self.history_path.clone()) {
                builder = builder.with_history(Box::new(history));
            }
        }

        // Tab: accept slash-command hinter hint, otherwise immediately exit for agent switch.
        // crossterm reports Tab as KeyCode::Char('\t').
        let mut keybindings = default_emacs_keybindings();
        keybindings.add_binding(
            KeyModifiers::NONE,
            KeyCode::Char('\t'),
            ReedlineEvent::UntilFound(vec![
                ReedlineEvent::HistoryHintComplete,
                ReedlineEvent::ExecuteHostCommand(String::from("\t")),
            ]),
        );

        // Support vim mode if enabled via env
        if std::env::var("NCA_EDITOR_MODE")
            .map(|v| v.eq_ignore_ascii_case("vi") || v.eq_ignore_ascii_case("vim"))
            .unwrap_or(false)
        {
            builder = builder.with_edit_mode(Box::new(Vi::default()));
        } else {
            builder = builder.with_edit_mode(Box::new(Emacs::new(keybindings)));
        }

        Ok(builder)
    }

    async fn handle_command(&mut self, input: &str, out: ReplOutput<'_>) -> anyhow::Result<bool> {
        let mut parts = input.split_whitespace();
        let command = parts.next().unwrap_or_default();
        let rest = input
            .strip_prefix(command)
            .map(str::trim)
            .unwrap_or_default();

        match command {
            "/q" | "/quit" | "/exit" => return Ok(false),
            "/stop" => {
                self.runtime.request_cancel();
                out.println("[stop] cancelling current turn…");
            }
            "/help" => {
                let help_lines = vec![
                    "nca Interactive Mode".into(),
                    String::new(),
                    "INPUT MODES:".into(),
                    "  ! <cmd>     Run shell command (output feeds into context)".into(),
                    "  @path       Inline file mentions".into(),
                    "  / <cmd>     Slash commands".into(),
                    "  \\           Multiline input (end line with \\ to continue)".into(),
                    String::new(),
                    "SLASH COMMANDS:".into(),
                    "  /help              Show this help".into(),
                    "  /status            Session status".into(),
                    "  /agent [name]     Show or switch agent (OMO specialists)".into(),
                    "  /plan <task>       Planning-oriented turn".into(),
                    "  /review <task>     Code review turn".into(),
                    "  /fix <task>        Bug-fix turn".into(),
                    "  /test <task>       Validation turn".into(),
                    "  /clear             Clear the screen".into(),
                    "  /compact           Compact session summary".into(),
                    "  /new               Start a new session".into(),
                    "  /export            Export session to markdown".into(),
                    "  /thinking          Toggle thinking/reasoning visibility".into(),
                    "  /tool-output      Toggle tool output expand/collapse (TUI)".into(),
                    "  /skills            List discovered skills".into(),
                    "  /memory [text]     Show or store memory notes".into(),
                    "  /models            Browse and select models".into(),
                    "  /connect           Connect LLM provider".into(),
                    "  /provider [name]   Default provider".into(),
                    "  /apikey <p> <key>  Store provider API key".into(),
                    "  /model [name]      Set active model".into(),
                    "  /editor [seed]     Open external editor".into(),
                    "  /set-editor <cmd>  Persist editor command".into(),
                    "  /mcp               List MCP servers".into(),
                    "  /sessions          List/switch sessions".into(),
                    "  /permissions [m]   Show or set permission mode".into(),
                    "  /config            Show runtime config".into(),
                    "  /doctor            Run config checks".into(),
                    "  /diff              Show recent file changes".into(),
                    "  /cost              Show token usage".into(),
                    "  /stats             Session statistics".into(),
                    "  /exit              Exit repl".into(),
                    String::new(),
                    "KEYBOARD SHORTCUTS:".into(),
                    "  Tab          Cycle agent".into(),
                    "  Ctrl+P       Command palette".into(),
                    "  Ctrl+X M     Switch model".into(),
                    "  Ctrl+X E     Open editor".into(),
                    "  Ctrl+X L     Switch session".into(),
                    "  Ctrl+X N     New session".into(),
                    "  Ctrl+X C     Compact".into(),
                    "  Ctrl+X S     View status".into(),
                    "  Ctrl+X A     Agent picker".into(),
                    "  Ctrl+X H     Help".into(),
                    "  Ctrl+X Q     Exit".into(),
                    "  Ctrl+C       Cancel request".into(),
                    "  Ctrl+L       Clear screen".into(),
                    "  Ctrl+V       Paste image (TUI)".into(),
                    "  F2           Cycle recent models".into(),
                ];
                if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("help".to_string(), help_lines);
                } else {
                    for l in &help_lines {
                        out.println(l);
                    }
                }
            }
            "/status" => {
                let snapshot = self.runtime.snapshot();
                let mut lines = vec![
                    format!("Session:     {}", snapshot.id),
                    format!("Model:       {}", self.runtime.model()),
                    format!("Agent:       {}", self.current_agent_label),
                    format!("Permission:  {:?}", self.runtime.permission_mode()),
                    format!("Children:    {}", snapshot.child_session_ids.len()),
                    format!("Memory:      {}", self.runtime.memory_store_path().display()),
                ];
                if let Some(summary) = &snapshot.session_summary {
                    lines.push(String::new());
                    lines.push(format!("Summary: {}", summary.replace('\n', " ")));
                }
                if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("status".to_string(), lines);
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/agent" => {
                if let Some(target) = parts.next() {
                    let target_clean = target.trim_start_matches('@').to_lowercase();
                    let entries = self.agent_entries();
                    if let Some(idx) = entries.iter().position(|e| *e == target_clean) {
                        self.agent_index = idx;
                        let name = entries[idx].clone();
                        self.current_agent_label = format!("@{name}");
                        self.prompt.set_agent(&self.current_agent_label);
                        let profile_name = if idx == 0 { None } else { Some(name.as_str()) };
                        if let Err(e) = self.runtime.apply_agent_profile(profile_name) {
                            out.eprintln(&format!("Failed to switch agent: {e}"));
                        }
                        if let ReplOutput::Tui(st) = &out {
                            st.set_agent_profile(self.current_agent_label.clone());
                            st.set_permission_mode(format!(
                                "{:?}",
                                self.runtime.permission_mode()
                            ));
                            st.set_model(self.runtime.model().to_string());
                        }
                        out.println(&format!("Switched to @{name}"));
                    } else {
                        out.println(&format!("Unknown agent: {target}"));
                        out.println(&format!(
                            "Available: {}",
                            entries.join(", ")
                        ));
                    }
                } else if let ReplOutput::Tui(st) = &out {
                    let labels = self.build_agent_picker_labels();
                    st.open_agent_picker(labels, self.agent_index);
                } else {
                    let entries = self.agent_entries();
                    out.println(&format!("Current agent: {}", self.current_agent_label));
                    out.println("Available agents:");
                    for (i, name) in entries.iter().enumerate() {
                        let marker = if i == self.agent_index { " *" } else { "" };
                        out.println(&format!("  @{name}{marker}"));
                    }
                }
            }
            "/plan" => {
                self.run_preset(
                    "Create a short implementation plan before coding. Focus on steps, risks, and validation.\n\nTask:\n",
                    rest,
                    out,
                )
                .await?
            }
            "/review" => {
                self.run_preset(
                    "Review the requested code or changes. Prioritize bugs, regressions, risks, and missing tests.\n\nReview target:\n",
                    rest,
                    out,
                )
                .await?
            }
            "/fix" => {
                self.run_preset(
                    "Diagnose and fix the issue below. Prefer a minimal verified change.\n\nIssue:\n",
                    rest,
                    out,
                )
                .await?
            }
            "/test" => {
                self.run_preset(
                    "Validate the requested area. Run tests or checks if tools allow, and report what passed or failed.\n\nTarget:\n",
                    rest,
                    out,
                )
                .await?
            }
            "/model" => {
                if let Some(model) = parts.next() {
                    let mut cfg = self.runtime.config().clone();
                    cfg.apply_model_override(model);
                    cfg.model.track_recent_model(&self.runtime.config().model.resolve_alias(model));
                    match self.runtime.apply_nca_config(cfg) {
                        Ok(()) => {
                            if let Err(e) = self
                                .runtime
                                .config()
                                .save_workspace_file(self.runtime.workspace_root())
                            {
                                out.eprintln(&format!(
                                    "[model] session updated; workspace save failed: {e}"
                                ));
                            } else {
                                out.println(&format!(
                                    "model set to {} (saved .nca/config.local.toml)",
                                    self.runtime.model()
                                ));
                            }
                            if let ReplOutput::Tui(st) = out {
                                st.set_model(self.runtime.model().to_string());
                            }
                        }
                        Err(e) => out.eprintln(&format!("[model] {e}")),
                    }
                } else if let ReplOutput::Tui(st) = &out {
                    let provider_models = nca_runtime::model_limits_api::fetch_provider_model_ids(self.runtime.config()).await;
                    let entries = build_model_picker_entries(self.runtime.config(), &provider_models);
                    st.open_model_picker(entries);
                } else {
                    out.println(&format!("active model: {}", self.runtime.model()));
                    for p in ProviderKind::ALL {
                        out.println(&format!(
                            "  {} → {}",
                            p.display_name(),
                            self.runtime.config().provider.model_for(p)
                        ));
                    }
                    out.println("usage: /model <name>");
                }
            }
            "/clear" => {
                out.clear_screen();
                out.println("[screen cleared]");
            }
            "/undo" => {
                out.eprintln("[undo] Not yet implemented - use /compact to save session state");
            }
            "/redo" => {
                out.eprintln("[redo] Not yet implemented");
            }
            "/diff" => {
                // Show recent file changes via git
                let output = Command::new("sh")
                    .arg("-c")
                    .arg("git diff --stat HEAD~5..HEAD 2>/dev/null || git diff --stat 2>/dev/null || echo 'No git changes'")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
                    .await;
                match output {
                    Ok(cmd_out) => {
                        let diff = String::from_utf8_lossy(&cmd_out.stdout);
                        if diff.is_empty() {
                            out.println("[diff] No recent changes");
                        } else {
                            out.print(&diff);
                        }
                    }
                    Err(e) => out.eprintln(&format!("[diff] Failed: {e}")),
                }
            }
            "/cost" => {
                let snapshot = self.runtime.snapshot();
                let cs = self.runtime.context_stats();
                out.eprintln(&format!("[cost] Session: {}", snapshot.id));
                out.eprintln(&format!(
                    "[cost] Context: {}% ({} / {} tokens)",
                    cs.usage_percent, cs.estimated_tokens, cs.context_window
                ));
                out.eprintln(&format!(
                    "[cost] Tokens: {} in + {} out",
                    snapshot.total_input_tokens, snapshot.total_output_tokens
                ));
                out.eprintln(&format!(
                    "[cost] Cost: ${:.4}", snapshot.estimated_cost_usd
                ));
            }
            "/stats" => {
                let snapshot = self.runtime.snapshot();
                let cs = self.runtime.context_stats();
                let lines = vec![
                    format!("Session:     {}", snapshot.id),
                    format!("Model:       {}", self.runtime.model()),
                    format!("Agent:       {}", self.current_agent_label),
                    format!("Permission:  {:?}", self.runtime.permission_mode()),
                    format!("Context:     {}% ({} / {} tokens)", cs.usage_percent, cs.estimated_tokens, cs.context_window),
                    format!("Tokens:      {} in + {} out", snapshot.total_input_tokens, snapshot.total_output_tokens),
                    format!("Cost:        ${:.4}", snapshot.estimated_cost_usd),
                    format!("Children:    {}", snapshot.child_session_ids.len()),
                    format!("Memory:      {}", self.runtime.memory_store_path().display()),
                ];
                if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("stats".to_string(), lines);
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/permissions" => {
                if let Some(sub) = parts.next() {
                    if sub.eq_ignore_ascii_case("bypass") {
                        // bypass shortcut: /permissions bypass [on|off|toggle]
                        let arg = parts.next().unwrap_or("").trim();
                        let target = match arg.to_ascii_lowercase().as_str() {
                            "" | "toggle" => {
                                if self.runtime.permission_mode() == PermissionMode::BypassPermissions {
                                    PermissionMode::Default
                                } else {
                                    PermissionMode::BypassPermissions
                                }
                            }
                            "on" | "enable" | "yes" | "1" => PermissionMode::BypassPermissions,
                            "off" | "disable" | "no" | "0" => PermissionMode::Default,
                            _ => {
                                out.println(
                                    "usage: /permissions bypass [on|off|toggle] — default toggles bypass ↔ default",
                                );
                                return Ok(true);
                            }
                        };
                        self.runtime.set_permission_mode(target);
                        if let ReplOutput::Tui(st) = out {
                            st.set_permission_mode(format!("{target:?}"));
                        }
                        out.println(&format!("permission mode set to {target:?}"));
                    } else if let Ok(parsed_mode) = sub.parse::<PermissionMode>() {
                        self.runtime.set_permission_mode(parsed_mode);
                        if let ReplOutput::Tui(st) = out {
                            st.set_permission_mode(format!("{parsed_mode:?}"));
                        }
                        out.println(&format!("permission mode set to {parsed_mode:?}"));
                    } else {
                        out.println(
                            "invalid mode; expected one of: default, plan, accept-edits, dont-ask, bypass-permissions",
                        );
                    }
                } else if let ReplOutput::Tui(st) = &out {
                    let current_idx = self.runtime.permission_mode().index();
                    st.open_permission_picker(current_idx);
                } else {
                    out.println(&format!(
                        "permission_mode: {:?}",
                        self.runtime.permission_mode()
                    ));
                    out.println("(shortcut: /permissions bypass toggles bypass ↔ default)");
                }
            }
            "/skills" => {
                let skills = SkillCatalog::discover(
                    self.runtime.workspace_root(),
                    &self.runtime.config().harness.skill_directories,
                )
                .map_err(anyhow::Error::msg)?;
                if skills.is_empty() {
                    let lines = vec!["No skills discovered.".into()];
                    if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("skills".to_string(), lines);
                } else {
                        out.println("no skills discovered");
                    }
                } else {
                    let lines: Vec<String> = skills.iter().map(|s| s.summary_line()).collect();
                    if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("skills".to_string(), lines);
                } else {
                        for l in &lines {
                            out.println(l);
                        }
                    }
                }
            }
            "/memory" => {
                if rest.is_empty() {
                    let store = MemoryStore::new(self.runtime.memory_store_path());
                    let mem = store.load().await.map_err(anyhow::Error::msg)?;
                    if mem.notes.is_empty() {
                        let lines = vec!["No memory notes stored.".into()];
                        if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("memory".to_string(), lines);
                } else {
                            out.println("no memory notes stored");
                        }
                    } else {
                        let lines: Vec<String> = mem
                            .notes
                            .iter()
                            .rev()
                            .take(20)
                            .map(|note| {
                                format!("{} {} {}", note.id, note.kind, note.content.replace('\n', " "))
                            })
                            .collect();
                        if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("memory".to_string(), lines);
                } else {
                            for l in lines.iter().take(5) {
                                out.println(l);
                            }
                        }
                    }
                } else {
                    self.runtime
                        .append_memory_note("note", Some(rest.to_string()))
                        .await
                        .map_err(anyhow::Error::msg)?;
                    out.println("memory note saved");
                }
            }
            "/compact" => {
                let summary = self.runtime.compact_summary();
                self.runtime.set_session_summary(Some(summary.clone()));
                self.runtime
                    .append_memory_note("session-summary", Some(summary.clone()))
                    .await
                    .map_err(anyhow::Error::msg)?;
                self.runtime.save().await.map_err(anyhow::Error::msg)?;
                out.println(&format!("saved session summary:\n{}", summary));
            }
            "/models" => {
                if let ReplOutput::Tui(st) = &out {
                    let provider_models = nca_runtime::model_limits_api::fetch_provider_model_ids(self.runtime.config()).await;
                    let entries = build_model_picker_entries(self.runtime.config(), &provider_models);
                    st.open_model_picker(entries);
                } else {
                    let provider = self.runtime.config().provider.default;
                    out.println(&format!(
                        "default_provider={} default_model={} thinking={} budget={}",
                        provider.display_name(),
                        self.runtime.config().model.default_model,
                        self.runtime.config().model.enable_thinking,
                        self.runtime.config().model.thinking_budget
                    ));
                    for provider in nca_common::config::ProviderKind::ALL {
                        out.println(&format!(
                            "  {} -> {} ({})",
                            provider.display_name(),
                            self.runtime.config().provider.model_for(provider),
                            self.runtime.config().provider.base_url_for(provider)
                        ));
                    }
                    for (alias, target) in &self.runtime.config().model.aliases {
                        out.println(&format!("  {alias} -> {target}"));
                    }
                }
            }
            "/mcp" => {
                let lines: Vec<String> = if self.runtime.config().mcp.servers.is_empty() {
                    vec!["No MCP servers configured.".into()]
                } else {
                    self.runtime
                        .config()
                        .mcp
                        .servers
                        .iter()
                        .filter(|server| server.enabled)
                        .map(|server| {
                            format!(
                                "{} command={} {}",
                                server.name,
                                server.command,
                                server.args.join(" ")
                            )
                        })
                        .collect()
                };
                if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("mcp".to_string(), lines);
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/agents" => {
                let snapshot = self.runtime.snapshot();
                let lines: Vec<String> = if snapshot.child_session_ids.is_empty() {
                    vec!["No child sessions yet.".into()]
                } else {
                    snapshot.child_session_ids.clone()
                };
                if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("agents".to_string(), lines);
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/logs" => {
                match tokio::fs::read_to_string(self.runtime.event_log_path()).await {
                    Ok(data) => {
                        if let ReplOutput::Tui(st) = &out {
                            let lines: Vec<String> = data.lines().rev().take(100).map(String::from).collect();
                            let lines: Vec<String> = lines.into_iter().rev().collect();
                            st.open_info_modal("logs (last 100)".to_string(), lines);
                        } else {
                            out.print(&data);
                        }
                    }
                    Err(err) => {
                        out.eprintln(&format!("failed to read log: {err}"))
                    }
                }
            }
            "/attach" => {
                let snapshot = self.runtime.snapshot();
                let lines = vec![
                    format!("Session:  {}", snapshot.id),
                    format!(
                        "Socket:   {}",
                        snapshot
                            .socket_path
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "<none>".into())
                    ),
                ];
                if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("attach".to_string(), lines);
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/mount" => {
                let rest_trim = rest.trim();
                if rest_trim.is_empty() {
                    // List mounted paths.
                    let mounted = self.runtime.mounted_paths();
                    let ws = self.runtime.workspace_root().display().to_string();
                    let mut lines = vec![
                        format!("Workspace: {}", ws),
                        format!("Mounted paths: {}", mounted.len()),
                    ];
                    if mounted.is_empty() {
                        lines.push("  (none)".into());
                    } else {
                        for p in &mounted {
                            lines.push(format!("  {}", p.display()));
                        }
                    }
                    lines.push(String::new());
                    lines.push("Usage: /mount <path>   Mount an external directory".into());
                    lines.push("       /unmount <path>  Remove a mount".into());
                    if let ReplOutput::Tui(st) = &out {
                        st.open_info_modal("mount".to_string(), lines);
                    } else {
                        for l in &lines {
                            out.println(l);
                        }
                    }
                } else {
                    let path = std::path::Path::new(rest_trim);
                    match self.runtime.mount_path(path) {
                        Ok(()) => {
                            let mounted = self.runtime.mounted_paths();
                            out.println(&format!(
                                "[mount] {} ({} mounted)",
                                path.display(),
                                mounted.len()
                            ));
                        }
                        Err(e) => {
                            out.eprintln(&format!("[mount] {e}"));
                        }
                    }
                }
            }
            "/unmount" => {
                let rest_trim = rest.trim();
                if rest_trim.is_empty() {
                    out.println("Usage: /unmount <path>");
                } else {
                    let path = std::path::Path::new(rest_trim);
                    match self.runtime.unmount_path(path) {
                        Ok(()) => {
                            out.println(&format!("[unmount] {}", path.display()));
                        }
                        Err(e) => {
                            out.eprintln(&format!("[unmount] {e}"));
                        }
                    }
                }
            }
            "/image" => {
                let st = match &out {
                    ReplOutput::Tui(st) => st,
                    ReplOutput::Stdio => {
                        out.eprintln(
                            "[image] stage images from the full-screen TUI (Ctrl+V, /image paste, /image <path>)",
                        );
                        return Ok(true);
                    }
                };
                let workspace = self.runtime.workspace_root().to_path_buf();
                let sid = self.runtime.session_id().to_string();
                let rest_trim = rest.trim();
                if rest_trim.is_empty() || rest_trim.eq_ignore_ascii_case("paste") {
                    match crate::image_attach::paste_clipboard_image(&workspace, &sid) {
                        Ok(att) => {
                            let path = att.path.clone();
                            st.push_staged_image(att);
                            let n = 0;  // count tracked by NcaModel
                            out.println(&format!(
                                "[image] staged {path} — press Enter to send ({n} attached)"
                            ));
                        }
                        Err(e) => out.eprintln(&format!("[image] {e}")),
                    }
                } else if rest_trim.eq_ignore_ascii_case("clear") {
                    st.clear_staged_images();
                    out.println("[image] cleared staged images");
                } else {
                    let p = std::path::Path::new(rest_trim);
                    match crate::image_attach::import_image_file(&workspace, &sid, p) {
                        Ok(att) => {
                            let path = att.path.clone();
                            st.push_staged_image(att);
                            let n = 0;  // count tracked by NcaModel
                            out.println(&format!(
                                "[image] staged {path} — press Enter to send ({n} attached)"
                            ));
                        }
                        Err(e) => out.eprintln(&format!("[image] {e}")),
                    }
                }
            }
            "/config" => {
                let config = self.runtime.config();
                let lines = vec![
                    format!("Provider:    {}", config.provider.default.display_name()),
                    format!("Model:       {}", self.runtime.model()),
                    format!("Permission:  {:?}", self.runtime.permission_mode()),
                    format!("Memory:      {}", self.runtime.memory_store_path().display()),
                    format!("Editor:      {}", config.effective_editor_command()),
                    format!("Thinking:    {} (budget: {})", config.model.enable_thinking, config.model.thinking_budget),
                    format!("Max tokens:  {}", config.model.max_tokens),
                    String::new(),
                    "Provider endpoints:".into(),
                    format!("  MiniMax:     {}", config.provider.base_url_for(ProviderKind::MiniMax)),
                    format!("  OpenAI:      {}", config.provider.base_url_for(ProviderKind::OpenAi)),
                    format!("  Anthropic:   {}", config.provider.base_url_for(ProviderKind::Anthropic)),
                    format!("  OpenRouter:  {}", config.provider.base_url_for(ProviderKind::OpenRouter)),
                    format!("  ZhipuAI:     {}", config.provider.base_url_for(ProviderKind::ZhipuAI)),
                    format!("  DeepSeek:    {}", config.provider.base_url_for(ProviderKind::DeepSeek)),
                ];
                if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("config".to_string(), lines);
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/connect" => {
                if let ReplOutput::Tui(st) = out {
                    st.open_connect_modal();
                    out.println(
                        "[connect] Choose a provider (↑↓ · Enter · type to search · Esc). Add key with /apikey if needed.",
                    );
                } else {
                    out.println("Connect an LLM provider (non-TUI):");
                    out.println("  /provider <minimax|openai|anthropic|openrouter|zhipuai|deepseek>");
                    out.println("  /apikey <provider> <secret>   — save API key to .nca/config.local.toml");
                    out.println("  /model <name>                 — set model after switching provider");
                    out.println(&format!(
                        "  current: {} → {}",
                        self.runtime.config().provider.default.display_name(),
                        self.runtime.model()
                    ));
                }
            }
            "/settings" => {
                let lines = vec![
                    "Workspace settings (.nca/config.local.toml):".into(),
                    String::new(),
                    format!("  Provider:    {}", self.runtime.config().provider.default.display_name()),
                    format!("  Model:       {}", self.runtime.model()),
                    format!("  Editor:      {}", self.runtime.config().effective_editor_command()),
                    format!("  Permission:  {:?}", self.runtime.permission_mode()),
                    String::new(),
                    "Commands:".into(),
                    "  /connect           OpenCode-style provider picker".into(),
                    "  /models            Browse and select models".into(),
                    "  /provider [name]   Default LLM provider".into(),
                    "  /apikey <p> <key>  Store API key for a provider".into(),
                    "  /model [name]      Model for the active provider".into(),
                    "  /editor [seed]     Open external editor".into(),
                    "  /set-editor <cmd>  Persist editor command".into(),
                ];
                if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("settings".to_string(), lines);
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/provider" => {
                let rest = rest.trim();
                if rest.is_empty() {
                    if let ReplOutput::Tui(st) = out {
                        st.open_provider_picker(false);
                        out.println("[provider] choose with ↑↓ + Enter, Esc to cancel");
                    } else {
                        out.println(&format!(
                            "current default provider: {} (model {})",
                            self.runtime.config().provider.default.display_name(),
                            self.runtime.model()
                        ));
                        out.println("usage: /provider <minimax|openai|anthropic|openrouter|zhipuai|deepseek>");
                    }
                } else if let Some(p) = ProviderKind::from_cli_name(rest)
                    .or_else(|| ProviderKind::parse_display_name(rest))
                {
                    self.apply_provider_in_session(p, out).await?;
                } else {
                    out.eprintln("unknown provider; try: minimax, openai, anthropic, openrouter, zhipuai, deepseek");
                }
            }
            "/apikey" => {
                let mut toks = rest.split_whitespace();
                let p_name = toks.next();
                let key = toks.collect::<Vec<_>>().join(" ");
                let key = key.trim();
                if let Some(pn) = p_name {
                    let p = ProviderKind::from_cli_name(pn)
                        .or_else(|| ProviderKind::parse_display_name(pn));
                    if let Some(p) = p {
                        if key.is_empty() {
                            if let ReplOutput::Tui(st) = out {
                                st.open_api_key_modal(
                                        p,
                                        self.runtime.config().provider.api_key_present_for(p),
                                        false,
                                    );
                            } else {
                                out.println("usage: /apikey <provider> <secret>");
                            }
                        } else {
                            self.save_provider_api_key(p, key, out).await?;
                        }
                    } else {
                        out.eprintln("unknown provider; try: minimax, openai, anthropic, openrouter, zhipuai, deepseek");
                    }
                } else if let ReplOutput::Tui(st) = out {
                    st.open_provider_picker(true);
                    out.println("[apikey] pick provider, then paste key + Enter");
                } else {
                    out.println("usage: /apikey <provider> <secret>");
                }
            }
            "/editor" => {
                let seed = if rest.is_empty() { None } else { Some(rest) };
                match self.open_external_editor(seed).await {
                    Some(text) if !text.is_empty() => {
                        if let ReplOutput::Tui(st) = out {
                            st.set_input_buffer(text.clone(), text.chars().count());
                            out.println("[editor] loaded into composer — press Enter to send");
                        } else {
                            let expanded = match expand_at_file_mentions_default(
                                &text,
                                self.runtime.workspace_root(),
                            ) {
                                Ok(s) => s,
                                Err(e) => {
                                    out.eprintln(&format!("file mention expansion: {e}"));
                                    text
                                }
                            };
                            match self.runtime.run_turn(&expanded).await {
                                Ok(o) => println!("{o}"),
                                Err(e) => eprintln!("error: {e}"),
                            }
                        }
                    }
                    Some(_) => out.println("[editor] empty buffer — nothing sent"),
                    None => {}
                }
            }
            "/set-editor" => {
                let cmd = rest.trim();
                if cmd.is_empty() {
                    out.println(&format!(
                        "usage: /set-editor <command>  (effective: {})",
                        self.runtime.config().effective_editor_command()
                    ));
                } else {
                    self.runtime.config_mut().ui.editor = Some(cmd.to_string());
                    match self
                        .runtime
                        .config()
                        .save_workspace_file(self.runtime.workspace_root())
                    {
                        Ok(()) => out.println(&format!(
                            "[set-editor] saved `{cmd}` to .nca/config.local.toml"
                        )),
                        Err(e) => out.eprintln(&format!("[set-editor] save failed: {e}")),
                    }
                }
            }
            "/doctor" => {
                let mut lines = Vec::new();
                for provider in nca_common::config::ProviderKind::ALL {
                    let configured = self
                        .runtime
                        .config()
                        .provider
                        .api_key_present_for(provider);
                    lines.push(format!(
                        "{}{} API key {} ({})",
                        provider.display_name(),
                        if provider == self.runtime.config().provider.default {
                            " [selected]"
                        } else {
                            ""
                        },
                        if configured { "✓ configured" } else { "✗ missing" },
                        self.runtime.config().provider.api_key_env_for(provider)
                    ));
                }
                if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("doctor".to_string(), lines);
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/auto-answer" => {
                let from_tui = if let ReplOutput::Tui(st) = &out {
                    st.get_active_question_id()
                } else {
                    None
                };
                let ok = if let Some(qid) = from_tui {
                    self.runtime
                        .submit_question_answer(&qid, QuestionSelection::Suggested)
                } else {
                    self.runtime.submit_suggested_answer()
                };
                if ok {
                    out.println("accepted suggested answer for pending question");
                } else {
                    out.eprintln(
                        "no pending interactive question to auto-answer (use when ask_question is waiting)",
                    );
                }
            }
            "/sessions" => match self.runtime.list_session_snapshots().await {
                Ok(mut snapshots) => {
                    snapshots.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
                    let current = self.runtime.session_id().to_string();
                    if snapshots.is_empty() {
                        let lines = vec!["No saved sessions.".into()];
                        if let ReplOutput::Tui(st) = &out {
                    st.open_info_modal("sessions".to_string(), lines);
                } else {
                            out.println("no saved sessions");
                        }
                    } else if let ReplOutput::Tui(st) = &out {
                        st.open_session_picker(snapshots, current.clone());
                    } else {
                        for snap in &snapshots {
                            let marker = if snap.id == current {
                                " *"
                            } else {
                                ""
                            };
                            let display = if let Some(title) = &snap.session_title {
                                format!("{title}{marker}  [{}]", snap.id)
                            } else {
                                format!("{}{marker}", snap.id)
                            };
                            out.println(&display);
                        }
                    }
                }
                Err(error) => {
                    out.eprintln(&format!("failed to list sessions: {error}"));
                }
            },
            "/new" => {
                let summary = self.runtime.compact_summary();
                self.runtime.set_session_summary(Some(summary.clone()));
                self.runtime
                    .append_memory_note("session-summary", Some(summary))
                    .await
                    .map_err(anyhow::Error::msg)?;
                self.runtime.save().await.map_err(anyhow::Error::msg)?;
                self.runtime.new_session().await.map_err(anyhow::Error::msg)?;
                let new_id = self.runtime.session_id().to_string();
                if let ReplOutput::Tui(st) = &out {
                    st.clear_transcript();
                    st.set_streaming_assistant(None);
                    st.reset_session_state(new_id.clone(), self.runtime.model().to_string());
                }
                out.println(&format!("new session started: {new_id}"));
            }
            "/export" => {
                let snapshot = self.runtime.snapshot();
                let events = self.runtime.event_log_path();
                let md = match tokio::fs::read_to_string(&events).await {
                    Ok(raw) => {
                        let mut md_lines = vec![
                            format!("# Session {}", snapshot.id),
                            String::new(),
                        ];
                        for line in raw.lines() {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line)
                                && let Some(kind) = val.get("kind").and_then(|v| v.as_str())
                            {
                                match kind {
                                    "MessageReceived" => {
                                        let role = val.get("role").and_then(|v| v.as_str()).unwrap_or("?");
                                        let content =
                                            val.get("content").and_then(|v| v.as_str()).unwrap_or("");
                                        md_lines.push(format!("## {role}"));
                                        md_lines.push(String::new());
                                        md_lines.push(content.to_string());
                                        md_lines.push(String::new());
                                    }
                                    "ToolCallStarted" => {
                                        let tool = val.get("tool").and_then(|v| v.as_str()).unwrap_or("?");
                                        md_lines.push(format!("### tool: {tool}"));
                                        md_lines.push(String::new());
                                    }
                                    _ => {}
                                }
                            }
                        }
                        md_lines.join("\n")
                    }
                    Err(e) => {
                        out.eprintln(&format!("[export] failed to read event log: {e}"));
                        return Ok(true);
                    }
                };
                let export_path = self.runtime.workspace_root().join(format!(".nca/export-{}.md", snapshot.id));
                if let Some(parent) = export_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                match tokio::fs::write(&export_path, &md).await {
                    Ok(()) => out.println(&format!("exported to {}", export_path.display())),
                    Err(e) => out.eprintln(&format!("[export] {e}")),
                }
            }
            "/thinking" => {
                let mut cfg = self.runtime.config().clone();
                cfg.model.enable_thinking = !cfg.model.enable_thinking;
                let new_state = cfg.model.enable_thinking;
                match self.runtime.apply_nca_config(cfg) {
                    Ok(()) => {
                        if let Err(e) = self.runtime.config().save_workspace_file(self.runtime.workspace_root()) {
                            out.eprintln(&format!("[thinking] toggled but save failed: {e}"));
                        } else {
                            out.println(&format!("thinking {} (budget: {})", if new_state { "enabled" } else { "disabled" }, self.runtime.config().model.thinking_budget));
                        }
                    }
                    Err(e) => out.eprintln(&format!("[thinking] {e}")),
                }
            }
            "/tool-output" => {
                if let ReplOutput::Tui(state) = &out {
                    state.toggle_all_tool_output();
                    state.push_system("[tool-output] toggled".into());
                } else {
                    out.println("[tool-output] only available in TUI mode");
                }
            }
            _ => {
                if command.starts_with('/') {
                    let cmd_name = command.trim_start_matches('/');
                    if let Some((plugin_name, intercept)) =
                        self.runtime.check_command_before(cmd_name, rest)
                        && intercept.handled
                    {
                        out.println(&format!("[{plugin_name}] {}", intercept.text));
                        return Ok(true);
                    }
                }
                if command.starts_with('/')
                    && self
                        .try_run_skill(command.trim_start_matches('/'), rest, &out)
                        .await?
                {
                    return Ok(true);
                }
                out.eprintln(&format!("unknown command: {command}"));
            }
        }

        Ok(true)
    }

    async fn run_preset(
        &mut self,
        prefix: &str,
        task: &str,
        out: ReplOutput<'_>,
    ) -> anyhow::Result<()> {
        if task.trim().is_empty() {
            out.println("usage: /<command> <task description>");
            return Ok(());
        }
        let prompt = format!("{prefix}{}", task.trim());
        let prompt = match expand_at_file_mentions_default(&prompt, self.runtime.workspace_root()) {
            Ok(s) => s,
            Err(e) => {
                out.eprintln(&format!("file mentions: {e}"));
                return Ok(());
            }
        };
        match self.runtime.run_turn(&prompt).await {
            Ok(output) => {
                if matches!(out, ReplOutput::Stdio) {
                    out.println(&output);
                }
            }
            Err(err) => {
                out.eprintln(&format!("error: {err}"));
            }
        }
        Ok(())
    }

    async fn try_run_skill(
        &mut self,
        skill_name: &str,
        task: &str,
        out: &ReplOutput<'_>,
    ) -> anyhow::Result<bool> {
        let skills = SkillCatalog::discover(
            self.runtime.workspace_root(),
            &self.runtime.config().harness.skill_directories,
        )
        .map_err(anyhow::Error::msg)?;
        let Some(skill) = skills.into_iter().find(|skill| skill.command == skill_name) else {
            return Ok(false);
        };

        // Route provider if the skill specifies one.
        if let Some(provider) = skill.provider {
            match self
                .runtime
                .switch_provider(provider, skill.model.as_deref())
            {
                Ok(effective_model) => {
                    out.println(&format!(
                        "[skill] provider={:?} model={effective_model}",
                        provider
                    ));
                }
                Err(e) => {
                    out.eprintln(&format!(
                        "[skill] failed to switch to provider {:?}: {e}",
                        provider
                    ));
                    return Ok(true);
                }
            }
        } else if let Some(model) = &skill.model {
            // No explicit provider — just override model (existing behavior).
            self.runtime
                .set_model(self.runtime.config().model.resolve_alias(model));
        }
        if let Some(mode) = skill.permission_mode {
            self.runtime.set_permission_mode(mode);
        }

        let prompt = skill.prompt_for_task(task);
        let prompt = match expand_at_file_mentions_default(&prompt, self.runtime.workspace_root()) {
            Ok(s) => s,
            Err(e) => {
                out.eprintln(&format!("file mentions: {e}"));
                return Ok(true);
            }
        };
        match self.runtime.run_turn(&prompt).await {
            Ok(output) => {
                if matches!(out, ReplOutput::Stdio) {
                    out.println(&output);
                }
            }
            Err(err) => {
                out.eprintln(&format!("error: {err}"));
            }
        }
        Ok(true)
    }

    /// Full-screen TUI: transcript + streaming + composer (default on TTY).
    pub async fn run_with_tui(&mut self) -> anyhow::Result<()> {
        let session_id = self.runtime.session_id().to_string();
        let model = self.runtime.model().to_string();
        let perm = format!("{:?}", self.runtime.permission_mode());

        // Elm feedback channel: bridge → NcaModel for rendering.
        let (feedback_tx, feedback_rx) = tokio::sync::mpsc::unbounded_channel::<TuiFeedbackMsg>();

        // TuiFeedbackChannel: all writes from cmd_rx loop go through this.
        let tui_feedback: Arc<TuiFeedbackChannel> =
            Arc::new(TuiFeedbackChannel::new(feedback_tx.clone()));

        let log_path = self.runtime.event_log_path();
        // Replay historical events into the Elm feedback channel (consumed by NcaModel at startup).
        replay_events_to_feedback(&log_path, &feedback_tx).await;

        // Populate the git branch name immediately so it appears on first render.
        let workspace = self.runtime.workspace_root();
        if let Some(branch) = git_current_branch(workspace) {
            tui_feedback.set_current_branch(&branch);
        }

        let rx = self
            .runtime
            .take_event_rx()
            .ok_or_else(|| anyhow::anyhow!("internal: event channel already taken"))?;
        let ipc = self.runtime.take_ipc_handle();
        let approval = self.runtime.take_ipc_approval_pending();
        let question = self.runtime.question_pending();

        // Shared handles for cmd_rx loop (read active question / staged images from Elm side).
        let active_question_id = tui_feedback.active_question_id_handle();
        let active_question_payload = tui_feedback.active_question_payload_handle();
        let active_approval_payload: std::sync::Arc<
            std::sync::Mutex<Option<crate::tui::state::ApprovalRequest>>,
        > = std::sync::Arc::new(std::sync::Mutex::new(None));
        let staged_images = tui_feedback.staged_images_handle();

        let _bridge = spawn_tui_bridge(
            rx,
            log_path,
            ipc,
            approval.clone(),
            question.clone(),
            feedback_tx.clone(),
        );

        let _spawn_task = {
            let spawn_rx = self.runtime.take_spawn_rx();
            let event_tx = self.runtime.event_tx();
            if let Some(srx) = spawn_rx {
                Some(nca_runtime::supervisor::spawn_subagent_consumer(
                    srx,
                    self.runtime.session_id().to_string(),
                    self.runtime.workspace_root().to_path_buf(),
                    self.runtime.config().clone(),
                    self.runtime.messages().to_vec(),
                    event_tx,
                ))
            } else {
                None
            }
        };

        // Answers must bypass the main `cmd_rx` loop: while `run_turn` is blocked inside
        // `ask_question`, that task never receives `TuiCmd::Submit` or `QuestionAnswer`.
        let (answer_tx, mut answer_rx) =
            tokio::sync::mpsc::unbounded_channel::<(String, QuestionSelection)>();
        let qp_dispatch = question.clone();
        tokio::spawn(async move {
            while let Some((qid, sel)) = answer_rx.recv().await {
                let _ = dispatch_question_answer(&qp_dispatch, &qid, sel);
            }
        });
        let answer_for_tui = answer_tx.clone();
        drop(answer_tx);

        let (approval_tx, mut approval_rx) =
            tokio::sync::mpsc::unbounded_channel::<ApprovalAnswer>();
        let approval_dispatch = approval.clone();
        let tui_fb = Arc::clone(&tui_feedback);
        tokio::spawn(async move {
            while let Some(answer) = approval_rx.recv().await {
                let (call_id, verdict) = match answer {
                    ApprovalAnswer::Verdict { call_id, approved } => (
                        call_id,
                        if approved {
                            nca_core::approval::ApprovalVerdict::Approved
                        } else {
                            nca_core::approval::ApprovalVerdict::Denied
                        },
                    ),
                    ApprovalAnswer::AllowPattern { call_id, pattern } => (
                        call_id,
                        nca_core::approval::ApprovalVerdict::AllowPattern(pattern),
                    ),
                };
                if !dispatch_tool_approval(&approval_dispatch, &call_id, verdict) {
                    tui_fb.push_error(
                        "approval was no longer pending; cleared stale approval state".into(),
                    );
                }
            }
        });
        let approval_for_tui = approval_tx.clone();
        drop(approval_tx);

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Msg>();
        let cancel_flag = self.runtime.cancel_handle();

        // Elm NcaModel params
        let skill_dirs = self.runtime.config().harness.skill_directories.clone();
        let params = crate::tui::elm::run::NcaModelParams {
            session_id: session_id.clone(),
            model: model.clone(),
            agent_label: self.current_agent_label.clone(),
            permission_mode: perm.clone(),
            workspace_root: self.runtime.workspace_root().to_path_buf(),
            skill_dirs,
            plugin_commands: self.runtime.plugin_commands(),
        };

        let ui = tokio::task::spawn_blocking(move || {
            run_nca_model(
                feedback_rx,
                cmd_tx,
                Some(answer_for_tui),
                Some(approval_for_tui),
                Some(cancel_flag),
                Arc::clone(&active_question_id),
                Arc::clone(&active_question_payload),
                Arc::clone(&active_approval_payload),
                Arc::clone(&staged_images),
                params,
            )
        });

        let feedback_tx_clone = feedback_tx.clone();

        loop {
            let msg = cmd_rx.recv().await;
            let Some(msg) = msg else { break };
            match msg {
                Msg::Cmd(cmd) => match cmd {
                    TuiCmd::Exit => {
                        tui_feedback.should_exit();
                        break;
                    }
                    TuiCmd::CycleAgent => {
                        let entries = self.agent_entries();
                        if entries.len() > 1 {
                            self.agent_index = (self.agent_index + 1) % entries.len();
                            let name = entries[self.agent_index].clone();
                            self.current_agent_label = format!("@{name}");
                            let profile_name = if self.agent_index == 0 {
                                None
                            } else {
                                Some(name.as_str())
                            };
                            if let Err(e) = self.runtime.apply_agent_profile(profile_name) {
                                tui_feedback.push_error(format!("Failed to switch agent: {e}"));
                            }
                            tui_feedback.set_agent_profile(self.current_agent_label.clone());
                            tui_feedback.set_permission_mode(format!(
                                "{:?}",
                                self.runtime.permission_mode()
                            ));
                            tui_feedback.set_model(self.runtime.model().to_string());
                        }
                    }
                    TuiCmd::CancelTurn => {
                        self.runtime.request_cancel();
                    }
                    TuiCmd::OpenBranchPicker => {
                        let workspace = self.runtime.workspace_root();
                        let branches = git_list_branches(workspace);
                        let current = git_current_branch(workspace).unwrap_or_default();
                        tui_feedback.open_branch_picker(branches, &current);
                        tui_feedback.set_current_branch(&current);
                    }
                    TuiCmd::SwitchBranch(name) => {
                        let workspace = self.runtime.workspace_root();
                        if git_switch_branch(workspace, &name) {
                            tui_feedback.set_current_branch(&name);
                            tui_feedback.push_system(format!("Switched to branch: {}", name));
                        } else {
                            tui_feedback
                                .push_error(format!("Failed to switch to branch: {}", name));
                        }
                    }
                    TuiCmd::CreateBranch(name) => {
                        let workspace = self.runtime.workspace_root();
                        if git_create_branch(workspace, &name) {
                            tui_feedback.set_current_branch(&name);
                            tui_feedback
                                .push_system(format!("Created and switched to branch: {}", name));
                        } else {
                            tui_feedback.push_error(format!("Failed to create branch: {}", name));
                        }
                    }
                    TuiCmd::ApplyDefaultProvider(p) => {
                        self.apply_provider_in_session(p, ReplOutput::Tui(tui_feedback.as_ref()))
                            .await?;
                    }
                    TuiCmd::PromptApiKey(p, connect_after_save) => {
                        let has_key = self.runtime.config().provider.api_key_present_for(p);
                        if has_key {
                            self.apply_provider_in_session(
                                p,
                                ReplOutput::Tui(tui_feedback.as_ref()),
                            )
                            .await?;
                        } else {
                            tui_feedback.open_api_key_modal(p, false, connect_after_save);
                        }
                    }
                    TuiCmd::ApplyModel(model_name) => {
                        let mut cfg = self.runtime.config().clone();
                        cfg.apply_model_override(&model_name);
                        cfg.model.track_recent_model(
                            &self.runtime.config().model.resolve_alias(&model_name),
                        );
                        match self.runtime.apply_nca_config(cfg) {
                            Ok(()) => {
                                if let Err(e) = self
                                    .runtime
                                    .config()
                                    .save_workspace_file(self.runtime.workspace_root())
                                {
                                    tui_feedback
                                        .push_error(format!("[model] workspace save failed: {e}"));
                                } else {
                                    tui_feedback.set_model(self.runtime.model().to_string());
                                    tui_feedback.push_system(format!(
                                        "[model] switched to {} (saved)",
                                        self.runtime.model()
                                    ));
                                }
                            }
                            Err(e) => {
                                tui_feedback.push_error(format!("[model] {e}"));
                            }
                        }
                    }
                    TuiCmd::ApplyModelProvider(p) => {
                        self.apply_provider_in_session(p, ReplOutput::Tui(tui_feedback.as_ref()))
                            .await?;
                    }
                    TuiCmd::ApplyPermission(idx) => {
                        let mode = PermissionMode::from_index(idx);
                        self.runtime.set_permission_mode(mode);
                        tui_feedback.set_permission_mode(format!("{mode:?}"));
                        tui_feedback.push_system(format!("permission mode set to {mode:?}"));
                    }
                    TuiCmd::SwitchAgent(idx) => {
                        let entries = self.agent_entries();
                        if let Some(name) = entries.get(idx) {
                            self.agent_index = idx;
                            let name = name.clone();
                            self.current_agent_label = format!("@{name}");
                            let profile_name = if idx == 0 { None } else { Some(name.as_str()) };
                            if let Err(e) = self.runtime.apply_agent_profile(profile_name) {
                                tui_feedback.push_error(format!("Failed to switch agent: {e}"));
                            }
                            tui_feedback.set_agent_profile(self.current_agent_label.clone());
                            tui_feedback.set_permission_mode(format!(
                                "{:?}",
                                self.runtime.permission_mode()
                            ));
                            tui_feedback.set_model(self.runtime.model().to_string());
                            tui_feedback.push_system(format!("switched to @{name}"));
                        }
                    }
                    TuiCmd::OpenEditor => {
                        self.handle_command("/editor", ReplOutput::Tui(tui_feedback.as_ref()))
                            .await?;
                    }
                    TuiCmd::NewSession => {
                        self.handle_command("/new", ReplOutput::Tui(tui_feedback.as_ref()))
                            .await?;
                    }
                    TuiCmd::RunCompact => {
                        self.handle_command("/compact", ReplOutput::Tui(tui_feedback.as_ref()))
                            .await?;
                    }
                    TuiCmd::OpenModelPicker => {
                        self.handle_command("/models", ReplOutput::Tui(tui_feedback.as_ref()))
                            .await?;
                    }
                    TuiCmd::OpenStatus => {
                        self.handle_command("/status", ReplOutput::Tui(tui_feedback.as_ref()))
                            .await?;
                    }
                    TuiCmd::OpenHelp => {
                        self.handle_command("/help", ReplOutput::Tui(tui_feedback.as_ref()))
                            .await?;
                    }
                    TuiCmd::OpenAgentPicker => {
                        let labels = self.build_agent_picker_labels();
                        tui_feedback.open_agent_picker(labels, self.agent_index);
                    }
                    TuiCmd::OpenSessions => {
                        self.handle_command("/sessions", ReplOutput::Tui(tui_feedback.as_ref()))
                            .await?;
                    }
                    TuiCmd::ResumeSession(session_id) => {
                        let current = self.runtime.session_id().to_string();
                        if session_id == current {
                            tui_feedback.push_system("Already on this session.".into());
                        } else {
                            match self.runtime.switch_to(&session_id).await {
                                Ok(()) => {
                                    let sid = self.runtime.session_id().to_string();
                                    let model = self.runtime.model().to_string();
                                    tui_feedback.reset_session_state(sid, model);
                                    let log_path = self.runtime.event_log_path();
                                    replay_events_to_feedback(&log_path, &feedback_tx_clone).await;
                                    tui_feedback
                                        .push_system(format!("Switched to session {session_id}"));
                                }
                                Err(err) => {
                                    tui_feedback.push_system(format!(
                                        "[!] Failed to switch to session {session_id}: {err}"
                                    ));
                                }
                            }
                        }
                    }
                    TuiCmd::CycleModel(forward) => {
                        let recent = &self.runtime.config().model.recent_models;
                        if recent.len() >= 2 {
                            let current = self.runtime.model().to_string();
                            let pos = recent.iter().position(|m| m == &current).unwrap_or(0);
                            let next_pos = if forward {
                                (pos + 1) % recent.len()
                            } else {
                                pos.checked_sub(1).unwrap_or(recent.len() - 1)
                            };
                            let next_model = recent[next_pos].clone();
                            let mut cfg = self.runtime.config().clone();
                            cfg.apply_model_override(&next_model);
                            if let Ok(()) = self.runtime.apply_nca_config(cfg) {
                                let _ = self
                                    .runtime
                                    .config()
                                    .save_workspace_file(self.runtime.workspace_root());
                                tui_feedback.set_model(self.runtime.model().to_string());
                                tui_feedback.push_system(format!(
                                    "[F2] switched to {}",
                                    self.runtime.model()
                                ));
                            }
                        } else {
                            tui_feedback.push_system(
                                "[F2] no recent models to cycle (need 2+ in model.recent_models)"
                                    .into(),
                            );
                        }
                    }
                    TuiCmd::ValidateApiKey(provider, api_key) => {
                        tui_feedback.set_validation_status(Some(
                            crate::tui::state::OnboardingValidation::Validating,
                        ));
                        let base_url = self
                            .runtime
                            .config()
                            .provider
                            .base_url_for(provider)
                            .to_string();
                        let result = nca_core::provider::validate::validate_api_key(
                            provider, &api_key, &base_url,
                        )
                        .await;
                        match &result {
                            nca_core::provider::validate::ValidationResult::Valid => {
                                tui_feedback.set_validation_status(Some(
                                    crate::tui::state::OnboardingValidation::Valid,
                                ));
                                tui_feedback.close_api_key_modal();
                                tui_feedback.close_connect_modal();
                                tui_feedback.set_onboarding_mode(false);
                            }
                            nca_core::provider::validate::ValidationResult::InvalidKey(msg) => {
                                tui_feedback.set_validation_status(Some(
                                    crate::tui::state::OnboardingValidation::Failed(msg.clone()),
                                ));
                            }
                            nca_core::provider::validate::ValidationResult::NetworkError(msg) => {
                                tui_feedback.set_validation_status(Some(
                                    crate::tui::state::OnboardingValidation::Failed(msg.clone()),
                                ));
                            }
                        }
                        if matches!(
                            result,
                            nca_core::provider::validate::ValidationResult::Valid
                        ) {
                            let mut cfg = self.runtime.config().clone();
                            cfg.set_provider_api_key(provider, &api_key);
                            cfg.set_default_provider(provider);
                            if let Err(e) = self.runtime.apply_nca_config(cfg) {
                                tracing::warn!("onboarding: provider apply failed: {e}");
                                tui_feedback.set_validation_status(Some(
                                    crate::tui::state::OnboardingValidation::Failed(format!(
                                        "Failed to apply provider: {e}"
                                    )),
                                ));
                                tui_feedback.set_onboarding_mode(true);
                                continue;
                            }
                            tui_feedback.set_model(self.runtime.model().to_string());
                            let mut cfg = self.runtime.config().clone();
                            cfg.ui.onboarding_completed = true;
                            if let Err(e) = cfg.save_global() {
                                tracing::warn!("onboarding: global config save failed: {e}");
                            }
                            let _ = self.runtime.apply_nca_config(cfg);
                        }
                    }
                    TuiCmd::QuestionAnswer(selection) => {
                        let qid = tui_feedback.get_active_question_id();
                        if let Some(qid) = qid
                            && !self.runtime.submit_question_answer(&qid, selection)
                        {
                            tui_feedback.push_error(
                                "failed to submit answer (expired or already answered)".into(),
                            );
                        }
                    }
                    TuiCmd::Submit(line) => {
                        let line = line.trim().to_string();
                        if line.starts_with('!') {
                            let shell_cmd = line.trim_start_matches('!').trim();
                            self.run_bash_tui(shell_cmd, tui_feedback.as_ref()).await;
                            continue;
                        }
                        if line.starts_with('/') {
                            if !self
                                .handle_command(&line, ReplOutput::Tui(tui_feedback.as_ref()))
                                .await?
                            {
                                tui_feedback.should_exit();
                                break;
                            }
                            continue;
                        }
                        let expanded = match expand_at_file_mentions_default(
                            &line,
                            self.runtime.workspace_root(),
                        ) {
                            Ok(s) => s,
                            Err(e) => {
                                tui_feedback.push_error(format!("file mentions: {e}"));
                                continue;
                            }
                        };
                        tui_feedback.set_busy(true);
                        let attachments = tui_feedback.take_staged_images();
                        let turn = if attachments.is_empty() {
                            self.runtime.run_turn(&expanded).await
                        } else {
                            self.runtime
                                .run_turn_with_images(&expanded, attachments)
                                .await
                        };
                        if let Err(e) = turn {
                            tracing::error!(
                                error = %e,
                                "provider_turn_error"
                            );
                            tui_feedback.push_error(e.to_string());
                        }
                        tui_feedback.set_busy(false);
                        tui_feedback.set_busy_state(BusyState::Idle);
                    }
                },
                Msg::Quit => {
                    break;
                }
                _ => {}
            }
        }

        let _ = ui.await;
        self.runtime.finish(EndReason::UserExit).await;
        Ok(())
    }

    async fn run_bash_tui(&self, cmd: &str, fb: &dyn TuiFeedback) {
        if cmd.is_empty() {
            fb.push_system("! usage: !<command>".into());
            return;
        }
        fb.push_system(format!("[bash] {cmd}"));
        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;
        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                for line in stdout.lines() {
                    fb.push_system(line.to_string());
                }
                if !stderr.is_empty() {
                    fb.push_system(format!("[stderr] {stderr}"));
                }
                fb.push_system(if out.status.success() {
                    "[bash] exit 0".into()
                } else {
                    format!("[bash] exit {}", out.status.code().unwrap_or(-1))
                });
            }
            Err(e) => fb.push_system(format!("[bash] {e}")),
        }
    }
}

/// Inline hint provider for slash commands.
/// Shows the first matching command as greyed-out text; Tab accepts it.
/// When no slash command is being typed, returns empty (Tab falls through to agent switch).
struct SlashHinter {
    /// Skill directories from config — used to discover skills for `/` completion.
    skill_directories: Vec<std::path::PathBuf>,
    /// Workspace root for resolving relative skill directories.
    workspace_root: std::path::PathBuf,
    /// Slash commands contributed by plugins.
    plugin_commands: Vec<String>,
    hint_suffix: String,
}

impl Hinter for SlashHinter {
    fn handle(
        &mut self,
        line: &str,
        _pos: usize,
        _history: &dyn reedline::History,
        _use_ansi_coloring: bool,
        _cwd: &str,
    ) -> String {
        self.hint_suffix.clear();

        if !line.starts_with('/') {
            return String::new();
        }

        let command = line.split_whitespace().next().unwrap_or(line);

        // Find the first (alphabetically) matching slash command
        for &cmd in SLASH_COMMANDS {
            if cmd.starts_with(command) && cmd != command {
                let suffix = &cmd[command.len()..];
                self.hint_suffix = suffix.to_string();
                return suffix.to_string();
            }
        }

        // Also check discovered skills (using config's skill_directories so
        // user-configured paths are respected, not just XDG defaults).
        if let Ok(skills) = SkillCatalog::discover(&self.workspace_root, &self.skill_directories) {
            let mut skill_match: Option<String> = None;
            for skill in &skills {
                let skill_cmd = format!("/{}", skill.command);
                if skill_cmd.starts_with(command) && skill_cmd != command {
                    let suffix = &skill_cmd[command.len()..];
                    skill_match = Some(suffix.to_string());
                    break;
                }
            }
            if let Some(suffix) = skill_match {
                self.hint_suffix = suffix.clone();
                return suffix;
            }
        }

        // Also check plugin-contributed slash commands.
        for cmd in &self.plugin_commands {
            let full = format!("/{cmd}");
            if full.starts_with(command) && full != command {
                let suffix = &full[command.len()..];
                self.hint_suffix = suffix.to_string();
                return suffix.to_string();
            }
        }

        String::new()
    }

    fn complete_hint(&self) -> String {
        self.hint_suffix.clone()
    }

    fn next_hint_token(&self) -> String {
        self.hint_suffix.clone()
    }
}

fn build_model_picker_entries(
    config: &nca_common::config::NcaConfig,
    provider_models: &[String],
) -> Vec<ModelPickerEntry> {
    let mut entries = Vec::new();
    entries.push(ModelPickerEntry {
        label: "Providers".into(),
        detail: String::new(),
        action: ModelPickerAction::ApplyModel(String::new()),
        is_header: true,
    });
    for p in ProviderKind::ALL {
        let model = config.provider.model_for(p);
        let key_status = if config.provider.api_key_present_for(p) {
            "key ✓"
        } else {
            "no key"
        };
        let selected = if p == config.provider.default {
            " [active]"
        } else {
            ""
        };
        entries.push(ModelPickerEntry {
            label: format!("{}{}", p.display_name(), selected),
            detail: format!("{model} ({key_status})"),
            action: ModelPickerAction::SwitchProvider(p),
            is_header: false,
        });
    }

    if !provider_models.is_empty() {
        entries.push(ModelPickerEntry {
            label: format!("{} models", config.provider.default.display_name()),
            detail: String::new(),
            action: ModelPickerAction::ApplyModel(String::new()),
            is_header: true,
        });
        for model_id in provider_models {
            entries.push(ModelPickerEntry {
                label: model_id.clone(),
                detail: String::new(),
                action: ModelPickerAction::ApplyModel(model_id.clone()),
                is_header: false,
            });
        }
    }

    entries.push(ModelPickerEntry {
        label: "Aliases".into(),
        detail: String::new(),
        action: ModelPickerAction::ApplyModel(String::new()),
        is_header: true,
    });
    for (alias, target) in &config.model.aliases {
        entries.push(ModelPickerEntry {
            label: alias.clone(),
            detail: format!("→ {target}"),
            action: ModelPickerAction::ApplyModel(alias.clone()),
            is_header: false,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_permission_aliases() {
        assert_eq!(
            "accept-edits".parse::<PermissionMode>().ok(),
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            "dontask".parse::<PermissionMode>().ok(),
            Some(PermissionMode::DontAsk)
        );
        assert_eq!(
            "bypass_permissions".parse::<PermissionMode>().ok(),
            Some(PermissionMode::BypassPermissions)
        );
        assert_eq!("invalid".parse::<PermissionMode>().ok(), None);
    }
}
