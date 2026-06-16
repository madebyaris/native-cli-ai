use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::{WorkspaceFs, sandbox_error_to_tool_result};

pub struct CopyPathTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl CopyPathTool {
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
        let p: Params = match call.extract_params() {
            Ok(p) => p,
            Err(e) => return e,
        };
        match self.fs.copy(&p.from, &p.to).await {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!("Copied {} -> {}", p.from, p.to),
                error: None,
            },
            Err(e) => sandbox_error_to_tool_result(&call.id, e),
        }
    }
}
