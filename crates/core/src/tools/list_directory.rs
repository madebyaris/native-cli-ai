use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

/// Lists files/directories under the workspace root.
pub struct ListDirectoryTool {
    workspace_root: std::path::PathBuf,
}

impl ListDirectoryTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
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
        let rel_path = call.input["path"].as_str().unwrap_or(".");
        let full_path = if rel_path == "." || rel_path.is_empty() {
            self.workspace_root.clone()
        } else {
            let candidate = std::path::PathBuf::from(rel_path);
            if candidate.is_absolute() {
                candidate
            } else {
                self.workspace_root.join(candidate)
            }
        };

        match full_path.canonicalize() {
            Ok(canonical) if canonical.starts_with(&self.workspace_root) => {
                let mut entries = match tokio::fs::read_dir(&canonical).await {
                    Ok(reader) => reader,
                    Err(e) => {
                        return ToolResult {
                            call_id: call.id.clone(),
                            success: false,
                            output: String::new(),
                            error: Some(format!("Failed to list directory: {e}")),
                        };
                    }
                };

                let mut out = Vec::new();
                loop {
                    match entries.next_entry().await {
                        Ok(Some(entry)) => {
                            let name = entry.file_name();
                            let name = name.to_string_lossy();
                            let suffix = match entry.file_type().await {
                                Ok(ft) if ft.is_dir() => "/",
                                _ => "",
                            };
                            out.push(format!("{name}{suffix}"));
                        }
                        Ok(None) => break,
                        Err(e) => {
                            return ToolResult {
                                call_id: call.id.clone(),
                                success: false,
                                output: String::new(),
                                error: Some(format!("Failed to read directory entry: {e}")),
                            };
                        }
                    }
                }

                out.sort();
                ToolResult {
                    call_id: call.id.clone(),
                    success: true,
                    output: out.join("\n"),
                    error: None,
                }
            }
            _ => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("Path is outside the workspace".into()),
            },
        }
    }
}
