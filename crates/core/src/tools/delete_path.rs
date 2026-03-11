use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

pub struct DeletePathTool {
    workspace_root: std::path::PathBuf,
}

impl DeletePathTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for DeletePathTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "delete_path".into(),
            description: "Delete a file or directory within the workspace".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "recursive": { "type": "boolean" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let path = call.input["path"].as_str().unwrap_or("");
        let recursive = call.input["recursive"].as_bool().unwrap_or(false);
        let full_path = self.workspace_root.join(path);

        let canonical = match full_path.canonicalize() {
            Ok(path) if path.starts_with(&self.workspace_root) => path,
            _ => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some("Path is outside the workspace".into()),
                };
            }
        };

        let metadata = match tokio::fs::metadata(&canonical).await {
            Ok(metadata) => metadata,
            Err(err) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to stat path: {err}")),
                };
            }
        };

        let result = if metadata.is_dir() {
            if recursive {
                tokio::fs::remove_dir_all(&canonical).await
            } else {
                tokio::fs::remove_dir(&canonical).await
            }
        } else {
            tokio::fs::remove_file(&canonical).await
        };

        match result {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!("Deleted {}", canonical.display()),
                error: None,
            },
            Err(err) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to delete path: {err}")),
            },
        }
    }
}
