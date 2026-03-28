use std::path::PathBuf;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

pub struct ReplaceMatchTool {
    workspace_root: PathBuf,
}

impl ReplaceMatchTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

fn canonicalize_workspace_path(
    workspace_root: &std::path::Path,
    path: &str,
) -> Result<PathBuf, String> {
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let full_path = workspace_root.join(path);
    let canonical = full_path
        .canonicalize()
        .map_err(|err| format!("Failed to resolve path '{path}': {err}"))?;
    if canonical.starts_with(&canonical_root) {
        Ok(canonical)
    } else {
        Err("Path is outside the workspace".into())
    }
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
        let path = call.input["path"].as_str().unwrap_or("");
        let line = call.input["line"].as_u64().unwrap_or(0) as usize;
        let column = call.input["column"].as_u64().unwrap_or(0) as usize;
        let old_text = call.input["old_text"].as_str().unwrap_or("");
        let new_text = call.input["new_text"].as_str().unwrap_or("");

        if old_text.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("old_text must not be empty".into()),
            };
        }
        if line == 0 || column == 0 {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("line and column must both be >= 1".into()),
            };
        }

        let canonical = match canonicalize_workspace_path(&self.workspace_root, path) {
            Ok(path) => path,
            Err(err) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(err),
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

        let total_occurrences = content.matches(old_text).count();
        let Some((line_start, segment)) = line_segment(&content, line) else {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!(
                    "line {line} does not exist in {}",
                    canonical.display()
                )),
            };
        };

        let body = line_body(segment);
        let byte_column = column - 1;
        if byte_column > body.len() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!(
                    "column {column} is outside line {line} in {}",
                    canonical.display()
                )),
            };
        }

        let absolute_start = line_start + byte_column;
        let absolute_end = absolute_start + old_text.len();
        let Some(found_text) = content.get(absolute_start..absolute_end) else {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!(
                    "old_text does not fit at {}:{}:{}",
                    canonical.display(),
                    line,
                    column
                )),
            };
        };

        if found_text != old_text {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Expected '{old_text}' at {}:{}:{}, found '{}'",
                    canonical.display(),
                    line,
                    column,
                    found_text
                )),
            };
        }

        content.replace_range(absolute_start..absolute_end, new_text);
        match tokio::fs::write(&canonical, content).await {
            Ok(()) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!(
                    "Replaced match at {}:{}:{} ({} total occurrence(s) of old_text in file)",
                    canonical.display(),
                    line,
                    column,
                    total_occurrences
                ),
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

#[cfg(test)]
mod tests {
    use super::*;

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

        let tool = ReplaceMatchTool::new(dir.path().to_path_buf());
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

        let tool = ReplaceMatchTool::new(dir.path().to_path_buf());
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
