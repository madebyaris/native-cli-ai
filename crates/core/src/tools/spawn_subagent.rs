use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use tokio::sync::{mpsc, oneshot};

use super::ToolExecutor;
use super::skill_hints::RecentSkillHints;

/// Request sent from the tool to the runtime to spawn a child session.
#[derive(Debug)]
pub struct SpawnRequest {
    pub task: String,
    pub focus_files: Vec<String>,
    pub skills: Vec<String>,
    pub use_worktree: bool,
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
    recent_skills: RecentSkillHints,
}

impl SpawnSubagentTool {
    pub fn new(spawn_tx: mpsc::Sender<SpawnRequest>, recent_skills: RecentSkillHints) -> Self {
        Self {
            spawn_tx,
            recent_skills,
        }
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
                to child agents that work in isolated git worktrees. When the task needs \
                specialized guidance, include relevant skill names so the child can load them \
                with invoke_skill before starting. If skills are omitted, the most recently \
                loaded skill may be inherited automatically."
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
                    "skills": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of exact skill command names from the available skills manifest. The child loads them with invoke_skill before working if they apply."
                    },
                    "use_worktree": {
                        "type": "boolean",
                        "description": "If true, the sub-agent runs in an isolated git worktree branch. Defaults to true."
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

        let skills: Vec<String> = call.input["skills"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::trim))
                    .filter(|value| !value.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let skills = if skills.is_empty() {
            self.recent_skills.recent(1)
        } else {
            skills
        };

        let use_worktree = call.input["use_worktree"].as_bool().unwrap_or(true);

        let (reply_tx, reply_rx) = oneshot::channel();

        let req = SpawnRequest {
            task,
            focus_files,
            skills,
            use_worktree,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_exposes_optional_skills_array() {
        let (tx, _rx) = mpsc::channel(1);
        let tool = SpawnSubagentTool::new(tx, RecentSkillHints::default());

        let def = tool.definition();
        assert_eq!(def.name, "spawn_subagent");
        assert_eq!(def.parameters["properties"]["skills"]["type"], "array");
        assert_eq!(
            def.parameters["properties"]["skills"]["items"]["type"],
            "string"
        );
        assert!(def.description.contains("skill names"));
    }

    #[tokio::test]
    async fn execute_forwards_skills_in_spawn_request() {
        let (tx, mut rx) = mpsc::channel(1);
        let tool = SpawnSubagentTool::new(tx, RecentSkillHints::default());
        let call = ToolCall {
            id: "call-1".into(),
            name: "spawn_subagent".into(),
            input: serde_json::json!({
                "task": "Review the parser",
                "focus_files": ["src/lib.rs"],
                "skills": ["rust-refactor", "  ", "testing"],
                "use_worktree": false
            }),
        };

        let exec = tokio::spawn(async move { tool.execute(&call).await });
        let req = rx.recv().await.expect("spawn request");
        assert_eq!(req.task, "Review the parser");
        assert_eq!(req.focus_files, vec!["src/lib.rs"]);
        assert_eq!(req.skills, vec!["rust-refactor", "testing"]);
        assert!(!req.use_worktree);

        let _ = req.reply.send(SpawnResponse {
            child_session_id: "child-1".into(),
            status: "completed".into(),
            output: "done".into(),
            workspace: "/tmp/worktree".into(),
            branch: Some("nca/child-1".into()),
            worktree_path: Some("/tmp/worktree".into()),
        });

        let result = exec.await.expect("task join");
        assert!(result.success);
        assert!(result.output.contains("\"child_session_id\": \"child-1\""));
    }

    #[tokio::test]
    async fn execute_inherits_most_recent_skill_when_skills_are_omitted() {
        let (tx, mut rx) = mpsc::channel(1);
        let hints = RecentSkillHints::default();
        hints.record("review");
        let tool = SpawnSubagentTool::new(tx, hints);
        let call = ToolCall {
            id: "call-2".into(),
            name: "spawn_subagent".into(),
            input: serde_json::json!({
                "task": "Review the parser",
                "focus_files": ["src/lib.rs"],
                "use_worktree": false
            }),
        };

        let exec = tokio::spawn(async move { tool.execute(&call).await });
        let req = rx.recv().await.expect("spawn request");
        assert_eq!(req.skills, vec!["review"]);

        let _ = req.reply.send(SpawnResponse {
            child_session_id: "child-2".into(),
            status: "completed".into(),
            output: "done".into(),
            workspace: "/tmp/worktree".into(),
            branch: None,
            worktree_path: None,
        });

        let result = exec.await.expect("task join");
        assert!(result.success);
    }
}
