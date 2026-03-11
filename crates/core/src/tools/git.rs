use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

pub struct GitStatusTool {
    workspace_root: std::path::PathBuf,
}

impl GitStatusTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
}

pub struct GitDiffTool {
    workspace_root: std::path::PathBuf,
}

impl GitDiffTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
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
        run_git(&self.workspace_root, &["status", "--short", "--branch"], call).await
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
                    "staged": { "type": "boolean" }
                }
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let staged = call.input["staged"].as_bool().unwrap_or(false);
        let args = if staged {
            vec!["diff", "--cached", "--no-color"]
        } else {
            vec!["diff", "--no-color"]
        };
        run_git(&self.workspace_root, &args, call).await
    }
}

async fn run_git(
    workspace_root: &std::path::PathBuf,
    args: &[&str],
    call: &ToolCall,
) -> ToolResult {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(workspace_root)
        .output()
        .await;

    match output {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            if !out.stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            ToolResult {
                call_id: call.id.clone(),
                success: out.status.success(),
                output: text,
                error: None,
            }
        }
        Err(err) => ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: String::new(),
            error: Some(format!("Failed to run git: {err}")),
        },
    }
}
