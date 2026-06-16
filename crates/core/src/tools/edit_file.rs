use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::{WorkspaceFs, sandbox_error_to_tool_result};

pub struct EditFileTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl EditFileTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Params {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait::async_trait]
impl ToolExecutor for EditFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "edit_file".into(),
            description: "Replace a specific string in an existing file".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" },
                    "replace_all": { "type": "boolean" }
                },
                "required": ["path", "old_text", "new_text"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let p: Params = match call.extract_params() {
            Ok(p) => p,
            Err(e) => return e,
        };

        if p.old_text.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("old_text must not be empty".into()),
            };
        }

        let content = match self.fs.read_file(&p.path).await {
            Ok(c) => c,
            Err(e) => return sandbox_error_to_tool_result(&call.id, e),
        };

        let occurrence_count = content.matches(&p.old_text).count();
        if occurrence_count == 0 {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("old_text was not found".into()),
            };
        }

        let updated = if p.replace_all {
            content.replace(&p.old_text, &p.new_text)
        } else if occurrence_count > 1 {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!(
                    "old_text matched {occurrence_count} occurrences; use replace_all or replace_match for a precise edit"
                )),
            };
        } else if let Some(index) = content.find(&p.old_text) {
            let mut updated = content.clone();
            updated.replace_range(index..index + p.old_text.len(), &p.new_text);
            updated
        } else {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("old_text was not found".into()),
            };
        };

        match self.fs.write_file(&p.path, &updated).await {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!(
                    "Edited {path} (replaced {count} occurrence{s})",
                    path = p.path,
                    count = occurrence_count,
                    s = if occurrence_count == 1 { "" } else { "s" }
                ),
                error: None,
            },
            Err(e) => sandbox_error_to_tool_result(&call.id, e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_fs::RealFs;

    fn make_call(input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "edit_file".into(),
            input,
        }
    }

    #[tokio::test]
    async fn edit_file_rejects_ambiguous_single_replacements() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "alpha\nalpha\n").unwrap();

        let fs: Arc<dyn WorkspaceFs> = Arc::new(RealFs::new(dir.path().to_path_buf()));
        let tool = EditFileTool::new(fs);
        let result = tool
            .execute(&make_call(serde_json::json!({
                "path": "main.rs",
                "old_text": "alpha",
                "new_text": "beta"
            })))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("replace_match"));
    }

    #[tokio::test]
    async fn edit_file_replace_all_reports_replacement_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "alpha\nalpha\n").unwrap();

        let fs: Arc<dyn WorkspaceFs> = Arc::new(RealFs::new(dir.path().to_path_buf()));
        let tool = EditFileTool::new(fs);
        let result = tool
            .execute(&make_call(serde_json::json!({
                "path": "main.rs",
                "old_text": "alpha",
                "new_text": "beta",
                "replace_all": true
            })))
            .await;

        assert!(result.success, "{result:?}");
        assert!(result.output.contains("replaced 2 occurrences"));
        let updated = std::fs::read_to_string(dir.path().join("main.rs")).unwrap();
        assert_eq!(updated, "beta\nbeta\n");
    }
}
