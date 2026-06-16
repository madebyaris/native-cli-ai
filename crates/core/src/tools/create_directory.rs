use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::{WorkspaceFs, sandbox_error_to_tool_result};

pub struct CreateDirectoryTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl CreateDirectoryTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Params {
    path: String,
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
        let p: Params = match call.extract_params() {
            Ok(p) => p,
            Err(e) => return e,
        };
        match self.fs.create_dir_all(&p.path).await {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!("Created directory {}", p.path),
                error: None,
            },
            Err(e) => sandbox_error_to_tool_result(&call.id, e),
        }
    }
}
