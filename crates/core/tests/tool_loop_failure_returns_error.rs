#![allow(clippy::pedantic, dead_code, unused_imports)]

mod support;

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use nca_common::config::NcaConfig;
use nca_common::message::Message;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use nca_core::approval::{ApprovalHandler, ApprovalPolicy, ApprovalVerdict};
use nca_core::provider::{Provider, ProviderError, StreamChunk};
use nca_core::tools::{ToolExecutor, ToolRegistry};
use serde_json::json;
use support::{readonly_tools, test_agent};
use tokio::sync::mpsc;

struct AlwaysFailTool;

#[async_trait]
impl ToolExecutor for AlwaysFailTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "always_fail".into(),
            description: "always fails".into(),
            parameters: json!({"type": "object", "properties": {}}),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: String::new(),
            error: Some("boom".into()),
        }
    }
}

struct AutoApprove;

#[async_trait]
impl ApprovalHandler for AutoApprove {
    async fn resolve(&self, _call: &ToolCall, _description: &str) -> ApprovalVerdict {
        ApprovalVerdict::Approved
    }
}

struct RepeatToolCallProvider;

#[async_trait]
impl Provider for RepeatToolCallProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _workspace_root: &Path,
    ) -> Result<mpsc::Receiver<StreamChunk>, ProviderError> {
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let _ = tx
                .send(StreamChunk::ToolUse(ToolCall {
                    id: "call-1".into(),
                    name: "always_fail".into(),
                    input: json!({}),
                }))
                .await;
            let _ = tx.send(StreamChunk::Done).await;
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn three_consecutive_tool_failures_return_err_not_ok() {
    let mut tools = readonly_tools();
    tools.register(Box::new(AlwaysFailTool));

    let config = NcaConfig::default();
    let (mut agent, _event_rx) = {
        let (event_tx, event_rx) = mpsc::channel(64);
        let agent = nca_core::agent::AgentLoop::new(
            Box::new(RepeatToolCallProvider),
            tools,
            ApprovalPolicy::new(config.permissions.clone()).with_handler(Arc::new(AutoApprove)),
            "test-model".into(),
            event_tx,
            8,
            8,
            0,
            None,
            Default::default(),
            Default::default(),
        );
        (agent, event_rx)
    };
    agent.set_system_prompt("test");

    let result = agent.run_turn("trigger tool", Path::new("."), &[]).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("failed 3 times consecutively"));
}
