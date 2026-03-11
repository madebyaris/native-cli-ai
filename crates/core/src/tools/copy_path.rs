use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

pub struct CopyPathTool {
    workspace_root: std::path::PathBuf,
}

impl CopyPathTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for CopyPathTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "copy_path".into(),
            description: "Copy a file within the workspace".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "to": { "type": "string" }
                },
                "required": ["from", "to"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let from = call.input["from"].as_str().unwrap_or("");
        let to = call.input["to"].as_str().unwrap_or("");
        let from_path = self.workspace_root.join(from);
        let to_path = self.workspace_root.join(to);

        let canonical_from = match from_path.canonicalize() {
            Ok(path) if path.starts_with(&self.workspace_root) => path,
            _ => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some("Source path is outside the workspace".into()),
                };
            }
        };

        if let Some(parent) = to_path.parent() {
            if let Err(err) = tokio::fs::create_dir_all(parent).await {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to create destination directory: {err}")),
                };
            }
            match parent.canonicalize() {
                Ok(path) if path.starts_with(&self.workspace_root) => {}
                _ => {
                    return ToolResult {
                        call_id: call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some("Destination path is outside the workspace".into()),
                    };
                }
            }
        }

        match tokio::fs::copy(&canonical_from, &to_path).await {
            Ok(_) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!("Copied {} -> {}", canonical_from.display(), to_path.display()),
                error: None,
            },
            Err(err) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to copy path: {err}")),
            },
        }
    }
}
