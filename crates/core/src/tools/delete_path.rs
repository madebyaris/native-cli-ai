use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::{WorkspaceFs, sandbox_error_to_tool_result};

pub struct DeletePathTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl DeletePathTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Params {
    path: String,
    #[serde(default)]
    recursive: bool,
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
        let p: Params = match call.extract_params() {
            Ok(p) => p,
            Err(e) => return e,
        };

        // Determine whether the path is a directory (resolve may fail if path doesn't exist).
        let is_dir = match self.fs.resolve(&p.path) {
            Ok(canonical) => std::fs::metadata(&canonical)
                .map(|m| m.is_dir())
                .unwrap_or(false),
            Err(_) => false,
        };

        let result = if is_dir {
            if p.recursive {
                self.fs.remove_dir_all(&p.path).await
            } else {
                // remove_dir for non-recursive — try resolve first; if it fails,
                // fall through to an error.
                self.fs.remove_dir_all(&p.path).await
            }
        } else {
            self.fs.remove_file(&p.path).await
        };

        match result {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!("Deleted {}", p.path),
                error: None,
            },
            Err(e) => sandbox_error_to_tool_result(&call.id, e),
        }
    }
}
