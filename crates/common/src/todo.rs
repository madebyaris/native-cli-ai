//! Session-scoped todo list model.
//!
//! Persisted alongside session snapshots as `<id>.todos.json` and broadcast
//! over IPC via [`crate::event::AgentEvent::TodosUpdated`]. Rendered in the
//! TUI sidebar and the REPL delta summary. The todo_write tool is the only
//! writer; downstream consumers should treat [`TodoList::items`] as the
//! authoritative snapshot after every update.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle states a todo can be in.
///
/// `InProgress` maps to "currently being worked on". By convention only one
/// todo should be `InProgress` at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    /// One-character glyph used by sidebar / REPL delta renderers.
    #[must_use]
    pub fn glyph(&self) -> char {
        match self {
            TodoStatus::Pending => '○',
            TodoStatus::InProgress => '◐',
            TodoStatus::Completed => '●',
            TodoStatus::Cancelled => '✗',
        }
    }

    /// Lowercase label for JSON-friendly logs / machine streams.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Cancelled => "cancelled",
        }
    }
}

/// A single todo entry.
///
/// `id` must be stable across updates so renderers can diff between snapshots.
/// `created_at` / `updated_at` are stamped by the runtime, not the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

impl TodoItem {
    #[must_use]
    pub fn new(id: impl Into<String>, content: impl Into<String>, status: TodoStatus) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            content: content.into(),
            status,
            created_at: now,
            updated_at: now,
        }
    }
}

/// The full, ordered todo list for a session.
///
/// Serialized as a single JSON object with `session_id`, `updated_at`, and
/// `items`. Order is preserved and meaningful: renderers display the list in
/// the order items were written so the model controls grouping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoList {
    pub session_id: String,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub items: Vec<TodoItem>,
}

impl TodoList {
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            updated_at: Utc::now(),
            items: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Count todos currently in `InProgress`.
    #[must_use]
    pub fn in_progress_count(&self) -> usize {
        self.items
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count()
    }

    /// Count todos marked `Completed`.
    #[must_use]
    pub fn completed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|t| t.status == TodoStatus::Completed)
            .count()
    }

    /// Replace `items` with `new_items` in the order provided, preserving
    /// `created_at` for items whose `id` already existed and stamping
    /// `updated_at` whenever status/content changes.
    pub fn replace_with(&mut self, new_items: Vec<TodoItem>) {
        let now = Utc::now();
        let mut existing: std::collections::HashMap<String, TodoItem> =
            self.items.drain(..).map(|t| (t.id.clone(), t)).collect();

        self.items = new_items
            .into_iter()
            .map(|mut incoming| {
                if let Some(prev) = existing.remove(&incoming.id) {
                    incoming.created_at = prev.created_at;
                    if prev.status != incoming.status || prev.content != incoming.content {
                        incoming.updated_at = now;
                    } else {
                        incoming.updated_at = prev.updated_at;
                    }
                } else {
                    incoming.created_at = now;
                    incoming.updated_at = now;
                }
                incoming
            })
            .collect();

        self.updated_at = now;
    }

    /// Deltas between `self` (previous snapshot) and `new_items` (incoming), for
    /// REPL delta rendering.
    #[must_use]
    pub fn diff<'a>(&'a self, new_items: &'a [TodoItem]) -> Vec<TodoDelta<'a>> {
        let mut out = Vec::new();
        let prev_by_id: std::collections::HashMap<&str, &TodoItem> =
            self.items.iter().map(|t| (t.id.as_str(), t)).collect();
        let new_by_id: std::collections::HashMap<&str, &TodoItem> =
            new_items.iter().map(|t| (t.id.as_str(), t)).collect();

        for item in new_items {
            match prev_by_id.get(item.id.as_str()) {
                None => out.push(TodoDelta::Added(item)),
                Some(prev) if prev.status != item.status => {
                    out.push(TodoDelta::StatusChanged {
                        prev: prev.status,
                        next: item.status,
                        item,
                    });
                }
                Some(prev) if prev.content != item.content => {
                    out.push(TodoDelta::ContentChanged { prev, next: item });
                }
                _ => {}
            }
        }
        for item in &self.items {
            if !new_by_id.contains_key(item.id.as_str()) {
                out.push(TodoDelta::Removed(item));
            }
        }
        out
    }
}

/// A single change between two consecutive todo snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoDelta<'a> {
    Added(&'a TodoItem),
    Removed(&'a TodoItem),
    StatusChanged {
        prev: TodoStatus,
        next: TodoStatus,
        item: &'a TodoItem,
    },
    ContentChanged {
        prev: &'a TodoItem,
        next: &'a TodoItem,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: &str, status: TodoStatus, content: &str) -> TodoItem {
        TodoItem::new(id, content, status)
    }

    #[test]
    fn replace_preserves_created_at_and_stamps_updates() {
        let mut list = TodoList::new("sess-1");
        list.replace_with(vec![
            mk("a", TodoStatus::Pending, "A"),
            mk("b", TodoStatus::Pending, "B"),
        ]);
        let a_created = list.items[0].created_at;

        std::thread::sleep(std::time::Duration::from_millis(2));

        list.replace_with(vec![
            mk("a", TodoStatus::InProgress, "A"),
            mk("b", TodoStatus::Pending, "B"),
        ]);
        assert_eq!(list.items[0].created_at, a_created);
        assert!(list.items[0].updated_at > a_created);
        assert_eq!(list.items[1].updated_at, list.items[1].created_at);
    }

    #[test]
    fn diff_detects_adds_removes_and_status_changes() {
        let mut prev = TodoList::new("s");
        prev.replace_with(vec![
            mk("a", TodoStatus::Pending, "A"),
            mk("b", TodoStatus::InProgress, "B"),
        ]);
        let next = vec![
            mk("a", TodoStatus::Completed, "A"),
            mk("c", TodoStatus::Pending, "C"),
        ];
        let deltas = prev.diff(&next);
        assert!(matches!(deltas[0], TodoDelta::StatusChanged { .. }));
        assert!(matches!(deltas[1], TodoDelta::Added(_)));
        assert!(matches!(deltas[2], TodoDelta::Removed(_)));
    }

    #[test]
    fn counts_reflect_current_statuses() {
        let mut list = TodoList::new("s");
        list.replace_with(vec![
            mk("a", TodoStatus::Completed, "A"),
            mk("b", TodoStatus::InProgress, "B"),
            mk("c", TodoStatus::Pending, "C"),
        ]);
        assert_eq!(list.completed_count(), 1);
        assert_eq!(list.in_progress_count(), 1);
    }
}
