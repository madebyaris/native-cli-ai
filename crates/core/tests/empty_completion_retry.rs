#![allow(clippy::pedantic, dead_code, unused_imports)]

mod support;

use std::path::Path;

use nca_common::config::ModelRetryConfig;
use nca_core::provider::{ProviderError, StreamChunk};
use support::{ProviderScriptStep, ScriptedProvider, readonly_tools, test_agent};

fn empty_then_success_provider() -> ScriptedProvider {
    ScriptedProvider::new(vec![
        ProviderScriptStep::Chunks(vec![StreamChunk::Done]),
        ProviderScriptStep::Chunks(vec![
            StreamChunk::TextDelta("finally".into()),
            StreamChunk::Done,
        ]),
    ])
}

#[tokio::test]
async fn empty_completion_retries_without_usage_chunk() {
    let retry = ModelRetryConfig {
        max_empty_response_retries: 1,
        empty_response_backoff_initial_ms: 1,
        empty_response_backoff_max_ms: 1,
    };

    let (mut agent, _event_rx) = test_agent(
        Box::new(empty_then_success_provider()),
        readonly_tools(),
        retry,
    );
    agent.set_system_prompt("test");

    let text = agent
        .run_turn("hello", Path::new("."), &[])
        .await
        .expect("retry should recover");
    assert_eq!(text, "finally");
}

#[tokio::test]
async fn empty_completion_exhausts_retries_and_errors() {
    let retry = ModelRetryConfig {
        max_empty_response_retries: 0,
        empty_response_backoff_initial_ms: 1,
        empty_response_backoff_max_ms: 1,
    };

    let (mut agent, _event_rx) = test_agent(
        Box::new(ScriptedProvider::new(vec![ProviderScriptStep::Chunks(
            vec![StreamChunk::Done],
        )])),
        readonly_tools(),
        retry,
    );
    agent.set_system_prompt("test");

    let result = agent.run_turn("hello", Path::new("."), &[]).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ProviderError::Other(_)));
}
