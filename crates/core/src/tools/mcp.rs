use crate::tools::ToolExecutor;
use nca_common::config::McpServerConfig;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub fn load_mcp_tools(
    workspace_root: &Path,
    servers: &[McpServerConfig],
) -> Result<Vec<Box<dyn ToolExecutor>>, String> {
    let mut tools: Vec<Box<dyn ToolExecutor>> = Vec::new();
    for server in servers.iter().filter(|server| server.enabled) {
        let server_tools = discover_server_tools(workspace_root, server)?;
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
            description: self
                .description
                .clone()
                .unwrap_or_else(|| format!("MCP tool `{}` from `{}`", self.tool_name, self.server.name)),
            parameters: self.parameters.clone(),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let server = self.server.clone();
        let workspace_root = self.workspace_root.clone();
        let tool_name = self.tool_name.clone();
        let input = call.input.clone();
        let call_id = call.id.clone();
        match tokio::task::spawn_blocking(move || execute_mcp_call(&workspace_root, &server, &tool_name, input)).await {
            Ok(Ok(output)) => ToolResult {
                call_id,
                success: true,
                output,
                error: None,
            },
            Ok(Err(error)) => ToolResult {
                call_id,
                success: false,
                output: String::new(),
                error: Some(error),
            },
            Err(error) => ToolResult {
                call_id,
                success: false,
                output: String::new(),
                error: Some(error.to_string()),
            },
        }
    }
}

fn discover_server_tools(
    workspace_root: &Path,
    server: &McpServerConfig,
) -> Result<Vec<McpTool>, String> {
    let mut client = McpClient::spawn(workspace_root, server)?;
    client.initialize()?;
    let tools = client.list_tools()?;
    client.shutdown();
    Ok(tools
        .into_iter()
        .map(|tool| McpTool {
            server: server.clone(),
            workspace_root: workspace_root.to_path_buf(),
            tool_name: tool.name,
            description: tool.description,
            parameters: serde_json::json!({
                "type": "object",
                "properties": tool.input_schema.properties,
                "required": tool.input_schema.required,
            }),
        })
        .collect())
}

fn execute_mcp_call(
    workspace_root: &Path,
    server: &McpServerConfig,
    tool_name: &str,
    input: Value,
) -> Result<String, String> {
    let mut client = McpClient::spawn(workspace_root, server)?;
    client.initialize()?;
    let result = client.call_tool(tool_name, input)?;
    client.shutdown();
    serde_json::to_string(&result).map_err(|err| err.to_string())
}

#[derive(Debug, serde::Deserialize)]
struct McpToolSchema {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "inputSchema", alias = "input_schema")]
    input_schema: McpInputSchema,
}

#[derive(Debug, Default, serde::Deserialize)]
struct McpInputSchema {
    #[serde(default)]
    properties: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    required: Option<Vec<String>>,
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn(workspace_root: &Path, server: &McpServerConfig) -> Result<Self, String> {
        let mut command = Command::new(&server.command);
        command
            .args(&server.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let working_directory = server
            .cwd
            .clone()
            .unwrap_or_else(|| workspace_root.to_path_buf());
        command.current_dir(working_directory);
        for (key, value) in &server.env {
            command.env(key, value);
        }

        let mut child = command
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
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    fn initialize(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "nca",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )?;
        self.notify("notifications/initialized", Value::Object(Default::default()))?;
        Ok(())
    }

    fn list_tools(&mut self) -> Result<Vec<McpToolSchema>, String> {
        let result = self.request("tools/list", serde_json::json!({}))?;
        serde_json::from_value(result.get("tools").cloned().unwrap_or_else(|| Value::Array(vec![])))
            .map_err(|err| format!("failed to decode MCP tool list: {err}"))
    }

    fn call_tool(&mut self, tool_name: &str, input: Value) -> Result<Value, String> {
        self.request(
            "tools/call",
            serde_json::json!({
                "name": tool_name,
                "arguments": input,
            }),
        )
    }

    fn shutdown(&mut self) {
        let _ = self.request("shutdown", Value::Null);
        let _ = self.notify("exit", Value::Null);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&value)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&value)?;
        loop {
            let response = self.read_message()?;
            if response.get("id") == Some(&Value::from(id)) {
                if let Some(error) = response.get("error") {
                    return Err(format!("MCP error: {}", error));
                }
                return Ok(response
                    .get("result")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())));
            }
        }
    }

    fn write_message(&mut self, value: &Value) -> Result<(), String> {
        let line = serde_json::to_string(value).map_err(|err| err.to_string())?;
        writeln!(self.stdin, "{line}").map_err(|err| err.to_string())?;
        self.stdin.flush().map_err(|err| err.to_string())
    }

    fn read_message(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        self.stdout.read_line(&mut line).map_err(|err| err.to_string())?;
        serde_json::from_str(&line).map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn loads_and_executes_stdio_mcp_tool() {
        let temp = tempdir().expect("tempdir");
        let server_path = compile_mock_mcp_server(temp.path());
        let config = McpServerConfig {
            name: "mock".into(),
            command: server_path.display().to_string(),
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
        };

        let tools = load_mcp_tools(temp.path(), &[config]).expect("load MCP tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].definition().name, "mcp__mock__echo");

        let result = tools[0]
            .execute(&ToolCall {
                id: "call-1".into(),
                name: "mcp__mock__echo".into(),
                input: json!({
                    "message": "hello"
                }),
            })
            .await;

        assert!(result.success, "tool should succeed: {:?}", result.error);
        assert!(result.output.contains("\"echoed\":\"hello\""));
    }

    fn compile_mock_mcp_server(dir: &Path) -> PathBuf {
        let source_path = dir.join("mock_mcp_server.rs");
        let binary_path = dir.join("mock_mcp_server");
        std::fs::write(&source_path, mock_server_source()).expect("write mock MCP source");

        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let output = std::process::Command::new(rustc)
            .arg("--edition=2021")
            .arg(&source_path)
            .arg("-o")
            .arg(&binary_path)
            .output()
            .expect("compile mock MCP server");

        assert!(
            output.status.success(),
            "failed to compile mock MCP server: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        binary_path
    }

    fn mock_server_source() -> &'static str {
        r#"
use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let message: String = line.clone();
        let id = extract_number_field(&message, "\"id\":");
        let method = extract_string_field(&message, "\"method\":\"").unwrap_or_default();

        match method.as_str() {
            "initialize" => {
                let response = format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{{\"tools\":{{}}}},\"serverInfo\":{{\"name\":\"mock\",\"version\":\"1.0.0\"}}}}}}",
                    id.unwrap_or(1)
                );
                writeln!(stdout, "{}", response).unwrap();
                stdout.flush().unwrap();
            }
            "tools/list" => {
                let response = format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"tools\":[{{\"name\":\"echo\",\"description\":\"Echo tool\",\"inputSchema\":{{\"type\":\"object\",\"properties\":{{\"message\":{{\"type\":\"string\"}}}},\"required\":[\"message\"]}}}}]}}}}",
                    id.unwrap_or(1)
                );
                writeln!(stdout, "{}", response).unwrap();
                stdout.flush().unwrap();
            }
            "tools/call" => {
                let message = extract_string_field(&message, "\"message\":\"").unwrap_or_default();
                let response = format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"echoed\":\"{}\"}}}}",
                    id.unwrap_or(1),
                    escape_json(&message)
                );
                writeln!(stdout, "{}", response).unwrap();
                stdout.flush().unwrap();
            }
            "shutdown" => {
                let response = format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{}}}}",
                    id.unwrap_or(1)
                );
                writeln!(stdout, "{}", response).unwrap();
                stdout.flush().unwrap();
                break;
            }
            _ => {}
        }
    }
}

fn extract_number_field(input: &str, marker: &str) -> Option<u64> {
    let rest = input.split(marker).nth(1)?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn extract_string_field(input: &str, marker: &str) -> Option<String> {
    let rest = input.split(marker).nth(1)?;
    let mut out = String::new();
    let mut escaped = false;
    for ch in rest.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => break,
            _ => out.push(ch),
        }
    }
    Some(out)
}

fn escape_json(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}
"#
    }
}
