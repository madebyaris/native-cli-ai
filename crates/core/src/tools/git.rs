use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;
use crate::workspace_fs::WorkspaceFs;

pub struct GitStatusTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl GitStatusTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for GitStatusTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_status".into(),
            description: "Show git status for the current workspace".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let output = tokio::process::Command::new("git")
            .arg("status")
            .arg("--porcelain=v1")
            .current_dir(self.fs.root())
            .output()
            .await;

        match output {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                ToolResult {
                    call_id: call.id.clone(),
                    success: true,
                    output: if text.is_empty() {
                        "Working tree clean".into()
                    } else {
                        text
                    },
                    error: None,
                }
            }
            Err(e) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to run git status: {e}")),
            },
        }
    }
}

pub struct GitDiffTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl GitDiffTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for GitDiffTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_diff".into(),
            description: "Show git diff for the current workspace".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "staged": { "type": "boolean", "description": "Show staged changes only" }
                }
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let staged = call.input["staged"].as_bool().unwrap_or(false);
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("diff");
        if staged {
            cmd.arg("--staged");
        }
        cmd.current_dir(self.fs.root());

        let output = cmd.output().await;
        match output {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                ToolResult {
                    call_id: call.id.clone(),
                    success: true,
                    output: if text.is_empty() {
                        "No diff output".into()
                    } else {
                        text
                    },
                    error: None,
                }
            }
            Err(e) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to run git diff: {e}")),
            },
        }
    }
}
