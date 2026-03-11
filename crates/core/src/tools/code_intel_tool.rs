use crate::code_intel::CodeIntel;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

pub struct CodeIntelTool<T: CodeIntel> {
    intel: T,
}

impl<T: CodeIntel> CodeIntelTool<T> {
    pub fn new(intel: T) -> Self {
        Self { intel }
    }
}

#[async_trait::async_trait]
impl<T: CodeIntel> ToolExecutor for CodeIntelTool<T> {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "query_symbols".into(),
            description: "Search for likely symbol definitions quickly using Rust-native local code intelligence".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "glob": { "type": "string" }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let query = call.input["query"].as_str().unwrap_or("");
        let glob = call.input["glob"].as_str();
        match self.intel.query_symbols(query, glob).await {
            Ok(matches) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: matches
                    .into_iter()
                    .map(|m| format!("{}:{}:{}", m.file.display(), m.line, m.text))
                    .collect::<Vec<_>>()
                    .join("\n"),
                error: None,
            },
            Err(err) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(err.to_string()),
            },
        }
    }
}
