pub mod bash;
pub mod code_intel_tool;
pub mod create_directory;
pub mod filesystem;
pub mod git;
pub mod list_directory;
pub mod search;
pub mod types;
pub mod write_file;

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

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

    pub fn with_default_readonly_tools(workspace_root: std::path::PathBuf) -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(filesystem::ReadFileTool::new(
            workspace_root.clone(),
        )));
        registry.register(Box::new(search::SearchCodeTool::new(
            workspace_root.clone(),
        )));
        registry.register(Box::new(list_directory::ListDirectoryTool::new(
            workspace_root.clone(),
        )));
        registry.register(Box::new(git::GitStatusTool::new(workspace_root.clone())));
        registry.register(Box::new(git::GitDiffTool::new(workspace_root)));
        registry
    }

    pub fn with_default_full_tools(workspace_root: std::path::PathBuf) -> Self {
        let mut registry = Self::with_default_readonly_tools(workspace_root.clone());
        registry.register(Box::new(code_intel_tool::CodeIntelTool::new(
            crate::code_intel::FastLocalCodeIntel::new(workspace_root.clone()),
        )));
        registry.register(Box::new(write_file::WriteFileTool::new(
            workspace_root.clone(),
        )));
        registry.register(Box::new(create_directory::CreateDirectoryTool::new(
            workspace_root,
        )));
        registry
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
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

/// Trait implemented by each tool.
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, call: &ToolCall) -> ToolResult;
}
