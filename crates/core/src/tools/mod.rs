pub mod apply_patch;
pub mod ask_question;
pub mod ast_grep;
pub mod code_intel_tool;
pub mod copy_path;
pub mod create_directory;
pub mod delete_path;
pub mod edit_file;
pub mod fetch_url;
pub mod filesystem;
pub mod git;
pub mod input_repair;
pub mod invoke_skill;
pub mod list_directory;
pub mod mcp;
pub mod move_path;
pub mod rename_path;
pub mod replace_match;
pub mod run_validation;
pub mod search;
pub mod spawn_subagent;
pub mod types;
pub mod web_search;
pub mod write_file;

pub use ask_question::AskQuestionTool;
pub use invoke_skill::InvokeSkillTool;

use nca_common::config::WebConfig;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use std::sync::Arc;

use crate::workspace_fs::WorkspaceFs;

/// Registry of available tools the agent can invoke.
pub struct ToolRegistry {
    tools: Vec<Box<dyn ToolExecutor>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Box<dyn ToolExecutor>) {
        self.tools.push(tool);
    }

    pub fn with_default_readonly_tools(fs: Arc<dyn WorkspaceFs>, web_config: WebConfig) -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(filesystem::ReadFileTool::new(fs.clone())));
        registry.register(Box::new(search::SearchCodeTool::new(fs.clone())));
        registry.register(Box::new(ast_grep::AstGrepSearchTool::new(fs.clone())));
        registry.register(Box::new(list_directory::ListDirectoryTool::new(fs.clone())));
        registry.register(Box::new(git::GitStatusTool::new(fs.clone())));
        registry.register(Box::new(git::GitDiffTool::new(fs)));
        registry.register(Box::new(web_search::WebSearchTool::new(web_config.clone())));
        registry.register(Box::new(fetch_url::FetchUrlTool::new(web_config)));
        registry
    }

    pub fn with_default_full_tools(fs: Arc<dyn WorkspaceFs>, web_config: WebConfig) -> Self {
        let mut registry = Self::with_default_readonly_tools(fs.clone(), web_config);
        registry.register(Box::new(code_intel_tool::CodeIntelTool::new(
            crate::code_intel::FastLocalCodeIntel::new(fs.root()),
        )));
        registry.register(Box::new(write_file::WriteFileTool::new(fs.clone())));
        registry.register(Box::new(create_directory::CreateDirectoryTool::new(
            fs.clone(),
        )));
        registry.register(Box::new(apply_patch::ApplyPatchTool::new(fs.clone())));
        registry.register(Box::new(edit_file::EditFileTool::new(fs.clone())));
        registry.register(Box::new(replace_match::ReplaceMatchTool::new(fs.clone())));
        registry.register(Box::new(ast_grep::AstGrepReplaceTool::new(fs.clone())));
        registry.register(Box::new(rename_path::RenamePathTool::new(fs.clone())));
        registry.register(Box::new(move_path::MovePathTool::new(fs.clone())));
        registry.register(Box::new(copy_path::CopyPathTool::new(fs.clone())));
        registry.register(Box::new(delete_path::DeletePathTool::new(fs.clone())));
        registry.register(Box::new(run_validation::RunValidationTool::new(fs)));
        registry
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut seen = std::collections::HashSet::new();
        self.tools
            .iter()
            .filter_map(|t| {
                let def = t.definition();
                if seen.insert(def.name.clone()) {
                    Some(def)
                } else {
                    tracing::warn!("duplicate tool name skipped: {}", def.name);
                    None
                }
            })
            .collect()
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        for tool in &self.tools {
            if tool.definition().name == call.name {
                return tool.execute(call).await;
            }
        }

        ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: String::new(),
            error: Some(format!("Unknown tool: {}", call.name)),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait implemented by each tool.
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, call: &ToolCall) -> ToolResult;
}

// ---------------------------------------------------------------------------
// C2: Typed parameter extraction
// ---------------------------------------------------------------------------

/// Extension trait for extracting typed parameters from a [`ToolCall`].
///
/// Each tool defines a `#[derive(Deserialize)]` struct for its parameters and
/// calls `call.extract_params::<Params>()?` at the top of `execute()`. This
/// replaces the repetitive `call.input["key"].as_str().unwrap_or("")` pattern
/// with a single deserialization call that:
/// - reports missing required fields clearly,
/// - coerces types via serde,
/// - provides compile-time struct shape for tests.
///
/// **Validate-then-repair**: if the initial deserialization fails, the input
/// is passed through [`input_repair::repair_value`] which fixes common LLM
/// tool-calling mistakes (null optional fields, stringified arrays, bare
/// objects where arrays are expected, etc.) before retrying. Valid inputs
/// are never touched.
pub trait ToolCallExt {
    fn extract_params<T: serde::de::DeserializeOwned>(&self) -> Result<T, ToolResult>;
}

impl ToolCallExt for ToolCall {
    fn extract_params<T: serde::de::DeserializeOwned>(&self) -> Result<T, ToolResult> {
        // Phase 1: try direct deserialization (fast path for well-formed inputs).
        if let Ok(params) = serde_json::from_value::<T>(self.input.clone()) {
            return Ok(params);
        }

        // Phase 2: repair then retry (handles ~90% of open-model tool errors).
        let repaired = input_repair::repair_value(&self.input);
        let Ok(params) = serde_json::from_value::<T>(repaired) else {
            return Err(ToolResult {
                call_id: self.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format_tool_param_error(&self.name, &self.input)),
            });
        };
        tracing::info!(
            tool = %self.name,
            call_id = %self.id,
            "tool_input_repaired"
        );
        Ok(params)
    }
}

/// Build a model-readable error message for invalid tool parameters.
///
/// Instead of a raw serde error (which models can't recover from), this
/// surfaces the tool name and the parameter keys that were received.
fn format_tool_param_error(tool_name: &str, input: &serde_json::Value) -> String {
    let received_keys = match input.as_object() {
        Some(map) => map.keys().cloned().collect::<Vec<_>>().join(", "),
        None => format!("raw value: {}", input),
    };
    format!(
        "Invalid parameters for tool `{}`. Received keys: [{}]. Please check the tool schema and retry.",
        tool_name, received_keys
    )
}
