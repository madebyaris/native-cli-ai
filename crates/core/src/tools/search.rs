use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

/// Code search tool that shells out to ripgrep.
pub struct SearchCodeTool {
    workspace_root: std::path::PathBuf,
}

impl SearchCodeTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for SearchCodeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "search_code".into(),
            description: "Search for a regex pattern in the codebase using ripgrep".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Optional file glob filter (e.g. '*.rs')"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let pattern = call.input["pattern"].as_str().unwrap_or("");
        let glob = call.input["glob"].as_str();

        let mut cmd = tokio::process::Command::new("rg");
        cmd.arg("--no-heading")
            .arg("--line-number")
            .arg("--color=never")
            .arg(pattern)
            .current_dir(&self.workspace_root);

        if let Some(g) = glob {
            cmd.arg("--glob").arg(g);
        }

        match cmd.output().await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                ToolResult {
                    call_id: call.id.clone(),
                    success: output.status.success(),
                    output: stdout,
                    error: None,
                }
            }
            Err(e) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to run ripgrep: {e}")),
            },
        }
    }
}
