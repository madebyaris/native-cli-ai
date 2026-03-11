use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::{ToolExecutor, rename_path::rename_impl};

pub struct MovePathTool {
    workspace_root: std::path::PathBuf,
}

impl MovePathTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for MovePathTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "move_path".into(),
            description: "Move a file or directory within the workspace".into(),
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
        rename_impl(&self.workspace_root, call, "moved").await
    }
}
