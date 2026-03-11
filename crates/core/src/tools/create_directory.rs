use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

pub struct CreateDirectoryTool {
    workspace_root: std::path::PathBuf,
}

impl CreateDirectoryTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for CreateDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "create_directory".into(),
            description: "Create a directory inside the workspace".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let path = call.input["path"].as_str().unwrap_or("");
        let full_path = self.workspace_root.join(path);

        let parent = full_path.parent().unwrap_or(&self.workspace_root);
        let canonical_parent = match parent.canonicalize() {
            Ok(path) => path,
            Err(_) => self.workspace_root.clone(),
        };

        if !canonical_parent.starts_with(&self.workspace_root) {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("Path is outside the workspace".into()),
            };
        }

        match tokio::fs::create_dir_all(&full_path).await {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!("Created directory {}", full_path.display()),
                error: None,
            },
            Err(err) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to create directory: {err}")),
            },
        }
    }
}
