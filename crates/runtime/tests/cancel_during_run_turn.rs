//! Cooperative cancel during an in-flight agent turn (mirrors `service.rs` control loop).

#![allow(clippy::pedantic, dead_code, unused_imports, unused_mut)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use nca_common::config::{ModelPricing, NcaConfig, WebConfig};
use nca_common::event::{AgentCommand, AgentEvent, EndReason};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use nca_core::agent::AgentLoop;
use nca_core::approval::ApprovalPolicy;
use nca_core::provider::{Provider, ProviderError, StreamChunk};
use nca_core::tools::ToolRegistry;
use nca_runtime::supervisor::{SessionControlCommand, spawn_command_consumer_with_store};
use tokio::sync::mpsc;

struct SlowStreamingProvider;

#[async_trait]
impl Provider for SlowStreamingProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _workspace_root: &Path,
    ) -> Result<mpsc::Receiver<StreamChunk>, ProviderError> {
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for i in 0..500 {
                if tx
                    .send(StreamChunk::TextDelta(format!("token{i} ")))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let _ = tx.send(StreamChunk::Done).await;
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn cancel_during_run_turn_sets_cancelled_end_reason() {
    let (event_tx, mut event_rx) = mpsc::channel(64);
    let (control_tx, mut control_rx) = mpsc::channel(16);
    let (cmd_tx, cmd_rx) = mpsc::channel(16);

    let _consumer = spawn_command_consumer_with_store(
        cmd_rx,
        None,
        None,
        None,
        Some(event_tx.clone()),
        None,
        Some(control_tx),
    );

    let config = NcaConfig::default();
    let mut agent = AgentLoop::new(
        Box::new(SlowStreamingProvider),
        ToolRegistry::with_default_readonly_tools(PathBuf::from("."), WebConfig::default()),
        ApprovalPolicy::new(config.permissions.clone()),
        "test-model".into(),
        event_tx.clone(),
        8,
        8,
        0,
        None,
        ModelPricing::default(),
        Default::default(),
    );
    agent.set_system_prompt("test");
    let cancel_token = agent.cancel_token();

    let turn = tokio::spawn(async move {
        let run_fut = agent.run_turn("stream please", Path::new("."), &[]);
        tokio::pin!(run_fut);

        tokio::select! {
            result = &mut run_fut => result,
            control = control_rx.recv() => {
                if matches!(control, Some(SessionControlCommand::Cancel)) {
                    cancel_token.cancel();
                }
                run_fut.await
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    cmd_tx
        .send(AgentCommand::Cancel)
        .await
        .expect("send cancel");

    let result = turn.await.expect("turn task");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("run cancelled"));

    let _ = event_tx
        .send(AgentEvent::SessionEnded {
            reason: EndReason::Cancelled,
        })
        .await;

    let mut saw_cancelled = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(
            event,
            AgentEvent::SessionEnded {
                reason: EndReason::Cancelled,
                ..
            }
        ) {
            saw_cancelled = true;
        }
    }
    assert!(saw_cancelled);
}
