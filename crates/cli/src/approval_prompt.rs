use nca_common::tool::ToolCall;
use nca_core::approval::ApprovalHandler;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct StdioApprovalHandler {
    prompt_lock: tokio::sync::Mutex<()>,
}

impl StdioApprovalHandler {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            prompt_lock: tokio::sync::Mutex::new(()),
        })
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for StdioApprovalHandler {
    async fn resolve(&self, call: &ToolCall, description: &str) -> bool {
        let _guard = self.prompt_lock.lock().await;
        let mut stderr = io::stderr();
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin);

        let prompt = format!(
            "\n[approval] {description}\nTool: {}\nInput: {}\nApprove? [y/N]: ",
            call.name, call.input
        );
        if stderr.write_all(prompt.as_bytes()).await.is_err() {
            return false;
        }
        if stderr.flush().await.is_err() {
            return false;
        }

        let mut answer = String::new();
        if reader.read_line(&mut answer).await.is_err() {
            return false;
        }

        matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}

/// Approval handler that receives ApproveToolCall/DenyToolCall from IPC (monitor).
/// Used when the runtime has an IPC server so the monitor can approve/deny.
pub struct IpcApprovalHandler {
    pending: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
}

impl IpcApprovalHandler {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn pending(&self) -> Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>> {
        self.pending.clone()
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for IpcApprovalHandler {
    async fn resolve(&self, call: &ToolCall, description: &str) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut m = self.pending.lock().unwrap();
            m.insert(call.id.clone(), tx);
        }
        // Wait for IPC command or timeout and fall back to stdio
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                {
                    let mut m = self.pending.lock().unwrap();
                    m.remove(&call.id);
                }
                let stdio = StdioApprovalHandler::new();
                stdio.resolve(call, description).await
            }
        }
    }
}

pub struct AutoDenyApprovalHandler;

impl AutoDenyApprovalHandler {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for AutoDenyApprovalHandler {
    async fn resolve(&self, _call: &ToolCall, _description: &str) -> bool {
        false
    }
}
