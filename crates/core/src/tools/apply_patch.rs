use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::{WorkspaceFs, sandbox_error_to_tool_result};

pub struct ApplyPatchTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl ApplyPatchTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Params {
    path: String,
    edits: Vec<PatchEdit>,
}

#[derive(Deserialize)]
struct PatchEdit {
    old_text: String,
    new_text: String,
}

#[async_trait::async_trait]
impl ToolExecutor for ApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "apply_patch".into(),
            description: "Apply one or more exact string replacements to a file".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_text": { "type": "string" },
                                "new_text": { "type": "string" }
                            },
                            "required": ["old_text", "new_text"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let p: Params = match call.extract_params() {
            Ok(p) => p,
            Err(e) => return e,
        };

        if p.edits.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("edits array must not be empty".into()),
            };
        }

        let mut content = match self.fs.read_file(&p.path).await {
            Ok(c) => c,
            Err(e) => return sandbox_error_to_tool_result(&call.id, e),
        };

        let mut applied = 0_usize;
        let mut errors = Vec::new();

        for (i, edit) in p.edits.iter().enumerate() {
            let count = content.matches(&edit.old_text).count();
            match count {
                0 => errors.push(format!("edit {}: old_text not found", i + 1)),
                1 => {
                    if let Some(idx) = content.find(&edit.old_text) {
                        content.replace_range(idx..idx + edit.old_text.len(), &edit.new_text);
                        applied += 1;
                    }
                }
                n => errors.push(format!(
                    "edit {}: old_text matched {n} times (ambiguous)",
                    i + 1
                )),
            }
        }

        let error_msg = if errors.is_empty() {
            None
        } else {
            Some(errors.join("; "))
        };

        match self.fs.write_file(&p.path, &content).await {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: errors.is_empty(),
                output: format!(
                    "Applied {applied}/{} edits to {path}",
                    p.edits.len(),
                    path = p.path
                ),
                error: error_msg,
            },
            Err(e) => sandbox_error_to_tool_result(&call.id, e),
        }
    }
}
