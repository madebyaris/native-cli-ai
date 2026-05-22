#![allow(clippy::pedantic, dead_code, unused_imports)]

mod support;

use nca_common::config::{CustomProviderConfig, NcaConfig, ProviderCompatibility, ProviderKind};
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use nca_core::provider::anthropic::AnthropicProvider;
use nca_core::provider::custom::CustomProvider;
use nca_core::provider::openai::OpenAiProvider;
use nca_core::provider::{Provider, StreamChunk};
use serde_json::json;
use support::{collect_chunks, spawn_sse_server};

#[tokio::test]
async fn anthropic_provider_streams_via_mock_http() {
    let body = concat!(
        "event: message_start\n",
        "data: {\"message\":{\"usage\":{\"input_tokens\":13}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello from Claude\"}}\n\n",
        "event: message_delta\n",
        "data: {\"usage\":{\"output_tokens\":5}}\n\n"
    )
    .to_string();
    let base_url = spawn_sse_server(body, 200, |request| {
        assert_eq!(request.url(), "/v1/messages");
    });

    let mut config = NcaConfig::default();
    config.provider.anthropic.api_key = Some("anthropic-test-key".into());
    config.provider.anthropic.base_url = base_url;

    let provider = AnthropicProvider::from_config(&config).expect("provider");
    let stream = provider
        .chat(
            &[Message::user("hello")],
            &[],
            "",
            std::path::Path::new("."),
        )
        .await
        .expect("chat stream");

    let chunks = collect_chunks(stream).await;
    assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Hello from Claude"));
    assert!(matches!(
        &chunks[1],
        StreamChunk::Usage {
            input_tokens: 13,
            output_tokens: 5
        }
    ));
    assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
}

#[tokio::test]
async fn openai_provider_streams_via_mock_http() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"},\"index\":0,\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7}}\n\n",
        "data: [DONE]\n\n"
    )
    .to_string();
    let base_url = spawn_sse_server(body, 200, |request| {
        assert_eq!(request.url(), "/v1/chat/completions");
    });

    let mut config = NcaConfig::default();
    config.provider.openai.api_key = Some("openai-test-key".into());
    config.provider.openai.base_url = base_url;

    let provider = OpenAiProvider::from_config(&config).expect("provider");
    let stream = provider
        .chat(
            &[Message::user("hello")],
            &[],
            "",
            std::path::Path::new("."),
        )
        .await
        .expect("chat stream");

    let chunks = collect_chunks(stream).await;
    assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Hello "));
    assert!(matches!(
        &chunks[1],
        StreamChunk::Usage {
            input_tokens: 11,
            output_tokens: 7
        }
    ));
}

#[tokio::test]
async fn custom_openai_compatible_streams_via_mock_http() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Custom \"},\"index\":0,\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\n",
        "data: [DONE]\n\n"
    )
    .to_string();
    let base_url = spawn_sse_server(body, 200, |request| {
        assert_eq!(request.url(), "/v1/chat/completions");
    });

    let mut config = NcaConfig::default();
    config.provider.default = ProviderKind::Custom;
    config.provider.custom = CustomProviderConfig {
        api_key: Some("custom-test-key".into()),
        base_url,
        compatibility: ProviderCompatibility::OpenAi,
        model: "custom-model".into(),
        ..Default::default()
    };

    let provider = CustomProvider::from_config(&config).expect("provider");
    let stream = provider
        .chat(
            &[Message::user("hello")],
            &[],
            "",
            std::path::Path::new("."),
        )
        .await
        .expect("chat stream");

    let chunks = collect_chunks(stream).await;
    assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Custom "));
}

#[tokio::test]
async fn custom_anthropic_compatible_streams_tool_via_mock_http() {
    let body = concat!(
        "event: content_block_start\n",
        "data: {\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"lookup\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"src\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {}\n\n"
    )
    .to_string();
    let base_url = spawn_sse_server(body, 200, |request| {
        assert_eq!(request.url(), "/v1/messages");
    });

    let mut config = NcaConfig::default();
    config.provider.default = ProviderKind::Custom;
    config.provider.custom = CustomProviderConfig {
        api_key: Some("custom-test-key".into()),
        base_url,
        compatibility: ProviderCompatibility::Anthropic,
        model: "custom-anthropic".into(),
        ..Default::default()
    };

    let provider = CustomProvider::from_config(&config).expect("provider");
    let stream = provider
        .chat(
            &[Message::user("hello")],
            &[ToolDefinition {
                name: "lookup".into(),
                description: "Lookup".into(),
                parameters: json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } }
                }),
            }],
            "",
            std::path::Path::new("."),
        )
        .await
        .expect("chat stream");

    let chunks = collect_chunks(stream).await;
    assert!(matches!(
        &chunks[0],
        StreamChunk::ToolUse(call) if call.name == "lookup" && call.input == json!({"path":"src"})
    ));
}
