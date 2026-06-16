use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::WorkspaceFs;

pub struct RunValidationTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl RunValidationTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Params {
    command: String,
    #[serde(default = "default_cwd")]
    cwd: String,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
}

fn default_cwd() -> String {
    ".".into()
}

fn default_timeout() -> u64 {
    120
}

#[async_trait::async_trait]
impl ToolExecutor for RunValidationTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "run_validation".into(),
            description: "Run a safe build, test, or lint command inside the workspace".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory relative to workspace root (default: '.')"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 120)"
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

        // Validate cwd stays within workspace.
        let cwd_abs = match self.fs.validate_prefix(&p.cwd) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(e.to_string()),
                };
            }
        };

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-lc")
            .arg(&p.command)
            .current_dir(&cwd_abs)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(p.timeout_secs),
            cmd.output(),
        )
        .await
        {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to execute command: {e}")),
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
