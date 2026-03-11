use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use super::ToolExecutor;

pub struct RunValidationTool {
    workspace_root: std::path::PathBuf,
}

impl RunValidationTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for RunValidationTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "run_validation".into(),
            description: "Run a safe build, test, or lint command inside the workspace".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "cwd": { "type": "string" },
                    "timeout_secs": { "type": "integer" }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let command = call.input["command"].as_str().unwrap_or("").trim();
        let cwd = call.input["cwd"].as_str().unwrap_or(".");
        let timeout_secs = call.input["timeout_secs"].as_u64().unwrap_or(120);

        if !is_safe_validation_command(command) {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("command is not an allowed validation command".into()),
            };
        }

        let full_cwd = self.workspace_root.join(cwd);
        let canonical_cwd = match full_cwd.canonicalize() {
            Ok(path) if path.starts_with(&self.workspace_root) => path,
            _ => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some("cwd is outside the workspace".into()),
                };
            }
        };

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-lc")
            .arg(command)
            .current_dir(&canonical_cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output =
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;

        let output = match output {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to run validation command: {err}")),
                };
            }
            Err(_) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "validation command timed out after {timeout_secs}s"
                    )),
                };
            }
        };

        let mut text = String::from_utf8_lossy(&output.stdout).to_string();
        if !output.stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        ToolResult {
            call_id: call.id.clone(),
            success: output.status.success(),
            output: text,
            error: None,
        }
    }
}

fn is_safe_validation_command(command: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "cargo build",
        "cargo test",
        "cargo check",
        "cargo clippy",
        "cargo fmt --check",
        "npm run build",
        "npm run test",
        "npm run lint",
        "pnpm build",
        "pnpm test",
        "pnpm lint",
        "yarn build",
        "yarn test",
        "yarn lint",
        "next build",
        "pytest",
        "go test",
    ];

    PREFIXES.iter().any(|prefix| command.starts_with(prefix))
}
