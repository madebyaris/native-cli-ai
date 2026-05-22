//! Approval handlers for tool calls: IPC-based (interactive) and auto-deny (non-interactive).

use super::ApprovalPendingMap;
use nca_core::approval::{ApprovalHandler, ApprovalVerdict};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

/// IPC-based approval handler that waits for approve/deny commands from
/// connected clients (e.g. CLI over the session socket).
pub struct IpcApprovalHandler {
    pending: ApprovalPendingMap,
}

impl IpcApprovalHandler {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn pending(&self) -> ApprovalPendingMap {
        self.pending.clone()
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for IpcApprovalHandler {
    async fn resolve(
        &self,
        call: &nca_common::tool::ToolCall,
        _description: &str,
    ) -> ApprovalVerdict {
        let (tx, rx) = oneshot::channel();
        {
            let mut m = self.pending.lock().unwrap();
            m.insert(call.id.clone(), tx);
        }
        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(verdict)) => verdict,
            _ => {
                let mut m = self.pending.lock().unwrap();
                m.remove(&call.id);
                ApprovalVerdict::Denied
            }
        }
    }
}

/// Auto-deny handler for non-interactive sessions.
pub(super) struct AutoDenyHandler;

#[async_trait::async_trait]
impl ApprovalHandler for AutoDenyHandler {
    async fn resolve(
        &self,
        _call: &nca_common::tool::ToolCall,
        _description: &str,
    ) -> ApprovalVerdict {
        ApprovalVerdict::Denied
    }
}
