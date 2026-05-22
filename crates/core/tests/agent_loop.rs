#![allow(clippy::pedantic, dead_code, unused_imports)]

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nca_common::event::AgentEvent;
use nca_common::message::Message;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use nca_core::approval::{ApprovalHandler, ApprovalPolicy, ApprovalVerdict};
use nca_core::provider::{Provider, ProviderError, StreamChunk};
use nca_core::tools::ToolExecutor;
use serde_json::json;
use support::{ProviderScriptStep, ScriptedProvider, readonly_tools, test_agent};
use tokio::sync::mpsc;

struct EchoTool;

#[async_trait]
impl ToolExecutor for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "echo".into(),
            description: "Echo input".into(),
            parameters: json!({
                "type": "object",
                "properties": { "msg": { "type": "string" } },
                "required": ["msg"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let msg = call.input["msg"].as_str().unwrap_or("").to_string();
        ToolResult {
            call_id: call.id.clone(),
            success: true,
            output: msg,
            error: None,
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

fn tool_then_text_provider() -> ScriptedProvider {
    ScriptedProvider::new(vec![
        ProviderScriptStep::Chunks(vec![
            StreamChunk::ToolUse(ToolCall {
                id: "call-echo".into(),
                name: "echo".into(),
                input: json!({ "msg": "pong" }),
            }),
            StreamChunk::Done,
        ]),
        ProviderScriptStep::Chunks(vec![
            StreamChunk::TextDelta("done".into()),
            StreamChunk::Done,
        ]),
    ])
}

#[tokio::test]
async fn text_turn_completes_with_assistant_message() {
    let (mut agent, mut event_rx) = test_agent(
        Box::new(ScriptedProvider::single_text("hello world")),
        readonly_tools(),
        Default::default(),
    );
    agent.set_system_prompt("test");

    let text = agent
        .run_turn("hi", Path::new("."), &[])
        .await
        .expect("turn");
    assert_eq!(text, "hello world");

    let mut saw_tokens = false;
    while let Ok(ev) = event_rx.try_recv() {
        if matches!(ev, AgentEvent::TokensStreamed { .. }) {
            saw_tokens = true;
        }
    }
    assert!(saw_tokens);
}

#[tokio::test]
async fn tool_turn_executes_and_follows_up() {
    let mut tools = readonly_tools();
    tools.register(Box::new(EchoTool));

    let config = nca_common::config::NcaConfig::default();
    let (event_tx, mut event_rx) = mpsc::channel(64);
    let mut agent = nca_core::agent::AgentLoop::new(
        Box::new(tool_then_text_provider()),
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
    agent.set_system_prompt("test");

    let text = agent
        .run_turn("use echo", Path::new("."), &[])
        .await
        .expect("tool turn");
    assert_eq!(text, "done");

    let mut saw_tool_start = false;
    let mut saw_tool_done = false;
    while let Ok(ev) = event_rx.try_recv() {
        match ev {
            AgentEvent::ToolCallStarted { tool, .. } if tool == "echo" => saw_tool_start = true,
            AgentEvent::ToolCallCompleted { output, .. } if output.success => saw_tool_done = true,
            _ => {}
        }
    }
    assert!(saw_tool_start);
    assert!(saw_tool_done);
}

#[tokio::test]
async fn cancel_mid_stream_aborts_turn() {
    let (mut agent, mut event_rx) = test_agent(
        Box::new(ScriptedProvider::slow_text(
            "never finishes",
            Duration::from_secs(5),
        )),
        readonly_tools(),
        Default::default(),
    );
    agent.set_system_prompt("test");

    let cancel = agent.cancel_token();
    let turn = tokio::spawn(async move { agent.run_turn("stream", Path::new("."), &[]).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();

    let result = turn.await.expect("join");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("run cancelled"));

    let mut saw_error = false;
    while let Ok(ev) = event_rx.try_recv() {
        if matches!(ev, AgentEvent::Error { .. }) {
            saw_error = true;
        }
    }
    assert!(saw_error);
}

#[tokio::test]
async fn max_turns_cap_stops_after_budget() {
    let (mut agent, _event_rx) = {
        let (event_tx, event_rx) = mpsc::channel(64);
        let config = nca_common::config::NcaConfig::default();
        let mut tools = readonly_tools();
        tools.register(Box::new(EchoTool));
        let agent = nca_core::agent::AgentLoop::new(
            Box::new(tool_then_text_provider()),
            tools,
            ApprovalPolicy::new(config.permissions.clone()).with_handler(Arc::new(AutoApprove)),
            "test-model".into(),
            event_tx,
            1,
            8,
            0,
            None,
            Default::default(),
            Default::default(),
        );
        (agent, event_rx)
    };
    agent.set_system_prompt("test");

    let result = agent.run_turn("loop", Path::new("."), &[]).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("turn budget exceeded")
    );
}

#[tokio::test]
async fn max_tool_calls_per_turn_cap() {
    struct TwoToolProvider;

    #[async_trait]
    impl Provider for TwoToolProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model: &str,
            _workspace_root: &Path,
        ) -> Result<mpsc::Receiver<StreamChunk>, ProviderError> {
            let (tx, rx) = mpsc::channel(8);
            tokio::spawn(async move {
                for id in ["c1", "c2"] {
                    let _ = tx
                        .send(StreamChunk::ToolUse(ToolCall {
                            id: id.into(),
                            name: "echo".into(),
                            input: json!({ "msg": id }),
                        }))
                        .await;
                }
                let _ = tx.send(StreamChunk::Done).await;
            });
            Ok(rx)
        }
    }

    let config = nca_common::config::NcaConfig::default();
    let mut tools = readonly_tools();
    tools.register(Box::new(EchoTool));
    let (mut agent, _event_rx) = {
        let (event_tx, event_rx) = mpsc::channel(64);
        let agent = nca_core::agent::AgentLoop::new(
            Box::new(TwoToolProvider),
            tools,
            ApprovalPolicy::new(config.permissions.clone()).with_handler(Arc::new(AutoApprove)),
            "test-model".into(),
            event_tx,
            8,
            1,
            0,
            None,
            Default::default(),
            Default::default(),
        );
        (agent, event_rx)
    };
    agent.set_system_prompt("test");

    let result = agent.run_turn("two tools", Path::new("."), &[]).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("tool-call budget exceeded")
    );
}
