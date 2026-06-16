//! Tests for the OpenRouter provider (now served by `OpenAiCompatProvider`).

#[cfg(test)]
mod tests {
    use crate::provider::openai_compat::{CompatProfile, OpenAiCompatProvider};
    use crate::provider::test_support::{collect_chunks, spawn_sse_server};
    use crate::provider::{Provider, StreamChunk};
    use nca_common::config::NcaConfig;
    use nca_common::message::Message;

    const OPENROUTER_PROFILE: CompatProfile = CompatProfile {
        name: "openrouter",
        endpoint_suffix: "v1/chat/completions",
        strip_reasoning: false,
    };

    #[tokio::test]
    async fn openrouter_provider_sends_optional_headers_and_streams_usage() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Router hello\"},\"index\":0,\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let base_url = spawn_sse_server(body, 200, |request| {
            assert_eq!(request.url(), "/v1/chat/completions");
            assert!(
                request
                    .headers()
                    .iter()
                    .any(|header| header.field.equiv("authorization")
                        && header.value.as_str() == "Bearer openrouter-test-key")
            );
            assert!(
                request
                    .headers()
                    .iter()
                    .any(|header| header.field.equiv("http-referer")
                        && header.value.as_str() == "https://nca.test")
            );
            assert!(
                request
                    .headers()
                    .iter()
                    .any(|header| header.field.equiv("x-title")
                        && header.value.as_str() == "Native CLI AI")
            );
        });

        let mut config = NcaConfig::default();
        config.provider.openrouter.api_key = Some("openrouter-test-key".into());
        config.provider.openrouter.base_url = base_url;
        config.provider.openrouter.site_url = Some("https://nca.test".into());
        config.provider.openrouter.app_name = Some("Native CLI AI".into());

        let mut extra = reqwest::header::HeaderMap::new();
        if let Some(url) = &config.provider.openrouter.site_url {
            extra.insert(
                reqwest::header::HeaderName::from_static("http-referer"),
                reqwest::header::HeaderValue::from_str(url).unwrap(),
            );
        }
        if let Some(name) = &config.provider.openrouter.app_name {
            extra.insert(
                reqwest::header::HeaderName::from_static("x-title"),
                reqwest::header::HeaderValue::from_str(name).unwrap(),
            );
        }

        let provider = OpenAiCompatProvider::from_config(
            &config.provider.openrouter,
            config.model.max_tokens,
            OPENROUTER_PROFILE,
            extra,
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
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(text) if text == "Router hello"));
        assert!(matches!(
            &chunks[1],
            StreamChunk::Usage {
                input_tokens: 9,
                output_tokens: 4,
                ..
            }
        ));
        assert!(matches!(chunks.last(), Some(StreamChunk::Done)));
    }
}
