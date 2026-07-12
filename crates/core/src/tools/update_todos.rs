//! Atomically replace the session todo list and broadcast `TodosUpdated`.

use nca_common::event::AgentEvent;
use nca_common::todo::{AgentTodo, TodoSource, TodoStatus};
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use super::ToolExecutor;

pub const MAX_TODOS: usize = 40;
pub const MAX_CONTENT_CHARS: usize = 500;
pub const MAX_ID_CHARS: usize = 64;

/// Shared authoritative todo list for a session.
pub type TodoStore = Arc<Mutex<Vec<AgentTodo>>>;

/// Tool that replaces the full session todo list in one call.
pub struct UpdateTodosTool {
    event_tx: mpsc::Sender<AgentEvent>,
    todos: TodoStore,
}

impl UpdateTodosTool {
    pub fn new(event_tx: mpsc::Sender<AgentEvent>, todos: TodoStore) -> Self {
        Self { event_tx, todos }
    }
}

fn parse_status(raw: &str) -> Result<TodoStatus, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "pending" => Ok(TodoStatus::Pending),
        "in_progress" | "in-progress" | "progress" => Ok(TodoStatus::InProgress),
        "completed" | "done" => Ok(TodoStatus::Completed),
        "cancelled" | "canceled" => Ok(TodoStatus::Cancelled),
        other => Err(format!(
            "invalid status `{other}` (expected pending|in_progress|completed|cancelled)"
        )),
    }
}

fn parse_source(raw: &str) -> Result<TodoSource, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "agent" => Ok(TodoSource::Agent),
        "plan" => Ok(TodoSource::Plan),
        "user" => Ok(TodoSource::User),
        other => Err(format!(
            "invalid source `{other}` (expected agent|plan|user)"
        )),
    }
}

/// Validate and normalize a replacement todo list.
pub fn validate_todos(input: &serde_json::Value) -> Result<Vec<AgentTodo>, String> {
    let arr = input
        .as_array()
        .ok_or_else(|| "todos must be an array".to_string())?;
    if arr.len() > MAX_TODOS {
        return Err(format!("at most {MAX_TODOS} todos allowed"));
    }

    let mut out = Vec::with_capacity(arr.len());
    let mut seen = HashSet::new();
    let mut in_progress = 0usize;

    for (idx, item) in arr.iter().enumerate() {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() {
            return Err(format!("todo[{idx}].id must be non-empty"));
        }
        if id.chars().count() > MAX_ID_CHARS {
            return Err(format!("todo[{idx}].id exceeds {MAX_ID_CHARS} characters"));
        }
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate todo id `{id}`"));
        }

        let content = item
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if content.is_empty() {
            return Err(format!("todo[{idx}].content must be non-empty"));
        }
        if content.chars().count() > MAX_CONTENT_CHARS {
            return Err(format!(
                "todo[{idx}].content exceeds {MAX_CONTENT_CHARS} characters"
            ));
        }

        let status = match item.get("status").and_then(|v| v.as_str()) {
            Some(s) => parse_status(s)?,
            None => TodoStatus::Pending,
        };
        if status == TodoStatus::InProgress {
            in_progress += 1;
        }

        let source = match item.get("source").and_then(|v| v.as_str()) {
            Some(s) => Some(parse_source(s)?),
            None => Some(TodoSource::Agent),
        };

        out.push(AgentTodo {
            id,
            content,
            status,
            source,
        });
    }

    if in_progress > 1 {
        return Err("at most one todo may be in_progress".into());
    }

    Ok(out)
}

#[async_trait::async_trait]
impl ToolExecutor for UpdateTodosTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "update_todos".into(),
            description: "Replace the session todo list atomically. Pass the complete current \
                list (not a patch). Use this to track multi-step work: keep at most one item \
                `in_progress`, mark finished items `completed`, and cancel abandoned items. \
                Prefer short stable ids and concise content."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "Full replacement list of todos.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "Stable unique id for this todo."
                                },
                                "content": {
                                    "type": "string",
                                    "description": "Short actionable description."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed", "cancelled"],
                                    "description": "Lifecycle status (default pending)."
                                },
                                "source": {
                                    "type": "string",
                                    "enum": ["agent", "plan", "user"],
                                    "description": "Optional provenance (default agent)."
                                }
                            },
                            "required": ["id", "content"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let todos = match validate_todos(&call.input["todos"]) {
            Ok(todos) => todos,
            Err(error) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(error),
                };
            }
        };

        {
            let mut guard = match self.todos.lock() {
                Ok(g) => g,
                Err(_) => {
                    return ToolResult {
                        call_id: call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some("todo store lock poisoned".into()),
                    };
                }
            };
            *guard = todos.clone();
        }

        if self
            .event_tx
            .send(AgentEvent::TodosUpdated {
                todos: todos.clone(),
            })
            .await
            .is_err()
        {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("failed to emit TodosUpdated (session ended?)".into()),
            };
        }

        let completed = todos
            .iter()
            .filter(|t| t.status == TodoStatus::Completed)
            .count();
        let in_progress = todos
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count();
        let pending = todos
            .iter()
            .filter(|t| t.status == TodoStatus::Pending)
            .count();

        ToolResult {
            call_id: call.id.clone(),
            success: true,
            output: format!(
                "Updated {} todos ({} pending, {} in_progress, {} completed)",
                todos.len(),
                pending,
                in_progress,
                completed
            ),
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_todos() -> serde_json::Value {
        serde_json::json!([
            {"id": "1", "content": "Inspect code", "status": "completed"},
            {"id": "2", "content": "Implement fix", "status": "in_progress"},
            {"id": "3", "content": "Add tests", "status": "pending"}
        ])
    }

    #[test]
    fn validate_accepts_valid_list() {
        let todos = validate_todos(&sample_todos()).unwrap();
        assert_eq!(todos.len(), 3);
        assert_eq!(todos[1].status, TodoStatus::InProgress);
    }

    #[test]
    fn validate_rejects_duplicate_ids() {
        let err = validate_todos(&serde_json::json!([
            {"id": "1", "content": "a"},
            {"id": "1", "content": "b"}
        ]))
        .unwrap_err();
        assert!(err.contains("duplicate"));
    }

    #[test]
    fn validate_rejects_empty_content() {
        let err = validate_todos(&serde_json::json!([{"id": "1", "content": "  "}])).unwrap_err();
        assert!(err.contains("content"));
    }

    #[test]
    fn validate_rejects_multiple_in_progress() {
        let err = validate_todos(&serde_json::json!([
            {"id": "1", "content": "a", "status": "in_progress"},
            {"id": "2", "content": "b", "status": "in_progress"}
        ]))
        .unwrap_err();
        assert!(err.contains("in_progress"));
    }

    #[test]
    fn validate_rejects_oversized_list() {
        let items: Vec<_> = (0..MAX_TODOS + 1)
            .map(|i| serde_json::json!({"id": i.to_string(), "content": format!("t{i}")}))
            .collect();
        let err = validate_todos(&serde_json::Value::Array(items)).unwrap_err();
        assert!(err.contains("at most"));
    }

    #[tokio::test]
    async fn execute_replaces_store_and_emits_event() {
        let (tx, mut rx) = mpsc::channel(4);
        let store: TodoStore = Arc::new(Mutex::new(Vec::new()));
        let tool = UpdateTodosTool::new(tx, store.clone());
        let call = ToolCall {
            id: "c1".into(),
            name: "update_todos".into(),
            input: serde_json::json!({"todos": sample_todos()}),
        };
        let result = tool.execute(&call).await;
        assert!(result.success, "{result:?}");
        assert_eq!(store.lock().unwrap().len(), 3);
        match rx.recv().await.expect("event") {
            AgentEvent::TodosUpdated { todos } => assert_eq!(todos.len(), 3),
            other => panic!("unexpected event {other:?}"),
        }
    }
}
