use std::sync::Arc;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;

use super::{ToolCallExt, ToolExecutor};
use crate::workspace_fs::{WorkspaceFs, sandbox_error_to_tool_result};

pub struct ReplaceMatchTool {
    fs: Arc<dyn WorkspaceFs>,
}

impl ReplaceMatchTool {
    pub fn new(fs: Arc<dyn WorkspaceFs>) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Params {
    path: String,
    line: usize,
    column: usize,
    old_text: String,
    new_text: String,
}

fn line_segment(content: &str, target_line: usize) -> Option<(usize, &str)> {
    if target_line == 0 {
        return None;
    }
    let mut start = 0_usize;
    for (index, segment) in content.split_inclusive('\n').enumerate() {
        if index + 1 == target_line {
            return Some((start, segment));
        }
        start += segment.len();
    }

    if !content.is_empty() && !content.ends_with('\n') {
        let line_count = content.lines().count();
        if target_line == line_count {
            let start = content
                .rmatch_indices('\n')
                .next()
                .map(|(idx, _)| idx + 1)
                .unwrap_or(0);
            return Some((start, &content[start..]));
        }
    }

    None
}

fn line_body(segment: &str) -> &str {
    segment
        .strip_suffix('\n')
        .unwrap_or(segment)
        .strip_suffix('\r')
        .unwrap_or_else(|| segment.strip_suffix('\n').unwrap_or(segment))
}

#[async_trait::async_trait]
impl ToolExecutor for ReplaceMatchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "replace_match".into(),
            description:
                "Replace a specific search match using exact path, line, and column coordinates"
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file, relative to workspace root"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number of the match"
                    },
                    "column": {
                        "type": "integer",
                        "description": "1-based byte column where the match starts"
                    },
                    "old_text": {
                        "type": "string",
                        "description": "Exact text expected at the provided line and column"
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Replacement text"
                    }
                },
                "required": ["path", "line", "column", "old_text", "new_text"]
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
        if p.line == 0 || p.column == 0 {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("line and column must both be >= 1".into()),
            };
        }

        let mut content = match self.fs.read_file(&p.path).await {
            Ok(c) => c,
            Err(e) => return sandbox_error_to_tool_result(&call.id, e),
        };

        let total_occurrences = content.matches(&p.old_text).count();
        let Some((line_start, segment)) = line_segment(&content, p.line) else {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("line {} does not exist in {}", p.line, p.path)),
            };
        };

        let body = line_body(segment);
        let byte_column = p.column - 1;
        if byte_column > body.len() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!(
                    "column {} is outside line {} in {}",
                    p.column, p.line, p.path
                )),
            };
        }

        let absolute_start = line_start + byte_column;
        let absolute_end = absolute_start + p.old_text.len();
        let Some(found_text) = content.get(absolute_start..absolute_end) else {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!(
                    "old_text does not fit at {}:{}:{}",
                    p.path, p.line, p.column
                )),
            };
        };

        if found_text != p.old_text {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Expected '{}' at {}:{}:{}, found '{}'",
                    p.old_text, p.path, p.line, p.column, found_text
                )),
            };
        }

        content.replace_range(absolute_start..absolute_end, &p.new_text);
        match self.fs.write_file(&p.path, &content).await {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!(
                    "Replaced match at {}:{}:{} ({} total occurrence(s) of old_text in file)",
                    p.path, p.line, p.column, total_occurrences
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
            name: "replace_match".into(),
            input,
        }
    }

    #[tokio::test]
    async fn replace_match_replaces_the_targeted_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.rs"),
            "fn main() { let first = alpha; let second = alpha; }\n",
        )
        .unwrap();

        let fs: Arc<dyn WorkspaceFs> = Arc::new(RealFs::new(dir.path().to_path_buf()));
        let tool = ReplaceMatchTool::new(fs);
        let result = tool
            .execute(&make_call(serde_json::json!({
                "path": "main.rs",
                "line": 1,
                "column": 45,
                "old_text": "alpha",
                "new_text": "beta"
            })))
            .await;

        assert!(result.success, "{result:?}");
        let updated = std::fs::read_to_string(dir.path().join("main.rs")).unwrap();
        assert_eq!(
            updated,
            "fn main() { let first = alpha; let second = beta; }\n"
        );
    }

    #[tokio::test]
    async fn replace_match_fails_when_the_coordinate_does_not_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() { alpha; }\n").unwrap();

        let fs: Arc<dyn WorkspaceFs> = Arc::new(RealFs::new(dir.path().to_path_buf()));
        let tool = ReplaceMatchTool::new(fs);
        let result = tool
            .execute(&make_call(serde_json::json!({
                "path": "main.rs",
                "line": 1,
                "column": 1,
                "old_text": "alpha",
                "new_text": "beta"
            })))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Expected 'alpha'"));
    }
}
