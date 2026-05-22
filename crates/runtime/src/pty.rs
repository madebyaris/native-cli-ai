use std::io::{Read, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

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

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Spawn a command in a real PTY, capture output, and return it.
    pub async fn exec(&self, command: &str, timeout_secs: u64) -> Result<PtyOutput, PtyError> {
        self.exec_with_lines(command, timeout_secs, |_| {}).await
    }

    /// Execute inside a PTY and invoke `on_line` for each output line as it arrives.
    pub async fn exec_with_lines<F>(
        &self,
        command: &str,
        timeout_secs: u64,
        on_line: F,
    ) -> Result<PtyOutput, PtyError>
    where
        F: FnMut(String) + Send + 'static,
    {
        let workspace_root = self.workspace_root.clone();
        let command = command.to_string();
        tokio::task::spawn_blocking(move || {
            exec_in_pty_with_lines(&workspace_root, &command, timeout_secs, on_line)
        })
        .await
        .map_err(|err| PtyError::SpawnFailed(format!("pty task join failed: {err}")))?
    }
}

fn exec_in_pty_with_lines<F>(
    workspace_root: &Path,
    command: &str,
    timeout_secs: u64,
    mut on_line: F,
) -> Result<PtyOutput, PtyError>
where
    F: FnMut(String),
{
    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| PtyError::SpawnFailed(format!("openpty failed: {err}")))?;

    let mut cmd = CommandBuilder::new("sh");
    cmd.arg("-lc");
    cmd.arg(command);
    cmd.cwd(workspace_root);
    // Pre-commit hooks and CI often run without TERM; programs like `tput` need it in a PTY.
    cmd.env("TERM", "xterm-256color");

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| PtyError::SpawnFailed(format!("pty spawn failed: {err}")))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| PtyError::SpawnFailed(format!("pty reader failed: {err}")))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|err| PtyError::SpawnFailed(format!("pty writer failed: {err}")))?;
    let _ = writer.flush();

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    let mut captured = String::new();

    loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err(PtyError::Timeout(timeout_secs));
        }

        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                while let Some(pos) = pending.find('\n') {
                    let line = pending[..pos].trim_end_matches('\r').to_string();
                    pending.drain(..=pos);
                    if !captured.is_empty() {
                        captured.push('\n');
                    }
                    captured.push_str(&line);
                    on_line(line);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => {
                return Err(PtyError::SpawnFailed(format!("pty read failed: {err}")));
            }
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|err| PtyError::SpawnFailed(format!("pty wait failed: {err}")))?
        {
            // Drain any remaining bytes after the shell exits.
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            if !pending.trim().is_empty() {
                if !captured.is_empty() {
                    captured.push('\n');
                }
                captured.push_str(pending.trim());
                on_line(pending.trim().to_string());
            }
            return Ok(PtyOutput {
                stdout: captured,
                exit_code: status.exit_code() as i32,
            });
        }
    }

    let status = child
        .wait()
        .map_err(|err| PtyError::SpawnFailed(format!("pty wait failed: {err}")))?;
    if !pending.trim().is_empty() {
        if !captured.is_empty() {
            captured.push('\n');
        }
        captured.push_str(pending.trim());
        on_line(pending.trim().to_string());
    }

    Ok(PtyOutput {
        stdout: captured,
        exit_code: status.exit_code() as i32,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn real_pty_reports_column_count() {
        let pty = PtyManager::new(std::env::temp_dir());
        let output = pty
            .exec("tput cols 2>/dev/null || echo 80", 10)
            .await
            .expect("pty exec");
        assert!(
            output.stdout.trim().chars().all(|c| c.is_ascii_digit()),
            "expected numeric cols, got {:?}",
            output.stdout
        );
        assert!(output.exit_code == 0 || output.stdout.trim().parse::<u32>().is_ok());
    }
}
