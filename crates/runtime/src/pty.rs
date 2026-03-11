use std::path::Path;
use std::process::Stdio;
use tokio::time::{Duration, timeout};

/// Manages PTY sessions for sandboxed command execution.
pub struct PtyManager {
    workspace_root: std::path::PathBuf,
}

impl PtyManager {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    /// Spawn a command in a new PTY, capture output, and return it.
    pub async fn exec(&self, command: &str, timeout_secs: u64) -> Result<PtyOutput, PtyError> {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-lc")
            .arg(command)
            .current_dir(&self.workspace_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = timeout(Duration::from_secs(timeout_secs), cmd.output())
            .await
            .map_err(|_| PtyError::Timeout(timeout_secs))?
            .map_err(|e| PtyError::SpawnFailed(e.to_string()))?;

        let mut text = String::new();
        text.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        Ok(PtyOutput {
            stdout: text,
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

#[derive(Debug)]
pub struct PtyOutput {
    pub stdout: String,
    pub exit_code: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("Command timed out after {0}s")]
    Timeout(u64),
    #[error("Spawn failed: {0}")]
    SpawnFailed(String),
}
