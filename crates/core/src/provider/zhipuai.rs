//! Tests for the ZhipuAI provider (now served by `OpenAiCompatProvider`).

#[cfg(test)]
mod tests {
    use crate::provider::openai_compat::{CompatProfile, OpenAiCompatProvider};
    use crate::provider::test_support::{collect_chunks, spawn_sse_server};
    use crate::provider::{Provider, StreamChunk};
    use nca_common::config::NcaConfig;
    use nca_common::message::Message;

    const ZHIPUAI_PROFILE: CompatProfile = CompatProfile {
        name: "zhipuai",
        endpoint_suffix: "chat/completions",
        strip_reasoning: false,
    };

    #[tokio::test]
    async fn zhipuai_provider_streams_text_and_usage() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"你好 \"},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"世界\"},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
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
            assert_eq!(auth.value.as_str(), "Bearer zhipuai-test-key");
        });

        let mut config = NcaConfig::default();
        config.provider.zhipuai.api_key = Some("zhipuai-test-key".into());
        config.provider.zhipuai.base_url = base_url;

        let provider = OpenAiCompatProvider::from_config(
            &config.provider.zhipuai,
            config.model.max_tokens,
            ZHIPUAI_PROFILE,
            reqwest::header::HeaderMap::new(),
        )
        .expect("provider");
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
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "你好 "));
        assert!(matches!(&chunks[1], StreamChunk::TextDelta(text) if text == "世界"));
        assert!(matches!(
            &chunks[2],
            StreamChunk::Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..
            }
        ));
        assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
    }
}
