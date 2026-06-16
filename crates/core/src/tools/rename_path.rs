use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::{WorkspaceFs, sandbox_error_to_tool_result};

pub struct RenamePathTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl RenamePathTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Params {
    from: String,
    to: String,
}

#[async_trait::async_trait]
impl ToolExecutor for RenamePathTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "rename_path".into(),
            description: "Rename a file or directory within the workspace".into(),
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
        let p: Params = match call.extract_params() {
            Ok(p) => p,
            Err(e) => return e,
        };
        match self.fs.rename(&p.from, &p.to).await {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!("Renamed {} -> {}", p.from, p.to),
                error: None,
            },
            Err(e) => sandbox_error_to_tool_result(&call.id, e),
        }
    }
}
