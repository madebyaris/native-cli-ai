//! Runtime-facing bash tool backed by a real PTY.

use crate::pty::PtyManager;
use nca_common::event::{AgentEvent, ToolOutputStream};
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use nca_core::tools::ToolExecutor;
use nca_core::tools::bash::BashTool;
use std::sync::Arc;
use tokio::sync::mpsc;

const MAX_CAPTURED_BYTES: usize = 64 * 1024;

/// Runtime bash tool using [`PtyManager`] for real PTY execution.
pub struct RuntimeBashTool {
    pty: Arc<PtyManager>,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    fallback: BashTool,
}

impl RuntimeBashTool {
    pub fn new(pty: Arc<PtyManager>) -> Self {
        let root = pty.workspace_root().to_path_buf();
        Self {
            pty,
            event_tx: None,
            fallback: BashTool::new(root),
        }
    }

    pub fn with_streaming(pty: Arc<PtyManager>, event_tx: mpsc::Sender<AgentEvent>) -> Self {
        let root = pty.workspace_root().to_path_buf();
        Self {
            pty,
            event_tx: Some(event_tx.clone()),
            fallback: BashTool::with_streaming(root, event_tx),
        }
    }

    fn parse_command(call: &ToolCall) -> Result<(String, u64), ToolResult> {
        let command = call
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if command.is_empty() {
            return Err(ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("missing required field: command".into()),
            });
        }
        let timeout_secs = call
            .input
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);
        Ok((command, timeout_secs))
    }
}

#[async_trait::async_trait]
impl ToolExecutor for RuntimeBashTool {
    fn definition(&self) -> ToolDefinition {
        self.fallback.definition()
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let (command, timeout_secs) = match Self::parse_command(call) {
            Ok(v) => v,
            Err(result) => return result,
        };

        if let Some(event_tx) = &self.event_tx {
            let pty = self.pty.clone();
            let event_tx = event_tx.clone();
            let call_id = call.id.clone();
            match pty
                .exec_with_lines(&command, timeout_secs, move |line| {
                    let _ = event_tx.try_send(AgentEvent::ToolOutputChunk {
                        call_id: call_id.clone(),
                        stream: ToolOutputStream::Stdout,
                        data: format!("{line}\n"),
                    });
                })
                .await
            {
                Ok(output) => ToolResult {
                    call_id: call.id.clone(),
                    success: output.exit_code == 0,
                    output: truncate_output(output.stdout),
                    error: if output.exit_code == 0 {
                        None
                    } else {
                        Some(format!("exit code {}", output.exit_code))
                    },
                },
                Err(err) => ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(err.to_string()),
                },
            }
        } else {
            match self.pty.exec(&command, timeout_secs).await {
                Ok(output) => ToolResult {
                    call_id: call.id.clone(),
                    success: output.exit_code == 0,
                    output: truncate_output(output.stdout),
                    error: if output.exit_code == 0 {
                        None
                    } else {
                        Some(format!("exit code {}", output.exit_code))
                    },
                },
                Err(err) => ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(err.to_string()),
                },
            }
        }
    }
}

fn truncate_output(mut text: String) -> String {
    if text.len() > MAX_CAPTURED_BYTES {
        text.truncate(MAX_CAPTURED_BYTES);
        text.push_str("\n… [output truncated]");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_command_requires_command_field() {
        let call = ToolCall {
            id: "c1".into(),
            name: "execute_bash".into(),
            input: json!({}),
        };
        assert!(RuntimeBashTool::parse_command(&call).is_err());
    }
}
