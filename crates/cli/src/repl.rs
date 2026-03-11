use crate::prompt::NcaPrompt;
use crate::runner::SessionRuntime;
use nca_common::config::PermissionMode;
use nca_common::event::EndReason;
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
            stdout
                .write_all(self.prompt.prompt_string().as_bytes())
                .await?;
            stdout.flush().await?;

            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }

            let input = line.trim();
            if input.is_empty() {
                continue;
            }

            if input.starts_with('/') {
                if !self.handle_command(input, &mut stdout).await? {
                    break;
                }
                continue;
            }

            match self.runtime.run_turn(input).await {
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

        match command {
            "/q" | "/quit" | "/exit" => return Ok(false),
            "/help" => {
                stdout
                    .write_all(
                        b"Run mode commands:\n\
                          /help                       Show this help\n\
                          /status                     Show current session status\n\
                          /model [name]               Get or set active model\n\
                          /permissions [mode]         Get or set permission mode\n\
                          /sessions                   List local session IDs\n\
                          /exit                       Exit repl\n",
                    )
                    .await?;
            }
            "/status" => {
                let status = format!(
                    "session={} model={} permission_mode={:?}\n",
                    self.runtime.session_id(),
                    self.runtime.model(),
                    self.runtime.permission_mode()
                );
                stdout.write_all(status.as_bytes()).await?;
            }
            "/model" => {
                if let Some(model) = parts.next() {
                    self.runtime.set_model(model.to_string());
                    stdout
                        .write_all(format!("model set to {model}\n").as_bytes())
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
                stdout
                    .write_all(format!("unknown command: {command}\n").as_bytes())
                    .await?;
            }
        }

        stdout.flush().await?;
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
