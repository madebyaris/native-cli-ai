//! Tests for the DeepSeek provider (now served by `OpenAiCompatProvider` with strip_reasoning).

#[cfg(test)]
mod tests {
    use crate::provider::openai_compat::{CompatProfile, OpenAiCompatProvider};
    use crate::provider::test_support::{collect_chunks, spawn_sse_server};
    use crate::provider::{Provider, StreamChunk};
    use nca_common::config::NcaConfig;
    use nca_common::message::Message;
    use nca_common::tool::ToolDefinition;
    use serde_json::json;
    use std::path::Path;

    const DEEPSEEK_PROFILE: CompatProfile = CompatProfile {
        name: "deepseek",
        endpoint_suffix: "chat/completions",
        strip_reasoning: true,
    };

    #[tokio::test]
    async fn deepseek_provider_streams_text_tool_and_usage() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"path\\\":\\\"\"}}]},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"src\\\"}\"}}]},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let base_url = spawn_sse_server(body, 200, |request| {
            assert_eq!(request.url(), "/chat/completions");
            let auth = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("authorization"))
                .expect("authorization header");
            assert_eq!(auth.value.as_str(), "Bearer deepseek-test-key");
        });

        let mut config = NcaConfig::default();
        config.provider.deepseek.api_key = Some("deepseek-test-key".into());
        config.provider.deepseek.base_url = base_url;

        let provider = OpenAiCompatProvider::from_config(
            &config.provider.deepseek,
            config.model.max_tokens,
            DEEPSEEK_PROFILE,
            reqwest::header::HeaderMap::new(),
        )
        .expect("provider");
        let stream = provider
            .chat(
                &[Message::user("hello")],
                &[ToolDefinition {
                    name: "lookup".into(),
                    description: "Lookup a path".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"}
                        }
                    }),
                }],
                "",
                Path::new("."),
            )
            .await
            .expect("chat stream");

        let chunks = collect_chunks(stream).await;
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Hello "));
        assert!(
            matches!(&chunks[1], StreamChunk::ToolUse(call) if call.id == "call_1" && call.name == "lookup" && call.input == json!({"path":"src"}))
        );
        assert!(matches!(
            &chunks[2],
            StreamChunk::Usage {
                input_tokens: 11,
                output_tokens: 7,
                ..
            }
        ));
        assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
    }

    #[tokio::test]
    async fn prepare_strips_reasoning_content() {
        let mut config = NcaConfig::default();
        config.provider.deepseek.api_key = Some("test".into());
        let provider = OpenAiCompatProvider::from_config(
            &config.provider.deepseek,
            config.model.max_tokens,
            DEEPSEEK_PROFILE,
            reqwest::header::HeaderMap::new(),
        )
        .expect("provider");

        let mut messages = vec![
            Message::user("hello"),
            Message::assistant("response").with_reasoning("thinking...".into()),
            Message::tool("call_1", "result"),
        ];

        provider
            .prepare_messages_for_request(&mut messages, Path::new("."))
            .await
            .expect("prepare");

        // reasoning_content must be stripped for DeepSeek
        assert!(messages[1].reasoning_content.is_none());
        // Other fields unchanged
        assert_eq!(messages[1].content.to_summary_text(), "response");
        // User/tool messages unaffected
        assert!(messages[0].reasoning_content.is_none());
        assert!(messages[2].reasoning_content.is_none());
    }
}
