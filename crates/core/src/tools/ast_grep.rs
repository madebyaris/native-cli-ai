use std::path::{Path, PathBuf};

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::Deserialize;
use serde_json::Value;

use super::ToolExecutor;

/// AST-aware code search tool backed by ast-grep.
///
/// Unlike `search_code` (ripgrep text search), this matches against the
/// abstract syntax tree so results are structurally accurate.
pub struct AstGrepSearchTool {
    workspace_root: PathBuf,
}

/// AST-aware code search-and-replace tool backed by ast-grep.
///
/// Dry-run by default: returns the proposed changes without applying them.
/// Set `apply` to `true` to write changes to disk.
pub struct AstGrepReplaceTool {
    workspace_root: PathBuf,
}

// ---------------------------------------------------------------------------
// ast-grep JSON output shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgMatch {
    text: String,
    file: String,
    lines: Option<String>,
    replacement: Option<String>,
    #[serde(default)]
    meta_variables: MetaVariables,
    range: SgRange,
}

#[derive(Debug, Deserialize)]
struct SgRange {
    start: SgPosition,
}

#[derive(Debug, Deserialize)]
struct SgPosition {
    line: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct MetaVariables {
    #[serde(default)]
    single: std::collections::HashMap<String, MetaVar>,
    #[serde(default)]
    multi: std::collections::HashMap<String, Vec<MetaVar>>,
    #[serde(default)]
    transformed: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct MetaVar {
    text: String,
    range: SgRange,
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Supported languages passed to `ast-grep --lang`.
fn is_supported_lang(lang: &str) -> bool {
    matches!(
        lang,
        "rust"
            | "python"
            | "javascript"
            | "typescript"
            | "tsx"
            | "java"
            | "go"
            | "c"
            | "cpp"
            | "csharp"
            | "html"
            | "css"
            | "json"
            | "yaml"
            | "kotlin"
            | "swift"
            | "scala"
            | "haskell"
            | "ruby"
            | "php"
    )
}

/// Build the search root path, resolving relative to workspace and
/// validating it stays inside the workspace.
fn resolve_search_root(workspace_root: &Path, path: Option<&str>) -> Result<PathBuf, String> {
    let root = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => workspace_root.join(p),
        None => workspace_root.to_path_buf(),
    };
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let canonical = root
        .canonicalize()
        .map_err(|err| format!("Failed to resolve path '{path:?}': {err}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("Path is outside the workspace".into());
    }
    Ok(root)
}

fn make_call_result(
    call_id: &str,
    success: bool,
    output: String,
    error: Option<String>,
) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        success,
        output,
        error,
    }
}

// ---------------------------------------------------------------------------
// AstGrepSearchTool
// ---------------------------------------------------------------------------

impl AstGrepSearchTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for AstGrepSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "ast_grep_search".into(),
            description: "AST-aware code search across the filesystem. Supports 25 languages. \
                Uses meta-variables: $VAR (single node), $$$ (multiple nodes). \
                IMPORTANT: Patterns must be complete AST nodes (valid code). \
                Examples: 'console.log($MSG)', 'def $FUNC($$$):', 'async function $NAME($$$)'"
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "AST pattern with meta-variables ($VAR, $$$). Must be complete AST node."
                    },
                    "lang": {
                        "type": "string",
                        "description": "Target language (e.g. rust, python, typescript, tsx, go, java, c, cpp, javascript, html, css, json, yaml, kotlin, swift, scala, haskell, ruby, php, csharp)."
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search (default: workspace root)"
                    },
                    "glob": {
                        "type": "string",
                        "description": "File glob filter (e.g. '**/*.rs')"
                    },
                    "context": {
                        "type": "integer",
                        "description": "Context lines around match (default: 0)"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of matches to return (default: 100)"
                    }
                },
                "required": ["pattern", "lang"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let pattern = match call.input["pattern"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(p) => p.to_string(),
            None => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some("pattern is required".into()),
                );
            }
        };

        let lang = match call.input["lang"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(l) if is_supported_lang(l) => l.to_string(),
            Some(l) => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some(format!("unsupported language '{l}'")),
                );
            }
            None => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some("lang is required".into()),
                );
            }
        };

        let search_root =
            match resolve_search_root(&self.workspace_root, call.input["path"].as_str()) {
                Ok(r) => r,
                Err(e) => return make_call_result(&call.id, false, String::new(), Some(e)),
            };

        let context = call.input["context"].as_u64().unwrap_or(0) as usize;
        let max_results = call.input["max_results"].as_u64().unwrap_or(100) as usize;
        let glob = call.input["glob"].as_str().map(str::to_string);

        let mut cmd = tokio::process::Command::new("ast-grep");
        cmd.arg("run")
            .arg("--pattern")
            .arg(&pattern)
            .arg("--lang")
            .arg(&lang)
            .arg("--json=compact")
            .arg(search_root)
            .current_dir(&self.workspace_root);

        if context > 0 {
            cmd.arg(format!("--context={context}"));
        }
        if let Some(g) = &glob {
            cmd.arg("--glob").arg(g);
        }

        let output = match cmd.output().await {
            Ok(o) => o,
            Err(err) => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some(format!("Failed to run ast-grep: {err}")),
                );
            }
        };

        let code = output.status.code().unwrap_or(-1);
        // ast-grep exits 0 even with no matches, but may exit non-zero on errors
        let stderr = String::from_utf8_lossy(&output.stderr);
        if code != 0 {
            return make_call_result(
                &call.id,
                false,
                String::new(),
                if stderr.is_empty() {
                    Some(format!("ast-grep exited with code {code}"))
                } else {
                    Some(format!("ast-grep exited with code {code}: {stderr}"))
                },
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return make_call_result(
                &call.id,
                true,
                serde_json::to_string_pretty(&serde_json::json!({
                    "pattern": pattern,
                    "lang": lang,
                    "total_matches": 0,
                    "matches": []
                }))
                .unwrap_or_default(),
                None,
            );
        }

        // Parse the JSON array from ast-grep
        let raw_matches: Vec<SgMatch> = match serde_json::from_str(&stdout) {
            Ok(m) => m,
            Err(err) => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some(format!("Failed to parse ast-grep output: {err}")),
                );
            }
        };

        let truncated = raw_matches.len() > max_results;
        let shown = raw_matches.len().min(max_results);

        let matches_json: Vec<Value> = raw_matches[..shown]
            .iter()
            .map(|m| {
                let file = m.file.trim_start_matches("./").to_string();
                let mut obj = serde_json::json!({
                    "file": file,
                    "line": m.range.start.line + 1,
                    "text": m.text,
                });
                if let Some(lines) = &m.lines {
                    obj["lines"] = serde_json::json!(lines);
                }
                // Expose meta-variables for agent context
                if !m.meta_variables.single.is_empty() {
                    let singles: serde_json::Map<String, Value> = m
                        .meta_variables
                        .single
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                serde_json::json!({
                                    "text": v.text,
                                    "line": v.range.start.line + 1,
                                }),
                            )
                        })
                        .collect();
                    obj["meta_variables"] = serde_json::json!({ "single": singles });
                }
                obj
            })
            .collect();

        let response = serde_json::json!({
            "pattern": pattern,
            "lang": lang,
            "total_matches": raw_matches.len(),
            "returned_matches": shown,
            "truncated": truncated,
            "matches": matches_json,
        });

        make_call_result(
            &call.id,
            true,
            serde_json::to_string_pretty(&response).unwrap_or_default(),
            None,
        )
    }
}

// ---------------------------------------------------------------------------
// AstGrepReplaceTool
// ---------------------------------------------------------------------------

impl AstGrepReplaceTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for AstGrepReplaceTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "ast_grep_replace".into(),
            description: "Replace code patterns across filesystem with AST-aware rewriting. \
                Dry-run by default. Use meta-variables in rewrite to preserve matched content. \
                Example: pattern='console.log($MSG)' rewrite='logger.info($MSG)'"
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "AST pattern to match"
                    },
                    "rewrite": {
                        "type": "string",
                        "description": "Replacement pattern (can use $VAR from pattern)"
                    },
                    "lang": {
                        "type": "string",
                        "description": "Target language"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search (default: workspace root)"
                    },
                    "glob": {
                        "type": "string",
                        "description": "File glob filter"
                    },
                    "apply": {
                        "type": "boolean",
                        "description": "Actually write changes to files (default: false = dry-run)"
                    }
                },
                "required": ["pattern", "rewrite", "lang"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let pattern = match call.input["pattern"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(p) => p.to_string(),
            None => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some("pattern is required".into()),
                );
            }
        };

        let rewrite = match call.input["rewrite"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(r) => r.to_string(),
            None => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some("rewrite is required".into()),
                );
            }
        };

        let lang = match call.input["lang"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(l) if is_supported_lang(l) => l.to_string(),
            Some(l) => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some(format!("unsupported language '{l}'")),
                );
            }
            None => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some("lang is required".into()),
                );
            }
        };

        let search_root =
            match resolve_search_root(&self.workspace_root, call.input["path"].as_str()) {
                Ok(r) => r,
                Err(e) => return make_call_result(&call.id, false, String::new(), Some(e)),
            };

        let apply = call.input["apply"].as_bool().unwrap_or(false);
        let glob = call.input["glob"].as_str().map(str::to_string);

        let mut cmd = tokio::process::Command::new("ast-grep");
        cmd.arg("run")
            .arg("--pattern")
            .arg(&pattern)
            .arg("--rewrite")
            .arg(&rewrite)
            .arg("--lang")
            .arg(&lang)
            .arg("--json=compact")
            .arg(search_root)
            .current_dir(&self.workspace_root);

        if apply {
            cmd.arg("--update-all");
        }

        if let Some(g) = &glob {
            cmd.arg("--glob").arg(g);
        }

        let output = match cmd.output().await {
            Ok(o) => o,
            Err(err) => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some(format!("Failed to run ast-grep: {err}")),
                );
            }
        };

        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if code != 0 {
            return make_call_result(
                &call.id,
                false,
                String::new(),
                if stderr.is_empty() {
                    Some(format!("ast-grep exited with code {code}"))
                } else {
                    Some(format!("ast-grep exited with code {code}: {stderr}"))
                },
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return make_call_result(
                &call.id,
                true,
                serde_json::to_string_pretty(&serde_json::json!({
                    "pattern": pattern,
                    "lang": lang,
                    "apply": apply,
                    "total_matches": 0,
                    "changes": []
                }))
                .unwrap_or_default(),
                None,
            );
        }

        let raw_matches: Vec<SgMatch> = match serde_json::from_str(&stdout) {
            Ok(m) => m,
            Err(err) => {
                return make_call_result(
                    &call.id,
                    false,
                    String::new(),
                    Some(format!("Failed to parse ast-grep output: {err}")),
                );
            }
        };

        let changes: Vec<Value> = raw_matches
            .iter()
            .map(|m| {
                let file = m.file.trim_start_matches("./").to_string();
                serde_json::json!({
                    "file": file,
                    "line": m.range.start.line + 1,
                    "matched": m.text,
                    "replacement": m.replacement,
                })
            })
            .collect();

        let response = serde_json::json!({
            "pattern": pattern,
            "rewrite": rewrite,
            "lang": lang,
            "apply": apply,
            "total_matches": raw_matches.len(),
            "changes": changes,
        });

        let note = if apply {
            String::new()
        } else {
            "Dry-run: no files were modified. Set apply=true to write changes.".to_string()
        };

        make_call_result(
            &call.id,
            true,
            serde_json::to_string_pretty(&response).unwrap_or_default() + "\n" + &note,
            None,
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_search_call(input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test-1".into(),
            name: "ast_grep_search".into(),
            input,
        }
    }

    fn make_replace_call(input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test-1".into(),
            name: "ast_grep_replace".into(),
            input,
        }
    }

    /// Helper: create a minimal Rust file for testing.
    fn write_rs_file(dir: &TempDir, content: &str) {
        std::fs::write(dir.path().join("main.rs"), content).unwrap();
    }

    #[tokio::test]
    async fn search_finds_function_pattern() {
        let dir = tempfile::tempdir().unwrap();
        write_rs_file(&dir, "fn hello() {}\nfn world() {}\n");

        let tool = AstGrepSearchTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_search_call(serde_json::json!({
                "pattern": "fn $NAME($$$) { $$$ }",
                "lang": "rust"
            })))
            .await;

        assert!(result.success, "search failed: {:?}", result.error);
        let parsed: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(parsed["total_matches"], 2);
    }

    #[tokio::test]
    async fn search_returns_empty_on_no_match() {
        let dir = tempfile::tempdir().unwrap();
        write_rs_file(&dir, "fn hello() {}\n");

        let tool = AstGrepSearchTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_search_call(serde_json::json!({
                "pattern": "class $NAME { $$$ }",
                "lang": "rust"
            })))
            .await;

        assert!(result.success, "search failed: {:?}", result.error);
        let parsed: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(parsed["total_matches"], 0);
        assert_eq!(parsed["matches"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn replace_dry_run_does_not_modify_files() {
        let dir = tempfile::tempdir().unwrap();
        let content = "let x = 1;\nlet y = 2;\n";
        write_rs_file(&dir, content);

        let tool = AstGrepReplaceTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_replace_call(serde_json::json!({
                "pattern": "let $A = $B",
                "rewrite": "const $A: i32 = $B",
                "lang": "rust",
                "apply": false
            })))
            .await;

        assert!(result.success, "replace failed: {:?}", result.error);
        // File must be unchanged
        let on_disk = std::fs::read_to_string(dir.path().join("main.rs")).unwrap();
        assert_eq!(on_disk, content);
    }

    #[tokio::test]
    async fn search_rejects_outside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        write_rs_file(&dir, "fn hello() {}\n");

        let tool = AstGrepSearchTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_search_call(serde_json::json!({
                "pattern": "fn $NAME($$$) { $$$ }",
                "lang": "rust",
                "path": "/etc"
            })))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("outside"));
    }

    #[tokio::test]
    async fn search_rejects_missing_lang() {
        let dir = tempfile::tempdir().unwrap();
        write_rs_file(&dir, "fn hello() {}\n");

        let tool = AstGrepSearchTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_search_call(serde_json::json!({
                "pattern": "fn $NAME($$$) { $$$ }"
            })))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("lang is required"));
    }

    #[tokio::test]
    async fn search_rejects_unsupported_lang() {
        let dir = tempfile::tempdir().unwrap();
        write_rs_file(&dir, "fn hello() {}\n");

        let tool = AstGrepSearchTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_search_call(serde_json::json!({
                "pattern": "fn $NAME($$$) { $$$ }",
                "lang": "brainfuck"
            })))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("unsupported"));
    }
}
