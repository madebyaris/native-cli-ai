use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

pub struct ApplyPatchTool {
    workspace_root: std::path::PathBuf,
}

impl ApplyPatchTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
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
                                "new_text": { "type": "string" },
                                "replace_all": { "type": "boolean" }
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
        let path = call.input["path"].as_str().unwrap_or("");
        let full_path = self.workspace_root.join(path);
        let canonical = match full_path.canonicalize() {
            Ok(canonical) if canonical.starts_with(&self.workspace_root) => canonical,
            _ => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some("Path is outside the workspace".into()),
                };
            }
        };

        let mut content = match tokio::fs::read_to_string(&canonical).await {
            Ok(content) => content,
            Err(err) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to read file: {err}")),
                };
            }
        };

        let Some(edits) = call.input["edits"].as_array() else {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("edits must be an array".into()),
            };
        };

        for edit in edits {
            let old_text = edit["old_text"].as_str().unwrap_or("");
            let new_text = edit["new_text"].as_str().unwrap_or("");
            let replace_all = edit["replace_all"].as_bool().unwrap_or(false);

            if old_text.is_empty() {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some("old_text must not be empty".into()),
                };
            }

            if replace_all {
                if !content.contains(old_text) {
                    return ToolResult {
                        call_id: call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(format!("text not found in {}", canonical.display())),
                    };
                }
                content = content.replace(old_text, new_text);
            } else if let Some(index) = content.find(old_text) {
                content.replace_range(index..index + old_text.len(), new_text);
            } else {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("text not found in {}", canonical.display())),
                };
            }
        }

        match tokio::fs::write(&canonical, content).await {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!("Patched {}", canonical.display()),
                error: None,
            },
            Err(err) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to write file: {err}")),
            },
        }
    }
}
