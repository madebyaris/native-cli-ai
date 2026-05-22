//! Shell command executor with optional live output streaming.
//!
//! When `event_tx` is `Some`, stdout and stderr are tailed line-by-line (on a
//! modest buffer) and broadcast as `AgentEvent::ToolOutputChunk`, giving the
//! TUI a live pane while long-running commands execute. The final
//! `ToolResult` still carries the complete captured output so anything that
//! only consumes the summary (NDJSON, log replay) continues to work.

use std::sync::Arc;
use std::sync::Mutex;

use nca_common::event::{AgentEvent, ToolOutputStream};
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, timeout};

use super::ToolExecutor;

/// Maximum bytes of stdout+stderr kept in the final `ToolResult.output`. Any
/// overflow is replaced with a truncation notice so we don't blow up the LLM
/// context window with megabytes of build logs.
const MAX_CAPTURED_BYTES: usize = 64 * 1024;

/// Executes shell commands inside the workspace.
pub struct BashTool {
    workspace_root: std::path::PathBuf,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
}

impl BashTool {
    /// Batch-only bash tool; preserves the legacy contract where stdout/stderr
    /// are returned once at completion.
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self {
            workspace_root,
            event_tx: None,
        }
    }

    /// Bash tool that fans out live output via `event_tx` in addition to the
    /// final `ToolResult`. Callers should check the session config before
    /// choosing this constructor.
    pub fn with_streaming(
        workspace_root: std::path::PathBuf,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Self {
        Self {
            workspace_root,
            event_tx: Some(event_tx),
        }
    }

    async fn run_streaming(&self, call: &ToolCall, command: &str, timeout_secs: u64) -> ToolResult {
        use tokio::process::Command;

        let event_tx = self
            .event_tx
            .clone()
            .expect("streaming bash tool constructed without event_tx");

        let mut cmd = Command::new("sh");
        cmd.arg("-lc")
            .arg(command)
            .current_dir(&self.workspace_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to spawn bash command: {e}")),
                };
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let captured = Arc::new(Mutex::new(Captured::default()));

        let stdout_handle = stdout.map(|pipe| {
            spawn_pipe_reader(
                pipe,
                ToolOutputStream::Stdout,
                call.id.clone(),
                event_tx.clone(),
                captured.clone(),
            )
        });
        let stderr_handle = stderr.map(|pipe| {
            spawn_pipe_reader(
                pipe,
                ToolOutputStream::Stderr,
                call.id.clone(),
                event_tx.clone(),
                captured.clone(),
            )
        });

        let start = Instant::now();
        let wait_result = timeout(Duration::from_secs(timeout_secs), child.wait()).await;

        if let Some(h) = stdout_handle {
            let _ = h.await;
        }
        if let Some(h) = stderr_handle {
            let _ = h.await;
        }

        let status = match wait_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: final_text(&captured),
                    error: Some(format!("Bash command I/O error: {e}")),
                };
            }
            Err(_) => {
                let _ = child.start_kill();
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: final_text(&captured),
                    error: Some(format!(
                        "Command timed out after {timeout_secs}s (elapsed {:?})",
                        start.elapsed()
                    )),
                };
            }
        };

        ToolResult {
            call_id: call.id.clone(),
            success: status.success(),
            output: final_text(&captured),
            error: None,
        }
    }

    async fn run_batch(&self, call: &ToolCall, command: &str, timeout_secs: u64) -> ToolResult {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-lc")
            .arg(command)
            .current_dir(&self.workspace_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = match timeout(Duration::from_secs(timeout_secs), cmd.output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to execute bash command: {e}")),
                };
            }
            Err(_) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Command timed out after {timeout_secs}s")),
                };
            }
        };

        let mut text = String::new();
        text.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        truncate_captured(&mut text);

        ToolResult {
            call_id: call.id.clone(),
            success: output.status.success(),
            output: text,
            error: None,
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for BashTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "execute_bash".into(),
            description: "Execute a shell command in the workspace".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30)"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let command = call.input["command"].as_str().unwrap_or("");
        let timeout_secs = call.input["timeout_secs"].as_u64().unwrap_or(30);

        if self.event_tx.is_some() {
            self.run_streaming(call, command, timeout_secs).await
        } else {
            self.run_batch(call, command, timeout_secs).await
        }
    }
}

#[derive(Default)]
struct Captured {
    stdout: String,
    stderr: String,
    truncated: bool,
}

fn spawn_pipe_reader<R>(
    pipe: R,
    stream: ToolOutputStream,
    call_id: String,
    event_tx: mpsc::Sender<AgentEvent>,
    captured: Arc<Mutex<Captured>>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = BufReader::new(pipe);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    {
                        let mut guard = match captured.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        append_captured(&mut guard, stream, &line);
                    }
                    let chunk = line.clone();
                    if event_tx
                        .send(AgentEvent::ToolOutputChunk {
                            call_id: call_id.clone(),
                            stream,
                            data: chunk,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn append_captured(captured: &mut Captured, stream: ToolOutputStream, data: &str) {
    let remaining = MAX_CAPTURED_BYTES.saturating_sub(captured_total_len(captured));
    if remaining == 0 {
        captured.truncated = true;
        return;
    }
    let buf = match stream {
        ToolOutputStream::Stdout => &mut captured.stdout,
        ToolOutputStream::Stderr => &mut captured.stderr,
    };
    if data.len() <= remaining {
        buf.push_str(data);
    } else {
        let mut cut = remaining;
        while cut > 0 && !data.is_char_boundary(cut) {
            cut -= 1;
        }
        buf.push_str(&data[..cut]);
        captured.truncated = true;
    }
}

fn captured_total_len(captured: &Captured) -> usize {
    captured.stdout.len() + captured.stderr.len()
}

fn final_text(captured: &Arc<Mutex<Captured>>) -> String {
    let guard = match captured.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let mut text = String::with_capacity(guard.stdout.len() + guard.stderr.len() + 32);
    text.push_str(&guard.stdout);
    if !guard.stderr.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&guard.stderr);
    }
    if guard.truncated {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("... [output truncated at 64KiB]");
    }
    text
}

fn truncate_captured(text: &mut String) {
    if text.len() > MAX_CAPTURED_BYTES {
        let mut cut = MAX_CAPTURED_BYTES;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n... [output truncated at 64KiB]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn streaming_bash_emits_chunks_and_completes() {
        let tmp = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let tool = BashTool::with_streaming(tmp.path().into(), tx);
        let res = tool
            .execute(&ToolCall {
                id: "call-1".into(),
                name: "execute_bash".into(),
                input: serde_json::json!({
                    "command": "printf 'one\\ntwo\\nthree\\n'",
                    "timeout_secs": 5
                }),
            })
            .await;
        assert!(res.success, "{:?}", res.error);
        assert!(res.output.contains("one"));
        assert!(res.output.contains("three"));

        let mut chunks = Vec::new();
        while let Ok(ev) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            match ev {
                Some(AgentEvent::ToolOutputChunk {
                    call_id,
                    stream,
                    data,
                }) => {
                    assert_eq!(call_id, "call-1");
                    chunks.push((stream, data));
                }
                Some(_) => {}
                None => break,
            }
        }
        assert!(
            !chunks.is_empty(),
            "expected ToolOutputChunk events, got none"
        );
    }

    #[tokio::test]
    async fn batch_bash_preserves_legacy_contract() {
        let tmp = tempdir().unwrap();
        let tool = BashTool::new(tmp.path().into());
        let res = tool
            .execute(&ToolCall {
                id: "call-1".into(),
                name: "execute_bash".into(),
                input: serde_json::json!({
                    "command": "echo hello-batch",
                    "timeout_secs": 5
                }),
            })
            .await;
        assert!(res.success);
        assert!(res.output.contains("hello-batch"));
    }

    #[tokio::test]
    async fn streaming_bash_reports_nonzero_exit() {
        let tmp = tempdir().unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let tool = BashTool::with_streaming(tmp.path().into(), tx);
        let res = tool
            .execute(&ToolCall {
                id: "call-2".into(),
                name: "execute_bash".into(),
                input: serde_json::json!({
                    "command": "false",
                    "timeout_secs": 5
                }),
            })
            .await;
        assert!(!res.success);
    }
}
