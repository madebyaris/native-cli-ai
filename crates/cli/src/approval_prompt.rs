use nca_common::tool::ToolCall;
use nca_core::approval::ApprovalHandler;
use std::sync::Arc;
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
