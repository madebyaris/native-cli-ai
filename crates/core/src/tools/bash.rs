use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use tokio::time::{Duration, timeout};

use super::ToolExecutor;

/// Executes shell commands inside the workspace.
pub struct BashTool {
    workspace_root: std::path::PathBuf,
}

impl BashTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
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

        ToolResult {
            call_id: call.id.clone(),
            success: output.status.success(),
            output: text,
            error: None,
        }
    }
}
