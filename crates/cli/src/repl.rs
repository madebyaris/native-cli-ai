use crate::prompt::NcaPrompt;
use crate::runner::SessionRuntime;
use nca_common::config::PermissionMode;
use nca_common::event::EndReason;
use nca_core::skills::SkillCatalog;
use nca_runtime::memory_store::MemoryStore;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

/// The main REPL loop: read input, send to agent, render response.
pub struct Repl {
    runtime: SessionRuntime,
    prompt: NcaPrompt,
    run_mode: bool,
}

impl Repl {
    pub fn new(runtime: SessionRuntime, safe_mode: bool, run_mode: bool) -> Self {
        Self {
            runtime,
            prompt: NcaPrompt::new(safe_mode, run_mode),
            run_mode,
        }
    }

    /// Run the interactive REPL until the user exits.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut stdout = io::stdout();

        if self.run_mode {
            stdout
                .write_all(b"[run-mode] Type /help for commands. Use /exit to quit.\n")
                .await?;
            stdout.flush().await?;
        }

        loop {
            let Some(input) = self.read_input(&mut reader, &mut stdout).await? else {
                break;
            };
            if input.is_empty() {
                continue;
            }

            if input.starts_with('/') {
                if !self.handle_command(&input, &mut stdout).await? {
                    break;
                }
                continue;
            }

            match self.runtime.run_turn(&input).await {
                Ok(output) => {
                    stdout.write_all(output.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                }
                Err(err) => {
                    stdout
                        .write_all(format!("error: {err}\n").as_bytes())
                        .await?;
                }
            }
            stdout.flush().await?;
        }

        self.runtime.finish(EndReason::UserExit).await;
        Ok(())
    }

    async fn handle_command(
        &mut self,
        input: &str,
        stdout: &mut tokio::io::Stdout,
    ) -> anyhow::Result<bool> {
        let mut parts = input.split_whitespace();
        let command = parts.next().unwrap_or_default();
        let rest = input
            .strip_prefix(command)
            .map(str::trim)
            .unwrap_or_default();

        match command {
            "/q" | "/quit" | "/exit" => return Ok(false),
            "/help" => {
                stdout
                    .write_all(
                        b"Run mode commands:\n\
                          /help                       Show this help\n\
                          /status                     Show current session status\n\
                          /plan <task>                Run a planning-oriented turn\n\
                          /review <task>              Review code or changes\n\
                          /fix <task>                 Run a bug-fix oriented turn\n\
                          /test <task>                Ask the agent to validate/test\n\
                          /skills                     List discovered skills\n\
                          /memory [text]              Show or store workspace memory\n\
                          /compact                    Save a compact session summary\n\
                          /models                     Show MiniMax aliases and defaults\n\
                          /mcp                        List configured MCP servers\n\
                          /agents                     Show child sessions\n\
                          /logs                       Print the current event log\n\
                          /attach                     Show current attach target\n\
                          /config                     Show effective runtime config\n\
                          /doctor                     Run MiniMax config checks\n\
                          /model [name]               Get or set active model\n\
                          /permissions [mode]         Get or set permission mode\n\
                          /sessions                   List local session IDs\n\
                          /exit                       Exit repl\n",
                    )
                    .await?;
            }
            "/status" => {
                let snapshot = self.runtime.snapshot();
                let status = format!(
                    "session={} model={} permission_mode={:?} children={} memory={}\n",
                    snapshot.id,
                    self.runtime.model(),
                    self.runtime.permission_mode(),
                    snapshot.child_session_ids.len(),
                    self.runtime.memory_store_path().display()
                );
                stdout.write_all(status.as_bytes()).await?;
                if let Some(summary) = snapshot.session_summary {
                    stdout
                        .write_all(format!("summary: {}\n", summary.replace('\n', " ")).as_bytes())
                        .await?;
                }
            }
            "/plan" => self
                .run_preset(
                    "Create a short implementation plan before coding. Focus on steps, risks, and validation.\n\nTask:\n",
                    rest,
                    stdout,
                )
                .await?,
            "/review" => self
                .run_preset(
                    "Review the requested code or changes. Prioritize bugs, regressions, risks, and missing tests.\n\nReview target:\n",
                    rest,
                    stdout,
                )
                .await?,
            "/fix" => self
                .run_preset(
                    "Diagnose and fix the issue below. Prefer a minimal verified change.\n\nIssue:\n",
                    rest,
                    stdout,
                )
                .await?,
            "/test" => self
                .run_preset(
                    "Validate the requested area. Run tests or checks if tools allow, and report what passed or failed.\n\nTarget:\n",
                    rest,
                    stdout,
                )
                .await?,
            "/model" => {
                if let Some(model) = parts.next() {
                    let resolved = self.runtime.config().model.resolve_alias(model);
                    self.runtime.set_model(resolved.clone());
                    stdout
                        .write_all(format!("model set to {resolved}\n").as_bytes())
                        .await?;
                } else {
                    stdout
                        .write_all(format!("model: {}\n", self.runtime.model()).as_bytes())
                        .await?;
                }
            }
            "/permissions" => {
                if let Some(mode) = parts.next() {
                    if let Some(parsed_mode) = parse_permission_mode(mode) {
                        self.runtime.set_permission_mode(parsed_mode);
                        stdout
                            .write_all(
                                format!("permission mode set to {parsed_mode:?}\n").as_bytes(),
                            )
                            .await?;
                    } else {
                        stdout
                            .write_all(
                                b"invalid mode; expected one of: default, plan, accept-edits, dont-ask, bypass-permissions\n",
                            )
                            .await?;
                    }
                } else {
                    stdout
                        .write_all(
                            format!("permission_mode: {:?}\n", self.runtime.permission_mode())
                                .as_bytes(),
                        )
                        .await?;
                }
            }
            "/skills" => {
                let skills = SkillCatalog::discover(
                    self.runtime.workspace_root(),
                    &self.runtime.config().harness.skill_directories,
                )
                .map_err(anyhow::Error::msg)?;
                if skills.is_empty() {
                    stdout.write_all(b"no skills discovered\n").await?;
                } else {
                    for skill in skills {
                        stdout
                            .write_all(format!("{}\n", skill.summary_line()).as_bytes())
                            .await?;
                    }
                }
            }
            "/memory" => {
                if rest.is_empty() {
                    let store = MemoryStore::new(self.runtime.memory_store_path());
                    let state = store.load().await.map_err(anyhow::Error::msg)?;
                    if state.notes.is_empty() {
                        stdout.write_all(b"no memory notes stored\n").await?;
                    } else {
                        for note in state.notes.iter().rev().take(5) {
                            stdout
                                .write_all(
                                    format!(
                                        "{} {} {}\n",
                                        note.id,
                                        note.kind,
                                        note.content.replace('\n', " ")
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                        }
                    }
                } else {
                    self.runtime
                        .append_memory_note("note", Some(rest.to_string()))
                        .await
                        .map_err(anyhow::Error::msg)?;
                    stdout.write_all(b"memory note saved\n").await?;
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
                stdout
                    .write_all(format!("saved session summary:\n{}\n", summary).as_bytes())
                    .await?;
            }
            "/models" => {
                stdout
                    .write_all(
                        format!(
                            "default={} thinking={} budget={}\n",
                            self.runtime.config().model.default_model,
                            self.runtime.config().model.enable_thinking,
                            self.runtime.config().model.thinking_budget
                        )
                        .as_bytes(),
                    )
                    .await?;
                for (alias, target) in &self.runtime.config().model.aliases {
                    stdout
                        .write_all(format!("  {alias} -> {target}\n").as_bytes())
                        .await?;
                }
            }
            "/mcp" => {
                if self.runtime.config().mcp.servers.is_empty() {
                    stdout.write_all(b"no MCP servers configured\n").await?;
                } else {
                    for server in self.runtime.config().mcp.servers.iter().filter(|server| server.enabled) {
                        stdout
                            .write_all(
                                format!(
                                    "{} command={} {}\n",
                                    server.name,
                                    server.command,
                                    server.args.join(" ")
                                )
                                .as_bytes(),
                            )
                            .await?;
                    }
                }
            }
            "/agents" => {
                let snapshot = self.runtime.snapshot();
                if snapshot.child_session_ids.is_empty() {
                    stdout.write_all(b"no child sessions yet\n").await?;
                } else {
                    for child in snapshot.child_session_ids {
                        stdout.write_all(format!("{child}\n").as_bytes()).await?;
                    }
                }
            }
            "/logs" => {
                match tokio::fs::read_to_string(self.runtime.event_log_path()).await {
                    Ok(data) => stdout.write_all(data.as_bytes()).await?,
                    Err(err) => {
                        stdout
                            .write_all(format!("failed to read log: {err}\n").as_bytes())
                            .await?
                    }
                }
            }
            "/attach" => {
                let snapshot = self.runtime.snapshot();
                stdout
                    .write_all(
                        format!(
                            "session={} socket={}\n",
                            snapshot.id,
                            snapshot
                                .socket_path
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "<none>".into())
                        )
                        .as_bytes(),
                    )
                    .await?;
            }
            "/config" => {
                let config = self.runtime.config();
                stdout
                    .write_all(
                        format!(
                            "provider={:?} model={} permission_mode={:?} memory={}\n",
                            config.provider.default,
                            self.runtime.model(),
                            self.runtime.permission_mode(),
                            self.runtime.memory_store_path().display()
                        )
                        .as_bytes(),
                    )
                    .await?;
            }
            "/doctor" => {
                let configured = self.runtime.config().provider.minimax.resolve_api_key().is_some();
                let message = if configured {
                    "MiniMax API key configured\n"
                } else {
                    "MiniMax API key missing\n"
                };
                stdout.write_all(message.as_bytes()).await?;
            }
            "/sessions" => match self.runtime.list_session_ids().await {
                Ok(mut ids) => {
                    ids.sort();
                    if ids.is_empty() {
                        stdout.write_all(b"no saved sessions\n").await?;
                    } else {
                        for id in ids {
                            stdout.write_all(format!("{id}\n").as_bytes()).await?;
                        }
                    }
                }
                Err(error) => {
                    stdout
                        .write_all(format!("failed to list sessions: {error}\n").as_bytes())
                        .await?;
                }
            },
            _ => {
                if command.starts_with('/') {
                    if self.try_run_skill(command.trim_start_matches('/'), rest, stdout).await? {
                        stdout.flush().await?;
                        return Ok(true);
                    }
                }
                stdout
                    .write_all(format!("unknown command: {command}\n").as_bytes())
                    .await?;
            }
        }

        stdout.flush().await?;
        Ok(true)
    }

    async fn read_input(
        &self,
        reader: &mut BufReader<io::Stdin>,
        stdout: &mut tokio::io::Stdout,
    ) -> anyhow::Result<Option<String>> {
        stdout
            .write_all(self.prompt.prompt_string().as_bytes())
            .await?;
        stdout.flush().await?;

        let mut chunks = Vec::new();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None);
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if let Some(prefix) = trimmed.strip_suffix('\\') {
                chunks.push(prefix.to_string());
                stdout
                    .write_all(self.prompt.multiline_prompt_string().as_bytes())
                    .await?;
                stdout.flush().await?;
                continue;
            }
            chunks.push(trimmed.to_string());
            break;
        }
        Ok(Some(chunks.join("\n").trim().to_string()))
    }

    async fn run_preset(
        &mut self,
        prefix: &str,
        task: &str,
        stdout: &mut tokio::io::Stdout,
    ) -> anyhow::Result<()> {
        if task.trim().is_empty() {
            stdout.write_all(b"usage requires a task\n").await?;
            return Ok(());
        }
        let prompt = format!("{prefix}{}", task.trim());
        match self.runtime.run_turn(&prompt).await {
            Ok(output) => {
                stdout.write_all(output.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
            }
            Err(err) => {
                stdout
                    .write_all(format!("error: {err}\n").as_bytes())
                    .await?;
            }
        }
        Ok(())
    }

    async fn try_run_skill(
        &mut self,
        skill_name: &str,
        task: &str,
        stdout: &mut tokio::io::Stdout,
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
        match self.runtime.run_turn(&prompt).await {
            Ok(output) => {
                stdout.write_all(output.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
            }
            Err(err) => {
                stdout
                    .write_all(format!("error: {err}\n").as_bytes())
                    .await?;
            }
        }
        Ok(true)
    }
}

fn parse_permission_mode(raw: &str) -> Option<PermissionMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "default" => Some(PermissionMode::Default),
        "plan" => Some(PermissionMode::Plan),
        "accept-edits" | "accept_edits" | "acceptedits" => Some(PermissionMode::AcceptEdits),
        "dont-ask" | "dont_ask" | "dontask" => Some(PermissionMode::DontAsk),
        "bypass-permissions" | "bypass_permissions" | "bypasspermissions" => {
            Some(PermissionMode::BypassPermissions)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_permission_aliases() {
        assert_eq!(
            parse_permission_mode("accept-edits"),
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            parse_permission_mode("dontask"),
            Some(PermissionMode::DontAsk)
        );
        assert_eq!(
            parse_permission_mode("bypass_permissions"),
            Some(PermissionMode::BypassPermissions)
        );
        assert_eq!(parse_permission_mode("invalid"), None);
    }
}
