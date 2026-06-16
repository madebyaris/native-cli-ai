use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::{WorkspaceFs, sandbox_error_to_tool_result};

pub struct ListDirectoryTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl ListDirectoryTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Params {
    #[serde(default = "default_path")]
    path: String,
}

fn default_path() -> String {
    ".".into()
}

#[async_trait::async_trait]
impl ToolExecutor for ListDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_directory".into(),
            description: "List files and directories under a path".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under workspace. Defaults to '.'"
                    }
                }
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let p: Params = match call.extract_params() {
            Ok(p) => p,
            Err(e) => return e,
        };

        let entries = match self.fs.read_dir(&p.path).await {
            Ok(e) => e,
            Err(e) => return sandbox_error_to_tool_result(&call.id, e),
        };

        let max_entries = 1000;
        let truncated = entries.len() > max_entries;
        let shown = entries.len().min(max_entries);

        let mut lines = Vec::with_capacity(shown);
        for entry in &entries[..shown] {
            lines.push(if entry.is_dir {
                format!("{}/", entry.name)
            } else {
                entry.name.clone()
            });
        }
        if truncated {
            lines.push(format!(
                "… ({} more entries omitted; max {})",
                entries.len() - max_entries,
                max_entries
            ));
        }

        ToolResult {
            call_id: call.id.clone(),
            success: true,
            output: lines.join("\n"),
            error: None,
        }
    }
}
