//! OpenAI provider — tests for the generic OpenAiCompatProvider with OpenAI profile.

#[cfg(test)]
mod tests {
    use nca_common::config::NcaConfig;
    use nca_common::message::Message;
    use nca_common::tool::ToolDefinition;
    use serde_json::json;

    use crate::provider::openai_compat::{CompatProfile, OpenAiCompatProvider};
    use crate::provider::test_support::{collect_chunks, spawn_sse_server};
    use crate::provider::{Provider, StreamChunk};

    const OPENAI_PROFILE: CompatProfile = CompatProfile {
        name: "OpenAI",
        endpoint_suffix: "v1/chat/completions",
        strip_reasoning: false,
    };

    fn build_provider(config: &NcaConfig) -> OpenAiCompatProvider {
        let extra = reqwest::header::HeaderMap::new();
        OpenAiCompatProvider::from_config(
            &config.provider.openai,
            config.model.max_tokens,
            OPENAI_PROFILE,
            extra,
        )
        .expect("provider")
    }

    #[tokio::test]
    async fn openai_provider_streams_text_tool_and_usage() {
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
            assert_eq!(request.url(), "/v1/chat/completions");
            let auth = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("authorization"))
                .expect("authorization header");
            assert_eq!(auth.value.as_str(), "Bearer openai-test-key");
        });

        let mut config = NcaConfig::default();
        config.provider.openai.api_key = Some("openai-test-key".into());
        config.provider.openai.base_url = base_url;

        let provider = build_provider(&config);
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
                std::path::Path::new("."),
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
}
