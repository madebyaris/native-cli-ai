use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;
use tokio::time::{Duration, timeout};

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::WorkspaceFs;

/// Executes shell commands inside the workspace.
pub struct BashTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl BashTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Params {
    command: String,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
}

fn default_timeout() -> u64 {
    30
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
        let p: Params = match call.extract_params() {
            Ok(p) => p,
            Err(e) => return e,
        };

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-lc")
            .arg(&p.command)
            .current_dir(self.fs.root())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = match timeout(Duration::from_secs(p.timeout_secs), cmd.output()).await {
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
                    error: Some(format!("Command timed out after {}s", p.timeout_secs)),
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

        ToolResult {
            call_id: call.id.clone(),
            success: output.status.success(),
            output: text,
            error: None,
        }
    }
}
