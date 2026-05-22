//! Unified IPC approval map for interactive CLI handlers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cli_prompts::{
    DisplayPrompt,
    prompts::{AbortReason, Confirmation},
};
use nca_common::tool::ToolCall;
use nca_core::approval::{ApprovalHandler, ApprovalVerdict};
use nca_core::ipc_pending::ApprovalPendingMap;
use tokio::sync::{Mutex as AsyncMutex, oneshot};

/// Pretty-print JSON with indentation for readability
pub fn format_json_pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Truncate long strings with ellipsis
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Interactive approval handler sharing the supervisor IPC pending map.
pub struct InteractiveIpcApprovalHandler {
    pending: ApprovalPendingMap,
    prompt_lock: AsyncMutex<()>,
}

impl InteractiveIpcApprovalHandler {
    pub fn new() -> (Arc<Self>, ApprovalPendingMap) {
        let pending: ApprovalPendingMap = Arc::new(Mutex::new(HashMap::new()));
        let handler = Arc::new(Self {
            pending: pending.clone(),
            prompt_lock: AsyncMutex::new(()),
        });
        (handler, pending)
    }

    pub fn pending(&self) -> ApprovalPendingMap {
        self.pending.clone()
    }

    fn prompt_approval(&self, call: &ToolCall, description: &str) -> Option<bool> {
        let tool_name = &call.name;
        let input_preview = truncate(&format_json_pretty(&call.input), 150);

        let prompt_msg = if description.is_empty() {
            format!(
                "Tool '{}' wants to execute:\n\nInput preview:\n{}",
                tool_name, input_preview
            )
        } else {
            format!(
                "{}\n\nTool: {}\nInput preview:\n{}",
                description, tool_name, input_preview
            )
        };

        let confirmed = Confirmation::new(&prompt_msg)
            .default_positive(false)
            .display();

        match confirmed {
            Ok(true) => Some(true),
            Ok(false) => Some(false),
            Err(AbortReason::Interrupt) | Err(AbortReason::Error(_)) => None,
        }
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for InteractiveIpcApprovalHandler {
    async fn resolve(&self, call: &ToolCall, description: &str) -> ApprovalVerdict {
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().expect("approval pending lock");
            map.insert(call.id.clone(), tx);
        }

        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(verdict)) => verdict,
            _ => {
                {
                    let mut map = self.pending.lock().expect("approval pending lock");
                    map.remove(&call.id);
                }

                let _guard = self.prompt_lock.lock().await;
                match self.prompt_approval(call, description) {
                    Some(true) => ApprovalVerdict::Approved,
                    _ => ApprovalVerdict::Denied,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn ipc_approve_resolves_pending_call() {
        let (handler, pending) = InteractiveIpcApprovalHandler::new();
        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            input: json!({"path": "README.md"}),
        };

        let resolve_task = tokio::spawn({
            let handler = handler.clone();
            async move { handler.resolve(&call, "read file").await }
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let mut map = pending.lock().expect("lock");
        let tx = map.remove("call-1").expect("pending entry");
        drop(map);
        let _ = tx.send(ApprovalVerdict::Approved);

        let verdict = resolve_task.await.expect("resolve task");
        assert!(matches!(verdict, ApprovalVerdict::Approved));
    }
}
