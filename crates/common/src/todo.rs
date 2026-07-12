//! Session todo items shared across runtime, tools, and TUI.

use serde::{Deserialize, Serialize};

/// Lifecycle status for a session todo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::InProgress => "◉",
            Self::Completed => "✓",
            Self::Cancelled => "✗",
        }
    }
}

/// Optional provenance for a todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoSource {
    #[default]
    Agent,
    Plan,
    User,
}

/// One item in the session task list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTodo {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub status: TodoStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<TodoSource>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_status_roundtrip() {
        for status in [
            TodoStatus::Pending,
            TodoStatus::InProgress,
            TodoStatus::Completed,
            TodoStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: TodoStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn agent_todo_defaults_status() {
        let raw = r#"{"id":"1","content":"do thing"}"#;
        let todo: AgentTodo = serde_json::from_str(raw).unwrap();
        assert_eq!(todo.status, TodoStatus::Pending);
        assert!(todo.source.is_none());
    }
}
