#![allow(clippy::pedantic, dead_code, unused_imports)]

mod support;

use std::path::Path;

use nca_common::config::ModelRetryConfig;
use nca_common::event::AgentEvent;
use nca_core::provider::{ProviderError, StreamChunk};
use support::{ProviderScriptStep, ScriptedProvider, readonly_tools, test_agent};

#[tokio::test]
async fn stream_error_propagates_as_provider_error() {
    use async_trait::async_trait;
    use nca_common::message::Message;
    use nca_common::tool::ToolDefinition;
    use nca_core::provider::{Provider, ProviderError};
    use std::path::Path;
    use tokio::sync::mpsc;

    struct ErrorStreamingProvider;

    #[async_trait]
    impl Provider for ErrorStreamingProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model: &str,
            _workspace_root: &Path,
        ) -> Result<mpsc::Receiver<StreamChunk>, ProviderError> {
            let (tx, rx) = mpsc::channel(4);
            tokio::spawn(async move {
                let _ = tx
                    .send(StreamChunk::Error(ProviderError::RequestFailed(
                        "upstream disconnected".into(),
                    )))
                    .await;
            });
            Ok(rx)
        }
    }

    let (mut agent, mut event_rx) = test_agent(
        Box::new(ErrorStreamingProvider),
        readonly_tools(),
        Default::default(),
    );
    agent.set_system_prompt("test");

    let result = agent.run_turn("hello", Path::new("."), &[]).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("upstream disconnected")
    );

    let mut saw_error_event = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(event, AgentEvent::Error { .. }) {
            saw_error_event = true;
        }
    }
    assert!(saw_error_event, "expected AgentEvent::Error");
}

#[tokio::test]
async fn successful_scripted_provider_still_works() {
    let (mut agent, _event_rx) = test_agent(
        Box::new(ScriptedProvider::single_text("ok")),
        readonly_tools(),
        Default::default(),
    );
    agent.set_system_prompt("test");

    let text = agent
        .run_turn("hello", Path::new("."), &[])
        .await
        .expect("turn succeeds");
    assert_eq!(text, "ok");
}
