//! `todo_write`: replaces the session todo list and emits `TodosUpdated`.
//!
//! Persists to `<sessions_dir>/<session_id>.todos.json` (pretty JSON) and
//! broadcasts a full snapshot on the event bus. The tool intentionally does
//! NOT support partial edits; the model must provide the complete desired
//! list every call. This keeps state machine semantics trivial and avoids
//! divergence between the JSON file and the in-memory `TuiSessionState`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use nca_common::event::AgentEvent;
use nca_common::todo::{TodoItem, TodoList, TodoStatus};
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use tokio::sync::mpsc;

use super::ToolExecutor;

/// Shared handle so the supervisor / session writer can snapshot todos at
/// session-save time.
pub type TodoListHandle = Arc<Mutex<TodoList>>;

pub struct TodoWriteTool {
    event_tx: mpsc::Sender<AgentEvent>,
    sessions_dir: PathBuf,
    session_id: String,
    list: TodoListHandle,
}

impl TodoWriteTool {
    pub fn new(
        event_tx: mpsc::Sender<AgentEvent>,
        sessions_dir: PathBuf,
        session_id: String,
    ) -> Self {
        Self {
            list: Arc::new(Mutex::new(TodoList::new(session_id.clone()))),
            event_tx,
            sessions_dir,
            session_id,
        }
    }

    pub fn with_initial(
        event_tx: mpsc::Sender<AgentEvent>,
        sessions_dir: PathBuf,
        session_id: String,
        initial: TodoList,
    ) -> Self {
        Self {
            event_tx,
            sessions_dir,
            session_id,
            list: Arc::new(Mutex::new(initial)),
        }
    }

    pub fn handle(&self) -> TodoListHandle {
        self.list.clone()
    }

    fn todos_path(&self) -> PathBuf {
        self.sessions_dir
            .join(format!("{}.todos.json", self.session_id))
    }

    fn parse_status(s: &str) -> Result<TodoStatus, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pending" => Ok(TodoStatus::Pending),
            "in_progress" | "inprogress" | "active" => Ok(TodoStatus::InProgress),
            "completed" | "done" => Ok(TodoStatus::Completed),
            "cancelled" | "canceled" => Ok(TodoStatus::Cancelled),
            other => Err(format!(
                "unknown status `{other}` (expected pending|in_progress|completed|cancelled)"
            )),
        }
    }

    fn parse_items(input: &serde_json::Value) -> Result<Vec<TodoItem>, String> {
        let arr = input
            .get("todos")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "`todos` must be an array".to_string())?;
        if arr.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(arr.len());
        let mut seen = std::collections::HashSet::new();
        for (idx, raw) in arr.iter().enumerate() {
            let id = raw
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("todos[{idx}].id is required"))?;
            if !seen.insert(id.to_string()) {
                return Err(format!("duplicate todo id `{id}` at index {idx}"));
            }
            let content = raw
                .get("content")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("todos[{idx}].content is required"))?;
            let status = raw
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending");
            let status =
                Self::parse_status(status).map_err(|e| format!("todos[{idx}].status: {e}"))?;
            out.push(TodoItem::new(id, content, status));
        }
        let in_progress = out
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count();
        if in_progress > 1 {
            return Err(format!(
                "only one todo may be `in_progress` at a time (found {in_progress})"
            ));
        }
        Ok(out)
    }

    async fn persist(&self, snapshot: &TodoList) -> Result<(), String> {
        tokio::fs::create_dir_all(&self.sessions_dir)
            .await
            .map_err(|e| format!("create sessions dir: {e}"))?;
        let json =
            serde_json::to_string_pretty(snapshot).map_err(|e| format!("serialize todos: {e}"))?;
        let path = self.todos_path();
        tokio::fs::write(&path, json)
            .await
            .map_err(|e| format!("write {}: {e}", path.display()))
    }
}

#[async_trait::async_trait]
impl ToolExecutor for TodoWriteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_write".into(),
            description: "Replace the session's todo list with the provided items. Always send \
                the COMPLETE list you want to persist; partial updates are not supported. Use \
                this for any task with 3+ steps, when the user provides multiple tasks, or when \
                you want to surface progress visibly. Order is preserved. At most one todo may \
                be `in_progress` at a time."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "Full ordered todo list to persist.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "Stable identifier for this todo across updates."
                                },
                                "content": {
                                    "type": "string",
                                    "description": "One-line description shown in UI."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed", "cancelled"],
                                    "description": "Current state. Only one may be in_progress."
                                }
                            },
                            "required": ["id", "content", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let new_items = match Self::parse_items(&call.input) {
            Ok(v) => v,
            Err(e) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(e),
                };
            }
        };

        let snapshot = {
            let mut guard = match self.list.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.replace_with(new_items);
            guard.clone()
        };

        if let Err(e) = self.persist(&snapshot).await {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("persist todos: {e}")),
            };
        }

        let _ = self
            .event_tx
            .send(AgentEvent::TodosUpdated {
                session_id: self.session_id.clone(),
                todos: snapshot.items.clone(),
            })
            .await;

        let summary = format!(
            "{} todos ({} in-progress, {} completed)",
            snapshot.len(),
            snapshot.in_progress_count(),
            snapshot.completed_count()
        );
        ToolResult {
            call_id: call.id.clone(),
            success: true,
            output: summary,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn call(id: &str, todos: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "todo_write".into(),
            input: serde_json::json!({ "todos": todos }),
        }
    }

    #[tokio::test]
    async fn rejects_duplicate_ids() {
        let tmp = tempdir().unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let tool = TodoWriteTool::new(tx, tmp.path().into(), "sess-1".into());
        let res = tool
            .execute(&call(
                "c1",
                serde_json::json!([
                    { "id": "a", "content": "first", "status": "pending" },
                    { "id": "a", "content": "dup",   "status": "pending" },
                ]),
            ))
            .await;
        assert!(!res.success);
        assert!(res.error.unwrap().contains("duplicate"));
    }

    #[tokio::test]
    async fn rejects_multiple_in_progress() {
        let tmp = tempdir().unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let tool = TodoWriteTool::new(tx, tmp.path().into(), "sess-1".into());
        let res = tool
            .execute(&call(
                "c1",
                serde_json::json!([
                    { "id": "a", "content": "A", "status": "in_progress" },
                    { "id": "b", "content": "B", "status": "in_progress" },
                ]),
            ))
            .await;
        assert!(!res.success);
        assert!(res.error.unwrap().contains("in_progress"));
    }

    #[tokio::test]
    async fn persists_and_emits_event() {
        let tmp = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        let tool = TodoWriteTool::new(tx, tmp.path().into(), "sess-99".into());
        let res = tool
            .execute(&call(
                "c1",
                serde_json::json!([
                    { "id": "a", "content": "task A", "status": "pending" },
                    { "id": "b", "content": "task B", "status": "in_progress" },
                ]),
            ))
            .await;
        assert!(res.success, "{:?}", res.error);

        let path = tmp.path().join("sess-99.todos.json");
        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        let parsed: TodoList = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.items.len(), 2);
        assert_eq!(parsed.items[1].status, TodoStatus::InProgress);

        let ev = rx.recv().await.expect("TodosUpdated emitted");
        match ev {
            AgentEvent::TodosUpdated { session_id, todos } => {
                assert_eq!(session_id, "sess-99");
                assert_eq!(todos.len(), 2);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
