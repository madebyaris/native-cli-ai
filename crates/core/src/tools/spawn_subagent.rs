use nca_common::config::ProviderKind;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use tokio::sync::{mpsc, oneshot};

use super::ToolExecutor;

/// Request sent from the tool to the runtime to spawn a child session.
#[derive(Debug)]
pub struct SpawnRequest {
    pub task: String,
    pub focus_files: Vec<String>,
    pub use_worktree: bool,
    pub provider_override: Option<ProviderKind>,
    pub model_override: Option<String>,
    /// Optional specialist agent name (e.g. "explorer", "oracle").
    /// When set, the runtime loads the matching agent profile.
    pub specialist: Option<String>,
    pub reply: oneshot::Sender<SpawnResponse>,
}

/// Response from the runtime after spawning a child session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpawnResponse {
    pub child_session_id: String,
    pub status: String,
    pub output: String,
    pub workspace: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
}

pub struct SpawnSubagentTool {
    spawn_tx: mpsc::Sender<SpawnRequest>,
}

impl SpawnSubagentTool {
    pub fn new(spawn_tx: mpsc::Sender<SpawnRequest>) -> Self {
        Self { spawn_tx }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for SpawnSubagentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "spawn_subagent".into(),
            description: "Spawn a sub-agent that runs as a separate session to handle a specific \
                task in parallel. The sub-agent inherits your conversation context and workspace. \
                Use this to delegate independent tasks (e.g. creating files, running builds) \
                to child agents that work in isolated git worktrees."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "A clear, self-contained description of what the sub-agent should do."
                    },
                    "focus_files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of file paths the sub-agent should focus on."
                    },
                    "use_worktree": {
                        "type": "boolean",
                        "description": "If true, the sub-agent runs in an isolated git worktree branch. Defaults to true."
                    },
                    "provider": {
                        "type": "string",
                        "description": "Optional provider override for the sub-agent (e.g. \"openai\", \"anthropic\", \"deepseek\"). Routes the child session to a different LLM endpoint."
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model name override for the sub-agent (e.g. \"gpt-4o\"). Used in conjunction with provider or alone for alias-based routing."
                    },
                    "specialist": {
                        "type": "string",
                        "description": "Optional specialist agent name (e.g. 'explorer', 'oracle', 'fixer'). When set, the runtime loads the matching agent profile (provider, model, system prompt) automatically."
                    }
                },
                "required": ["task"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let task = call.input["task"].as_str().unwrap_or("").to_string();

        if task.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("task is required".into()),
            };
        }

        let focus_files: Vec<String> = call.input["focus_files"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let use_worktree = call.input["use_worktree"].as_bool().unwrap_or(true);

        // Parse optional provider/model overrides for per-agent routing.
        let provider_override = call.input["provider"]
            .as_str()
            .and_then(ProviderKind::from_cli_name);
        let model_override = call.input["model"].as_str().map(String::from);
        let specialist = call.input["specialist"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(String::from);

        let (reply_tx, reply_rx) = oneshot::channel();

        let req = SpawnRequest {
            task,
            focus_files,
            use_worktree,
            provider_override,
            model_override,
            specialist,
            reply: reply_tx,
        };

        if self.spawn_tx.send(req).await.is_err() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("Sub-agent spawner is not available".into()),
            };
        }

        match tokio::time::timeout(std::time::Duration::from_secs(600), reply_rx).await {
            Ok(Ok(response)) => {
                let output = serde_json::to_string_pretty(&response).unwrap_or_default();
                let success = response.status == "completed";
                ToolResult {
                    call_id: call.id.clone(),
                    success,
                    output,
                    error: if success {
                        None
                    } else {
                        Some(format!(
                            "Sub-agent finished with status: {}",
                            response.status
                        ))
                    },
                }
            }
            Ok(Err(_)) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("Sub-agent spawner dropped the reply channel".into()),
            },
            Err(_) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("Sub-agent timed out after 600 seconds".into()),
            },
        }
    }
}
