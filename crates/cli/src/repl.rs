use crate::prompt::NcaPrompt;
use crate::runner::SessionRuntime;
use nca_common::event::EndReason;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

/// The main REPL loop: read input, send to agent, render response.
pub struct Repl {
    runtime: SessionRuntime,
    prompt: NcaPrompt,
}

impl Repl {
    pub fn new(runtime: SessionRuntime, safe_mode: bool) -> Self {
        Self {
            runtime,
            prompt: NcaPrompt::new(safe_mode),
        }
    }

    /// Run the interactive REPL until the user exits.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut stdout = io::stdout();

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
            if matches!(input, "/q" | "/quit" | "/exit") {
                break;
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
}
