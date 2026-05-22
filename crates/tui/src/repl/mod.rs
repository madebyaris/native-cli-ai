pub mod agent_profile;
pub mod commands;
pub mod input;
pub mod render;
pub mod tui_mode;

pub use agent_profile::AgentProfile;
pub(crate) use render::ReplOutput;

use commands::{build_model_picker_entries, parse_permission_mode, permission_mode_index};

use crate::file_mentions::expand_at_file_mentions_default;
use crate::prompt::NcaPrompt;
use crate::runner::SessionRuntime;
use nca_common::config::{PermissionMode, ProviderCompatibility, ProviderKind};
use nca_common::event::{EndReason, QuestionSelection};
use nca_core::skills::SkillCatalog;
use nca_runtime::memory_store::MemoryStore;
use reedline::{Emacs, FileBackedHistory, Reedline, Signal, Vi};
use std::process::Stdio;
use tokio::process::Command;

/// Session state for REPL
pub struct Repl {
    runtime: SessionRuntime,
    prompt: NcaPrompt,
    run_mode: bool,
    history_path: std::path::PathBuf,
    agent_profile: AgentProfile,
    current_agent_label: String,
}

impl Repl {
    pub fn new(runtime: SessionRuntime, safe_mode: bool, run_mode: bool) -> Self {
        let history_path = runtime.workspace_root().join(".nca/.history");
        let agent_profile = AgentProfile::default();
        let current_agent_label = format!("@{}", agent_profile.label());
        Self {
            runtime,
            prompt: NcaPrompt::new(safe_mode, run_mode),
            run_mode,
            history_path,
            agent_profile,
            current_agent_label,
        }
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

                    // Tab switches agent profile (OpenCode-style)
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
║    Tab       Switch agent profile (@build/@plan/@review...)   ║
║    Ctrl+D    Exit                                            ║
║    Ctrl+C    Cancel current request                           ║
║    Ctrl+L    Clear screen                                     ║
║    Ctrl+R    Search command history                           ║
╚══════════════════════════════════════════════════════════════╝
"#
        );
    }

    /// Switch to the next agent profile (called on Tab press)
    fn switch_agent(&mut self) {
        let next = self.agent_profile.next();
        self.agent_profile = next;
        self.current_agent_label = format!("@{}", next.label());
        self.prompt.set_agent(&self.current_agent_label);

        // Update runtime permission mode based on profile
        if next == AgentProfile::Plan {
            self.runtime.set_permission_mode(PermissionMode::Plan);
        }

        eprintln!("\n[agent] Switched to @{} mode", next.label());
        if next == AgentProfile::Plan {
            eprintln!("[agent] Plan mode: file edits and shell commands are disabled");
        }
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
        if p == ProviderKind::Custom
            && self
                .runtime
                .config()
                .provider
                .custom
                .base_url
                .trim()
                .is_empty()
        {
            out.eprintln(
                "[provider] custom provider is not configured yet; run /provider → \"Add custom provider…\", or: /custom <openai|anthropic> <base-url> [api-key] [model]",
            );
            return Ok(());
        }
        let mut cfg = self.runtime.config().clone();
        cfg.set_default_provider(p);
        match self.runtime.apply_nca_config(cfg) {
            Ok(()) => {
                if let ReplOutput::Tui(st) = &out
                    && let Ok(mut g) = st.lock()
                {
                    g.model = self.runtime.model().to_string();
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

    /// Save custom provider fields, switch default to Custom, and persist workspace config.
    async fn persist_custom_provider_config(
        &mut self,
        compatibility: ProviderCompatibility,
        base_url: String,
        api_key: Option<String>,
        model: Option<String>,
        out: ReplOutput<'_>,
    ) -> anyhow::Result<()> {
        let mut cfg = self.runtime.config().clone();
        cfg.set_custom_compatibility(compatibility);
        cfg.set_provider_base_url(ProviderKind::Custom, base_url);
        if let Some(k) = api_key.filter(|k| !k.trim().is_empty()) {
            cfg.set_provider_api_key(ProviderKind::Custom, k);
        }
        if let Some(m) = model.filter(|m| !m.trim().is_empty()) {
            cfg.provider.set_model_for(ProviderKind::Custom, m);
        }
        cfg.set_default_provider(ProviderKind::Custom);

        match self.runtime.apply_nca_config(cfg) {
            Ok(()) => {
                if let Err(e) = self
                    .runtime
                    .config()
                    .save_workspace_file(self.runtime.workspace_root())
                {
                    out.eprintln(&format!("[custom] applied but workspace save failed: {e}"));
                } else {
                    out.println(&format!(
                        "[custom] {} at {} — model {} — saved .nca/config.local.toml",
                        compatibility.display_name(),
                        self.runtime.config().provider.custom.base_url,
                        self.runtime.model()
                    ));
                }
                if let ReplOutput::Tui(st) = out
                    && let Ok(mut g) = st.lock()
                {
                    g.model = self.runtime.model().to_string();
                }
            }
            Err(e) => out.eprintln(&format!("[custom] {e}")),
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
            .with_quick_completions(true)
            .with_partial_completions(true)
            .with_ansi_colors(true);

        // Try to load history from disk
        if let Some(parent) = self.history_path.parent() {
            std::fs::create_dir_all(parent).ok();
            if let Ok(history) = FileBackedHistory::with_file(100, self.history_path.clone()) {
                builder = builder.with_history(Box::new(history));
            }
        }

        // Support vim mode if enabled via env
        if std::env::var("NCA_EDITOR_MODE")
            .map(|v| v.eq_ignore_ascii_case("vi") || v.eq_ignore_ascii_case("vim"))
            .unwrap_or(false)
        {
            builder = builder.with_edit_mode(Box::new(Vi::default()));
        } else {
            builder = builder.with_edit_mode(Box::new(Emacs::default()));
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
                    "  /agent [profile]   Show or switch agent profile".into(),
                    "  /plan <task>       Planning-oriented turn".into(),
                    "  /review <task>     Code review turn".into(),
                    "  /fix <task>        Bug-fix turn".into(),
                    "  /test <task>       Validation turn".into(),
                    "  /clear             Clear the screen".into(),
                    "  /compact           Compact session summary".into(),
                    "  /new               Start a new session".into(),
                    "  /export            Export session to markdown".into(),
                    "  /thinking          Toggle thinking/reasoning visibility".into(),
                    "  /skills            List discovered skills".into(),
                    "  /memory [text]     Show or store memory notes".into(),
                    "  /models            Browse and select models".into(),
                    "  /connect           Connect LLM provider".into(),
                    "  /provider [name]   Default provider".into(),
                    "  /custom            Configure custom endpoint".into(),
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
                    "  Tab          Cycle agent profile".into(),
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
                    if let Ok(mut g) = st.lock() {
                        g.open_info_modal("help", help_lines);
                    }
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
                    format!("Agent:       @{}", self.agent_profile.label()),
                    format!("Permission:  {:?}", self.runtime.permission_mode()),
                    format!("Children:    {}", snapshot.child_session_ids.len()),
                    format!("Memory:      {}", self.runtime.memory_store_path().display()),
                ];
                if let Some(summary) = &snapshot.session_summary {
                    lines.push(String::new());
                    lines.push(format!("Summary: {}", summary.replace('\n', " ")));
                }
                if let ReplOutput::Tui(st) = &out {
                    if let Ok(mut g) = st.lock() {
                        g.open_info_modal("status", lines);
                    }
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/agent" => {
                if let Some(target) = parts.next() {
                    let target_clean = target.trim_start_matches('@').to_lowercase();
                    let matched = AgentProfile::ALL.iter().find(|p| {
                        p.label() == target_clean
                    });
                    if let Some(profile) = matched {
                        self.agent_profile = *profile;
                        self.current_agent_label = format!("@{}", profile.label());
                        self.prompt.set_agent(&self.current_agent_label);
                        if *profile == AgentProfile::Plan {
                            self.runtime.set_permission_mode(PermissionMode::Plan);
                        } else {
                            self.runtime.set_permission_mode(PermissionMode::Default);
                        }
                        if let ReplOutput::Tui(st) = &out
                            && let Ok(mut g) = st.lock()
                        {
                            g.set_agent_profile(&self.current_agent_label);
                            g.set_permission_mode(&format!(
                                "{:?}",
                                self.runtime.permission_mode()
                            ));
                        }
                        out.println(&format!("Switched to @{} mode", profile.label()));
                    } else {
                        out.println(&format!("Unknown agent profile: {}", target));
                        out.println(&format!(
                            "Available: {}",
                            AgentProfile::ALL
                                .iter()
                                .map(|p| p.label())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                } else if let ReplOutput::Tui(st) = &out {
                    let current_idx = AgentProfile::ALL
                        .iter()
                        .position(|p| *p == self.agent_profile)
                        .unwrap_or(0);
                    if let Ok(mut g) = st.lock() {
                        g.open_agent_picker(current_idx);
                    }
                } else {
                    out.println(&format!("Current agent: @{}", self.agent_profile.label()));
                    out.println("Available profiles:");
                    for profile in AgentProfile::ALL {
                        let marker = if profile == self.agent_profile { " *" } else { "" };
                        out.println(&format!("  @{}{}", profile.label(), marker));
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
                    let resolved = self.runtime.config().model.resolve_alias(model);
                    let mut cfg = self.runtime.config().clone();
                    cfg.apply_model_override(&resolved);
                    cfg.model.track_recent_model(&resolved);
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
                            if let ReplOutput::Tui(st) = out
                                && let Ok(mut g) = st.lock()
                            {
                                g.model = self.runtime.model().to_string();
                            }
                        }
                        Err(e) => out.eprintln(&format!("[model] {e}")),
                    }
                } else if let ReplOutput::Tui(st) = &out {
                    let provider_models = nca_runtime::model_limits_api::fetch_provider_model_ids(self.runtime.config()).await;
                    let entries = build_model_picker_entries(self.runtime.config(), &provider_models);
                    if let Ok(mut g) = st.lock() {
                        g.open_model_picker(entries);
                    }
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
                let cfg = self.runtime.config();
                let resolved = cfg.model.resolve_alias(&snapshot.model);
                let pricing = cfg.model.pricing_for(&resolved);
                let mut lines = vec![
                    format!("Session:    {}", snapshot.id),
                    format!("Model:      {resolved}"),
                    format!(
                        "Pricing:    ${:.3}/M in  ${:.3}/M out",
                        pricing.input_per_million, pricing.output_per_million
                    ),
                    String::new(),
                    format!("Input:      {:>10} tokens", snapshot.total_input_tokens),
                    format!("Output:     {:>10} tokens", snapshot.total_output_tokens),
                    format!(
                        "Total:      {:>10} tokens",
                        snapshot.total_input_tokens + snapshot.total_output_tokens
                    ),
                    format!("Cost:       ${:.4}", snapshot.estimated_cost_usd),
                ];
                if let ReplOutput::Tui(st) = &out
                    && let Ok(g) = st.lock()
                    && !g.subagents.is_empty()
                {
                    let mut child_in = 0u64;
                    let mut child_out = 0u64;
                    for row in g.subagents.iter() {
                        child_in += row.tokens_in;
                        child_out += row.tokens_out;
                    }
                    if child_in > 0 || child_out > 0 {
                        lines.push(String::new());
                        lines.push(format!(
                            "Sub-agents: {} active, {child_in} in / {child_out} out",
                            g.subagents.len()
                        ));
                    }
                }
                if let ReplOutput::Tui(st) = &out {
                    if let Ok(mut g) = st.lock() {
                        g.open_info_modal("cost", lines);
                    }
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/stats" => {
                let snapshot = self.runtime.snapshot();
                let lines = vec![
                    format!("Session:     {}", snapshot.id),
                    format!("Model:       {}", self.runtime.model()),
                    format!("Agent:       @{}", self.agent_profile.label()),
                    format!("Permission:  {:?}", self.runtime.permission_mode()),
                    format!("Children:    {}", snapshot.child_session_ids.len()),
                    format!("Memory:      {}", self.runtime.memory_store_path().display()),
                ];
                if let ReplOutput::Tui(st) = &out {
                    if let Ok(mut g) = st.lock() {
                        g.open_info_modal("stats", lines);
                    }
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/permissions" => {
                if let Some(mode) = parts.next() {
                    if let Some(parsed_mode) = parse_permission_mode(mode) {
                        self.runtime.set_permission_mode(parsed_mode);
                        if let ReplOutput::Tui(st) = out
                            && let Ok(mut g) = st.lock()
                        {
                            g.set_permission_mode(&format!("{parsed_mode:?}"));
                        }
                        out.println(&format!("permission mode set to {parsed_mode:?}"));
                    } else {
                        out.println(
                            "invalid mode; expected one of: default, plan, accept-edits, dont-ask, bypass-permissions",
                        );
                    }
                } else if let ReplOutput::Tui(st) = &out {
                    let current_idx = permission_mode_index(self.runtime.permission_mode());
                    if let Ok(mut g) = st.lock() {
                        g.open_permission_picker(current_idx);
                    }
                } else {
                    out.println(&format!(
                        "permission_mode: {:?}",
                        self.runtime.permission_mode()
                    ));
                }
            }
            "/permission-bypass" => {
                let sub = parts.next().unwrap_or("").trim();
                let target = match sub.to_ascii_lowercase().as_str() {
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
                            "usage: /permission-bypass [on|off|toggle] — default toggles bypass ↔ default",
                        );
                        return Ok(true);
                    }
                };
                self.runtime.set_permission_mode(target);
                if let ReplOutput::Tui(st) = out
                    && let Ok(mut g) = st.lock()
                {
                    g.set_permission_mode(&format!("{target:?}"));
                }
                out.println(&format!("permission mode set to {target:?}"));
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
                        if let Ok(mut g) = st.lock() {
                            g.open_info_modal("skills", lines);
                        }
                    } else {
                        out.println("no skills discovered");
                    }
                } else {
                    let lines: Vec<String> = skills.iter().map(|s| s.summary_line()).collect();
                    if let ReplOutput::Tui(st) = &out {
                        if let Ok(mut g) = st.lock() {
                            g.open_info_modal("skills", lines);
                        }
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
                            if let Ok(mut g) = st.lock() {
                                g.open_info_modal("memory", lines);
                            }
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
                            if let Ok(mut g) = st.lock() {
                                g.open_info_modal("memory", lines);
                            }
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
                    if let Ok(mut g) = st.lock() {
                        g.open_model_picker(entries);
                    }
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
                    if let Ok(mut g) = st.lock() {
                        g.open_info_modal("mcp", lines);
                    }
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
                    if let Ok(mut g) = st.lock() {
                        g.open_info_modal("agents", lines);
                    }
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
                            if let Ok(mut g) = st.lock() {
                                g.open_info_modal("logs (last 100)", lines);
                            }
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
                let arg = rest.trim().to_string();
                if arg.is_empty() {
                    let snapshot = self.runtime.snapshot();
                    let mut lines = vec![
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
                    if let ReplOutput::Tui(st) = &out
                        && let Ok(g) = st.lock()
                        && !g.subagents.is_empty()
                    {
                        lines.push(String::new());
                        lines.push("Sub-agents (use /attach <prefix> to inspect):".into());
                        for row in g.subagents.iter().take(16) {
                            let id8 = row.id.chars().take(8).collect::<String>();
                            let status = if row.running { "●" } else { "○" };
                            lines.push(format!(
                                "  {status} {id8}  {}  ({}↑ {}↓)",
                                row.task, row.tokens_in, row.tokens_out
                            ));
                        }
                    }
                    if let ReplOutput::Tui(st) = &out {
                        if let Ok(mut g) = st.lock() {
                            g.open_info_modal("attach", lines);
                        }
                    } else {
                        for l in &lines {
                            out.println(l);
                        }
                    }
                } else {
                    let sessions_dir = self.runtime.workspace_root().join(".nca").join("sessions");
                    let matched_id: Option<String> = if let ReplOutput::Tui(st) = &out {
                        st.lock().ok().and_then(|g| {
                            g.subagents
                                .iter()
                                .find(|row| row.id.starts_with(&arg))
                                .map(|row| row.id.clone())
                        })
                    } else {
                        None
                    };
                    let target_id = matched_id.unwrap_or_else(|| arg.clone());
                    let log_path = sessions_dir.join(format!("{target_id}.events.jsonl"));
                    let mut lines = vec![
                        format!("Sub-agent: {target_id}"),
                        format!("Events:    {}", log_path.display()),
                    ];
                    match std::fs::read_to_string(&log_path) {
                        Ok(body) => {
                            let tail: Vec<&str> = body.lines().rev().take(20).collect();
                            lines.push(String::new());
                            lines.push(format!("Last {} events:", tail.len()));
                            for l in tail.iter().rev() {
                                lines.push(l.chars().take(200).collect());
                            }
                        }
                        Err(err) => {
                            lines.push(format!("(cannot read log: {err})"));
                        }
                    }
                    if let ReplOutput::Tui(st) = &out {
                        if let Ok(mut g) = st.lock() {
                            g.open_info_modal("attach", lines);
                        }
                    } else {
                        for l in &lines {
                            out.println(l);
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
                            let n = if let Ok(mut g) = st.lock() {
                                g.staged_image_attachments.push(att);
                                g.staged_image_attachments.len()
                            } else {
                                0
                            };
                            out.println(&format!(
                                "[image] staged {path} — press Enter to send ({n} attached)"
                            ));
                        }
                        Err(e) => out.eprintln(&format!("[image] {e}")),
                    }
                } else if rest_trim.eq_ignore_ascii_case("clear") {
                    if let Ok(mut g) = st.lock() {
                        g.staged_image_attachments.clear();
                    }
                    out.println("[image] cleared staged images");
                } else {
                    let p = std::path::Path::new(rest_trim);
                    match crate::image_attach::import_image_file(&workspace, &sid, p) {
                        Ok(att) => {
                            let path = att.path.clone();
                            let n = if let Ok(mut g) = st.lock() {
                                g.staged_image_attachments.push(att);
                                g.staged_image_attachments.len()
                            } else {
                                0
                            };
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
                let mut lines = vec![
                    format!("Provider:    {}", config.provider.default.display_name()),
                    format!("Model:       {}", self.runtime.model()),
                    format!("Permission:  {:?}", self.runtime.permission_mode()),
                    format!("Memory:      {}", self.runtime.memory_store_path().display()),
                    format!("Editor:      {}", config.effective_editor_command()),
                    format!("Thinking:    {} (budget: {})", config.model.enable_thinking, config.model.thinking_budget),
                    format!("Max tokens:  {}", config.model.max_tokens),
                    String::new(),
                    "Provider endpoints:".into(),
                ];
                for provider in ProviderKind::ALL {
                    lines.push(format!(
                        "  {:<12} {}",
                        format!("{}:", provider.display_name()),
                        config.provider.base_url_for(provider)
                    ));
                }
                if let ReplOutput::Tui(st) = &out {
                    if let Ok(mut g) = st.lock() {
                        g.open_info_modal("config", lines);
                    }
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/connect" => {
                if let ReplOutput::Tui(st) = out {
                    if let Ok(mut g) = st.lock() {
                        g.open_connect_modal();
                    }
                    out.println(
                        "[connect] Choose a provider (↑↓ · Enter · type to search · Esc). Add key with /apikey if needed.",
                    );
                } else {
                    out.println("Connect an LLM provider (non-TUI):");
                    out.println("  /provider <minimax|openai|anthropic|openrouter|custom>");
                    out.println("  /custom <openai|anthropic> <base-url> [api-key] [model]");
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
                    "  /custom            Configure custom endpoint".into(),
                    "  /apikey <p> <key>  Store API key for a provider".into(),
                    "  /model [name]      Model for the active provider".into(),
                    "  /editor [seed]     Open external editor".into(),
                    "  /set-editor <cmd>  Persist editor command".into(),
                ];
                if let ReplOutput::Tui(st) = &out {
                    if let Ok(mut g) = st.lock() {
                        g.open_info_modal("settings", lines);
                    }
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
                        if let Ok(mut g) = st.lock() {
                            g.open_provider_picker(self.runtime.config().provider.default, false);
                        }
                        out.println("[provider] choose with ↑↓ + Enter, Esc cancel, c = custom API help");
                    } else {
                        out.println(&format!(
                            "current default provider: {} (model {})",
                            self.runtime.config().provider.default.display_name(),
                            self.runtime.model()
                        ));
                        out.println("usage: /provider <minimax|openai|anthropic|openrouter|custom>");
                        out.println("       /provider add-custom   (TUI: BYO endpoint wizard)");
                    }
                } else if rest.eq_ignore_ascii_case("add-custom")
                    || rest.eq_ignore_ascii_case("custom-wizard")
                {
                    if let ReplOutput::Tui(st) = out {
                        if let Ok(mut g) = st.lock() {
                            g.open_custom_provider_setup(self.runtime.model().to_string());
                        }
                        out.println("[provider] add custom provider wizard opened");
                    } else {
                        out.println("Wizard needs the full-screen TUI. Use:");
                        out.println("  /custom <openai|anthropic> <base-url> [api-key] [model]");
                    }
                } else if let Some(p) = ProviderKind::from_cli_name(rest)
                    .or_else(|| ProviderKind::parse_display_name(rest))
                {
                    self.apply_provider_in_session(p, out).await?;
                } else {
                    out.eprintln("unknown provider; try: minimax, openai, anthropic, openrouter, custom, add-custom");
                }
            }
            "/custom" => {
                let mut toks = rest.split_whitespace();
                let compat_raw = toks.next();
                let base_url = toks.next();
                let api_key = toks.next();
                let model = toks.next();
                if compat_raw.is_none() || base_url.is_none() {
                    let custom = &self.runtime.config().provider.custom;
                    let lines = vec![
                        format!(
                            "Compatibility: {}",
                            custom.compatibility.display_name()
                        ),
                        format!(
                            "Base URL:      {}",
                            if custom.base_url.trim().is_empty() {
                                "<not set>"
                            } else {
                                custom.base_url.as_str()
                            }
                        ),
                        format!("Model:         {}", custom.model),
                        format!("API key env:   {}", custom.api_key_env),
                        format!(
                            "API key:       {}",
                            if custom.resolve_api_key().is_some() {
                                "configured"
                            } else {
                                "missing"
                            }
                        ),
                        String::new(),
                        "usage: /custom <openai|anthropic> <base-url> [api-key] [model]".into(),
                        "TUI:   /provider → \"Add custom provider…\"".into(),
                        "example: /custom openai https://sumopod.example sk-test my-model".into(),
                    ];
                    if let ReplOutput::Tui(st) = &out {
                        if let Ok(mut g) = st.lock() {
                            g.open_info_modal("custom provider", lines);
                        }
                    } else {
                        for line in &lines {
                            out.println(line);
                        }
                    }
                    return Ok(true);
                }

                let Some(compatibility) =
                    ProviderCompatibility::from_cli_name(compat_raw.unwrap_or_default())
                else {
                    out.eprintln("compatibility must be `openai` or `anthropic`");
                    return Ok(true);
                };

                let base_url = base_url.unwrap_or_default().to_string();
                let api_key = api_key.map(|s| s.to_string());
                let model = model.map(|s| s.to_string());
                self.persist_custom_provider_config(compatibility, base_url, api_key, model, out)
                    .await?;
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
                                if let Ok(mut g) = st.lock() {
                                    g.open_api_key_modal(
                                        p,
                                        self.runtime.config().provider.api_key_present_for(p),
                                        false,
                                    );
                                }
                            } else {
                                out.println("usage: /apikey <provider> <secret>");
                            }
                        } else {
                            self.save_provider_api_key(p, key, out).await?;
                        }
                    } else {
                        out.eprintln("unknown provider; try: minimax, openai, anthropic, openrouter, custom");
                    }
                } else if let ReplOutput::Tui(st) = out {
                    if let Ok(mut g) = st.lock() {
                        g.open_provider_picker(self.runtime.config().provider.default, true);
                    }
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
                            if let Ok(mut g) = st.lock() {
                                g.input_buffer = text;
                                g.cursor_char_idx = g.input_buffer.chars().count();
                            }
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
                    if let Ok(mut g) = st.lock() {
                        g.open_info_modal("doctor", lines);
                    }
                } else {
                    for l in &lines {
                        out.println(l);
                    }
                }
            }
            "/auto-answer" => {
                let from_tui = if let ReplOutput::Tui(st) = &out {
                    st.lock()
                        .ok()
                        .and_then(|g| g.active_question.as_ref().map(|q| q.question_id.clone()))
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
            "/sessions" => match self.runtime.list_session_ids().await {
                Ok(mut ids) => {
                    ids.sort();
                    if ids.is_empty() {
                        let lines = vec!["No saved sessions.".into()];
                        if let ReplOutput::Tui(st) = &out {
                            if let Ok(mut g) = st.lock() {
                                g.open_info_modal("sessions", lines);
                            }
                        } else {
                            out.println("no saved sessions");
                        }
                    } else if let ReplOutput::Tui(st) = &out {
                        let current = self.runtime.session_id().to_string();
                        if let Ok(mut g) = st.lock() {
                            g.open_session_picker(ids, &current);
                        }
                    } else {
                        for id in ids {
                            out.println(&id);
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
                if let ReplOutput::Tui(st) = &out
                    && let Ok(mut g) = st.lock()
                {
                    g.blocks.clear();
                    g.streaming_assistant = None;
                    g.scroll_lines = 0;
                    g.transcript_follow_tail = true;
                    g.session_id = new_id.clone();
                    g.model = self.runtime.model().to_string();
                    g.input_tokens = 0;
                    g.output_tokens = 0;
                    g.cost_usd = 0.0;
                    g.started = std::time::Instant::now();
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
            _ => {
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

        if let Some(model) = &skill.model {
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
}
