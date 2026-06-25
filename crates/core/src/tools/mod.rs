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
pub trait ToolCallExt {
    fn extract_params<T: serde::de::DeserializeOwned>(&self) -> Result<T, ToolResult>;
}

impl ToolCallExt for ToolCall {
    fn extract_params<T: serde::de::DeserializeOwned>(&self) -> Result<T, ToolResult> {
        serde_json::from_value(self.input.clone()).map_err(|e| ToolResult {
            call_id: self.id.clone(),
            success: false,
            output: String::new(),
            error: Some(format!("Invalid tool parameters: {e}")),
        })
    }
}
