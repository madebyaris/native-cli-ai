use crate::pty::PtyManager;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use nca_core::tools::ToolExecutor;
use std::sync::Arc;

/// Runtime-backed bash tool that executes shell commands via PTY.
/// Lives in the runtime crate so the supervisor can register it
/// without depending on the CLI crate.
pub struct RuntimeBashTool {
    pty: Arc<PtyManager>,
}

impl RuntimeBashTool {
    pub fn new(pty: Arc<PtyManager>) -> Self {
        Self { pty }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for RuntimeBashTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "execute_bash".into(),
            description: "Execute a shell command in the workspace".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Command timeout in seconds (default: 30)"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let command = call.input["command"].as_str().unwrap_or("");
        let timeout_secs = call.input["timeout_secs"].as_u64().unwrap_or(30);

        match self.pty.exec(command, timeout_secs).await {
            Ok(out) => ToolResult {
                call_id: call.id.clone(),
                success: out.exit_code == 0,
                output: if out.stdout.is_empty() {
                    format!("Command exited with status {}", out.exit_code)
                } else {
                    out.stdout
                },
                error: None,
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
