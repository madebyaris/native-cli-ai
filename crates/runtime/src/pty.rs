use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use tokio::time::{Duration, timeout};

/// Manages PTY sessions for sandboxed command execution.
pub struct PtyManager {
    workspace_root: Mutex<std::path::PathBuf>,
}

impl PtyManager {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: Mutex::new(workspace_root.as_ref().to_path_buf()),
        }
    }

    pub fn workspace_root(&self) -> std::path::PathBuf {
        self.workspace_root
            .lock()
            .expect("workspace_root lock poisoned")
            .clone()
    }

    /// Update the workspace root for this PTY manager.
    /// All subsequent shell commands will use the new root as their working directory.
    pub fn set_root(&self, path: &Path) {
        *self
            .workspace_root
            .lock()
            .expect("workspace_root lock poisoned") = path.to_path_buf();
    }

    /// Spawn a command in a new PTY, capture output, and return it.
    pub async fn exec(&self, command: &str, timeout_secs: u64) -> Result<PtyOutput, PtyError> {
        let root = self
            .workspace_root
            .lock()
            .expect("workspace_root lock poisoned")
            .clone();
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-lc")
            .arg(command)
            .current_dir(&root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| PtyError::SpawnFailed(e.to_string()))?;

        let status = match timeout(Duration::from_secs(timeout_secs), child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => {
                // Child exited with an error; ensure it's reaped.
                std::mem::drop(child.kill());
                return Err(PtyError::SpawnFailed(e.to_string()));
            }
            Err(_) => {
                // Timeout — kill the child process.
                std::mem::drop(child.kill());
                std::mem::drop(child.wait());
                return Err(PtyError::Timeout(timeout_secs));
            }
        };

        // Collect stdout/stderr from the piped handles.
        let mut stdout = String::new();
        let mut stderr = String::new();
        if let Some(mut out) = child.stdout.take() {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf).await;
            stdout = String::from_utf8_lossy(&buf).into_owned();
        }
        if let Some(mut err) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf).await;
            stderr = String::from_utf8_lossy(&buf).into_owned();
        }

        if !stderr.is_empty() && !stdout.is_empty() {
            stdout.push('\n');
            stdout.push_str(&stderr);
        } else if !stderr.is_empty() {
            stdout = stderr;
        }

        Ok(PtyOutput {
            stdout,
            exit_code: status.code().unwrap_or(-1),
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
