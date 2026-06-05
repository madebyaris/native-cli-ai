use crate::tools::ToolExecutor;
use nca_common::config::McpServerConfig;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ClientInfo, Implementation};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

pub async fn load_mcp_tools(
    workspace_root: &Path,
    servers: &[McpServerConfig],
) -> Result<Vec<Box<dyn ToolExecutor>>, String> {
    let mut tools: Vec<Box<dyn ToolExecutor>> = Vec::new();
    for server in servers.iter().filter(|server| server.enabled) {
        let server_tools = discover_server_tools(workspace_root, server).await?;
        for tool in server_tools {
            tools.push(Box::new(tool));
        }
    }
    Ok(tools)
}

#[derive(Clone)]
pub struct McpTool {
    server: McpServerConfig,
    workspace_root: PathBuf,
    tool_name: String,
    description: Option<String>,
    parameters: Value,
}

impl McpTool {
    fn prefixed_name(&self) -> String {
        format!("mcp__{}__{}", self.server.name, self.tool_name)
    }
}

#[async_trait::async_trait]
impl ToolExecutor for McpTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.prefixed_name(),
            description: self.description.clone().unwrap_or_else(|| {
                format!("MCP tool `{}` from `{}`", self.tool_name, self.server.name)
            }),
            parameters: self.parameters.clone(),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let server = self.server.clone();
        let workspace_root = self.workspace_root.clone();
        let tool_name = self.tool_name.clone();
        let input = call.input.clone();
        let call_id = call.id.clone();
        match execute_mcp_call(&workspace_root, &server, &tool_name, input).await {
            Ok(output) => ToolResult {
                call_id,
                success: true,
                output,
                error: None,
            },
            Err(error) => ToolResult {
                call_id,
                success: false,
                output: String::new(),
                error: Some(error),
            },
        }
    }
}

/// Spawn a child process for an MCP server, returning (stdout, stdin, child).
/// The child is kept alive for cleanup; caller must kill it when done.
fn spawn_mcp_server(
    server: &McpServerConfig,
    workspace_root: &Path,
) -> Result<
    (
        tokio::process::ChildStdout,
        tokio::process::ChildStdin,
        tokio::process::Child,
    ),
    String,
> {
    let mut cmd = Command::new(&server.command);
    cmd.args(&server.args)
        .envs(&server.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let working_dir = server
        .cwd
        .clone()
        .unwrap_or_else(|| workspace_root.to_path_buf());
    cmd.current_dir(working_dir);

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to start MCP server `{}`: {err}", server.name))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("missing stdin for MCP server `{}`", server.name))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("missing stdout for MCP server `{}`", server.name))?;

    Ok((stdout, stdin, child))
}

async fn discover_server_tools(
    workspace_root: &Path,
    server: &McpServerConfig,
) -> Result<Vec<McpTool>, String> {
    let (stdout, stdin, mut child) = spawn_mcp_server(server, workspace_root)?;

    let client_info = ClientInfo::new(
        rmcp::model::ClientCapabilities::default(),
        Implementation::new("nca", env!("CARGO_PKG_VERSION")),
    );
    let client = client_info
        .serve((stdout, stdin))
        .await
        .map_err(|err| format!("MCP server `{}` init failed: {err}", server.name))?;

    let tools = client
        .list_all_tools()
        .await
        .map_err(|err| format!("MCP server `{}` list_tools failed: {err}", server.name))?;

    let result: Vec<McpTool> = tools
        .into_iter()
        .map(|tool| {
            let schema = tool.input_schema.as_ref();
            let parameters = serde_json::json!({
                "type": "object",
                "properties": schema.get("properties"),
                "required": schema.get("required"),
            });

            McpTool {
                server: server.clone(),
                workspace_root: workspace_root.to_path_buf(),
                tool_name: tool.name.to_string(),
                description: tool.description.map(|d| d.to_string()),
                parameters,
            }
        })
        .collect();

    // rmcp handles transport close on cancel; kill child to avoid zombies
    let _ = child.start_kill();
    let _ = client.cancel().await;
    Ok(result)
}

async fn execute_mcp_call(
    workspace_root: &Path,
    server: &McpServerConfig,
    tool_name: &str,
    input: Value,
) -> Result<String, String> {
    let (stdout, stdin, mut child) = spawn_mcp_server(server, workspace_root)?;

    let client_info = ClientInfo::new(
        rmcp::model::ClientCapabilities::default(),
        Implementation::new("nca", env!("CARGO_PKG_VERSION")),
    );
    let client = client_info
        .serve((stdout, stdin))
        .await
        .map_err(|err| format!("MCP server `{}` init failed: {err}", server.name))?;

    let arguments = input.as_object().cloned().unwrap_or_default();

    let result = client
        .call_tool(CallToolRequestParams::new(tool_name.to_owned()).with_arguments(arguments))
        .await
        .map_err(|err| format!("MCP tool `{}` call failed: {err}", tool_name))?;

    let _ = child.start_kill();
    let _ = client.cancel().await;

    let output: Vec<String> = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    serde_json::to_string(&output).map_err(|err| err.to_string())
}
