//! `search_semantic` tool.
//!
//! Delegates to `nca-index`, which uses Tantivy's BM25 scorer when built with
//! the `semantic-index` feature. When the feature is disabled the tool still
//! registers — it returns a clear, actionable error pointing the user at the
//! correct cargo feature flag. This keeps the default binary lean without
//! hiding the tool from the agent's tool list.

use async_trait::async_trait;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde_json::json;
use std::path::PathBuf;

use crate::tools::ToolExecutor;

pub struct SearchSemanticTool {
    workspace_root: PathBuf,
}

impl SearchSemanticTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait]
impl ToolExecutor for SearchSemanticTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "search_semantic".into(),
            description:
                "BM25-ranked whole-workspace search. Prefer `search_code` for literal strings; \
                 use `search_semantic` for multi-word conceptual queries over indexed files. \
                 Requires `nca index rebuild` to have populated `.nca/index/` first."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Multi-word BM25 query." },
                    "limit": { "type": "integer", "description": "Max hits to return (default 10)." }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let query = call
            .input
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let limit = call
            .input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        if query.trim().is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("`query` is required".into()),
            };
        }

        if !nca_index::Index::is_available() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(
                    "search_semantic is disabled in this build. Rebuild with \
                     `cargo build --release --features semantic-index` and run \
                     `nca index rebuild` before using this tool."
                        .into(),
                ),
            };
        }

        let root = self.workspace_root.clone();
        let q = query.clone();
        let hits = tokio::task::spawn_blocking(move || {
            let idx = nca_index::Index::open(&root)?;
            idx.search(&q, limit)
        })
        .await;

        match hits {
            Ok(Ok(hits)) => {
                let payload = json!({
                    "query": query,
                    "hits": hits.iter().map(|h| json!({
                        "path": h.path,
                        "score": h.score,
                        "line": h.line,
                        "snippet": h.snippet,
                    })).collect::<Vec<_>>(),
                });
                ToolResult {
                    call_id: call.id.clone(),
                    success: true,
                    output: payload.to_string(),
                    error: None,
                }
            }
            Ok(Err(err)) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(err.to_string()),
            },
            Err(join_err) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("index task panicked: {join_err}")),
            },
        }
    }
}
