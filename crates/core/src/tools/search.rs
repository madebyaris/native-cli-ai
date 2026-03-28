use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Serialize;
use serde_json::Value;

use super::ToolExecutor;

/// Code search tool that shells out to ripgrep and returns structured JSON.
pub struct SearchCodeTool {
    workspace_root: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct ContextLine {
    line_number: u64,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct SearchMatch {
    path: String,
    line_number: u64,
    column: u64,
    end_column: u64,
    matched_text: String,
    line_text: String,
    before_context: Vec<ContextLine>,
    after_context: Vec<ContextLine>,
}

#[derive(Debug, Serialize)]
struct SearchResponse {
    pattern: String,
    mode: &'static str,
    path: String,
    glob: Option<String>,
    case_sensitive: bool,
    word: bool,
    context_before: usize,
    context_after: usize,
    max_results: usize,
    total_matches: usize,
    returned_matches: usize,
    truncated: bool,
    matches: Vec<SearchMatch>,
}

impl SearchCodeTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

fn relative_search_root(workspace_root: &Path, scope: Option<&str>) -> Result<String, String> {
    let Some(scope) = scope.map(str::trim).filter(|scope| !scope.is_empty()) else {
        return Ok(".".into());
    };

    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let candidate = workspace_root.join(scope);
    let canonical = candidate
        .canonicalize()
        .map_err(|err| format!("Failed to resolve search path '{scope}': {err}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("Search path is outside the workspace".into());
    }
    canonical
        .strip_prefix(&canonical_root)
        .map(|path| {
            let rendered = path.display().to_string();
            if rendered.is_empty() {
                ".".into()
            } else {
                rendered
            }
        })
        .map_err(|_| "Failed to render search path relative to workspace".into())
}

fn json_text(value: Option<&Value>) -> String {
    value
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn trimmed_line_text(value: &Value) -> String {
    json_text(value.get("lines"))
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string()
}

fn normalize_result_path(path: String) -> String {
    if path == "." {
        path
    } else {
        path.trim_start_matches("./").to_string()
    }
}

fn parse_context_line(data: &Value) -> Option<ContextLine> {
    Some(ContextLine {
        line_number: data.get("line_number")?.as_u64()?,
        text: trimmed_line_text(data),
    })
}

#[async_trait::async_trait]
impl ToolExecutor for SearchCodeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "search_code".into(),
            description: "Search code with ripgrep and return structured JSON match objects".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional file or directory path to search inside, relative to workspace root"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Optional file glob filter (e.g. '*.rs')"
                    },
                    "fixed_strings": {
                        "type": "boolean",
                        "description": "Treat pattern as a literal string instead of a regex"
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "Whether matching should be case-sensitive. Defaults to true."
                    },
                    "word": {
                        "type": "boolean",
                        "description": "Require matches to occur on word boundaries"
                    },
                    "context_before": {
                        "type": "integer",
                        "description": "Number of context lines to include before each match"
                    },
                    "context_after": {
                        "type": "integer",
                        "description": "Number of context lines to include after each match"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of match objects to return"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let pattern = call.input["pattern"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        if pattern.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("pattern must not be empty".into()),
            };
        }

        let scope = call.input["path"].as_str();
        let search_root = match relative_search_root(&self.workspace_root, scope) {
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

        let glob = call.input["glob"].as_str().map(str::to_string);
        let fixed_strings = call.input["fixed_strings"].as_bool().unwrap_or(false);
        let case_sensitive = call.input["case_sensitive"].as_bool().unwrap_or(true);
        let word = call.input["word"].as_bool().unwrap_or(false);
        let context_before = call.input["context_before"].as_u64().unwrap_or(0) as usize;
        let context_after = call.input["context_after"].as_u64().unwrap_or(0) as usize;
        let max_results = call.input["max_results"].as_u64().unwrap_or(100) as usize;

        let mut cmd = tokio::process::Command::new("rg");
        cmd.arg("--json")
            .arg("--color=never")
            .arg(pattern.as_str())
            .arg(search_root.as_str())
            .current_dir(&self.workspace_root);

        if fixed_strings {
            cmd.arg("--fixed-strings");
        }
        if !case_sensitive {
            cmd.arg("--ignore-case");
        }
        if word {
            cmd.arg("--word-regexp");
        }
        if context_before > 0 {
            cmd.arg("--before-context").arg(context_before.to_string());
        }
        if context_after > 0 {
            cmd.arg("--after-context").arg(context_after.to_string());
        }
        if let Some(glob) = &glob {
            cmd.arg("--glob").arg(glob);
        }

        let output = match cmd.output().await {
            Ok(output) => output,
            Err(err) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to run ripgrep: {err}")),
                };
            }
        };

        let exit_code = output.status.code().unwrap_or(-1);
        if !matches!(exit_code, 0 | 1) {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(if stderr.is_empty() {
                    format!("ripgrep failed with exit code {exit_code}")
                } else {
                    format!("ripgrep failed with exit code {exit_code}: {stderr}")
                }),
            };
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut matches: Vec<SearchMatch> = Vec::new();
        let mut total_matches = 0_usize;
        let mut truncated = false;
        let mut before_context_buffer = VecDeque::new();
        let mut pending_after_match_indices: Vec<usize> = Vec::new();
        let mut pending_after_remaining = 0_usize;

        for raw_line in stdout.lines() {
            if raw_line.trim().is_empty() {
                continue;
            }

            let event: Value = match serde_json::from_str(raw_line) {
                Ok(event) => event,
                Err(err) => {
                    return ToolResult {
                        call_id: call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to parse ripgrep JSON output: {err}")),
                    };
                }
            };

            let Some(kind) = event.get("type").and_then(Value::as_str) else {
                continue;
            };
            let data = event.get("data").unwrap_or(&Value::Null);

            match kind {
                "context" => {
                    let Some(context_line) = parse_context_line(data) else {
                        continue;
                    };
                    if pending_after_remaining > 0 && !pending_after_match_indices.is_empty() {
                        for index in &pending_after_match_indices {
                            if let Some(entry) = matches.get_mut(*index) {
                                entry.after_context.push(context_line.clone());
                            }
                        }
                        pending_after_remaining = pending_after_remaining.saturating_sub(1);
                        if pending_after_remaining == 0 {
                            pending_after_match_indices.clear();
                        }
                    }
                    if context_before > 0 {
                        before_context_buffer.push_back(context_line);
                        while before_context_buffer.len() > context_before {
                            before_context_buffer.pop_front();
                        }
                    }
                }
                "match" => {
                    let path = normalize_result_path(json_text(data.get("path")));
                    let line_number = data.get("line_number").and_then(Value::as_u64).unwrap_or(0);
                    let line_text = trimmed_line_text(data);
                    let before_context = before_context_buffer.iter().cloned().collect::<Vec<_>>();
                    before_context_buffer.clear();
                    pending_after_match_indices.clear();
                    pending_after_remaining = context_after;

                    let submatches = data
                        .get("submatches")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for submatch in submatches {
                        total_matches += 1;
                        if matches.len() >= max_results {
                            truncated = true;
                            continue;
                        }
                        let start = submatch.get("start").and_then(Value::as_u64).unwrap_or(0);
                        let end = submatch.get("end").and_then(Value::as_u64).unwrap_or(start);
                        let matched_text = json_text(submatch.get("match"));
                        matches.push(SearchMatch {
                            path: path.clone(),
                            line_number,
                            column: start + 1,
                            end_column: end + 1,
                            matched_text,
                            line_text: line_text.clone(),
                            before_context: before_context.clone(),
                            after_context: Vec::new(),
                        });
                        pending_after_match_indices.push(matches.len() - 1);
                    }
                }
                _ => {}
            }
        }

        let response = SearchResponse {
            pattern,
            mode: if fixed_strings { "literal" } else { "regex" },
            path: search_root,
            glob,
            case_sensitive,
            word,
            context_before,
            context_after,
            max_results,
            total_matches,
            returned_matches: matches.len(),
            truncated,
            matches,
        };

        match serde_json::to_string_pretty(&response) {
            Ok(output) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output,
                error: None,
            },
            Err(err) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to serialize search results: {err}")),
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
            name: "search_code".into(),
            input,
        }
    }

    #[tokio::test]
    async fn search_code_returns_structured_literal_matches() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "fn alpha() {}\nfn beta() { alpha(); }\n",
        )
        .unwrap();

        let tool = SearchCodeTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call(serde_json::json!({
                "pattern": "alpha",
                "fixed_strings": true,
                "glob": "*.rs",
                "max_results": 10
            })))
            .await;

        assert!(result.success, "{result:?}");
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["mode"], "literal");
        assert_eq!(output["total_matches"], 2);
        assert_eq!(output["returned_matches"], 2);
        assert_eq!(output["matches"][0]["path"], "src/lib.rs");
        assert_eq!(output["matches"][0]["line_number"], 1);
        assert_eq!(output["matches"][1]["line_number"], 2);
    }

    #[tokio::test]
    async fn search_code_treats_no_matches_as_success() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn present() {}\n").unwrap();

        let tool = SearchCodeTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call(serde_json::json!({
                "pattern": "missing_symbol",
                "fixed_strings": true
            })))
            .await;

        assert!(result.success, "{result:?}");
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["total_matches"], 0);
        assert_eq!(output["returned_matches"], 0);
        assert_eq!(output["matches"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn search_code_supports_literal_metacharacters() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn call() { foo.bar(); }\n").unwrap();

        let tool = SearchCodeTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call(serde_json::json!({
                "pattern": "foo.bar",
                "fixed_strings": true
            })))
            .await;

        assert!(result.success, "{result:?}");
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["total_matches"], 1);
        assert_eq!(output["matches"][0]["matched_text"], "foo.bar");
    }
}
