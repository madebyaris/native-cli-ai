//! MiniMax vision prep: `/v1/coding_plan/vlm` is mocked via tiny_http so image
//! attachments are materialized into text before the Anthropic-compatible chat call.

#![allow(clippy::pedantic, dead_code, unused_imports)]

use std::path::Path;
use std::thread;

use nca_common::message::{ContentPart, Message, MessageContent, Role};
use nca_core::provider::minimax_vlm::materialize_minimax_user_images;
use tiny_http::{Header, Response, Server, StatusCode};

fn spawn_vlm_server(response_json: &str) -> String {
    let server = Server::http("127.0.0.1:0").expect("start mock vlm server");
    let base_url = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => format!("http://{addr}"),
        other => panic!("unsupported listen addr: {other:?}"),
    };
    let body = response_json.to_string();

    thread::spawn(move || {
        let request = server.recv().expect("receive vlm request");
        assert_eq!(*request.method(), tiny_http::Method::Post);
        assert!(
            request.url().ends_with("/v1/coding_plan/vlm"),
            "unexpected path: {}",
            request.url()
        );
        let auth = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .expect("authorization header");
        assert_eq!(auth.value.as_str(), "Bearer test-vlm-key");

        let response = Response::from_string(body)
            .with_status_code(StatusCode(200))
            .with_header(
                Header::from_bytes("Content-Type", "application/json").expect("content-type"),
            );
        request.respond(response).expect("respond");
    });

    base_url
}

#[tokio::test]
async fn materialize_user_image_calls_coding_plan_vlm() {
    let temp = tempfile::tempdir().expect("tempdir");
    let image_rel = ".nca/attachments/test.png";
    let image_path = temp.path().join(image_rel);
    std::fs::create_dir_all(image_path.parent().expect("parent")).expect("mkdir");
    // Minimal valid 1x1 PNG
    let png: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    std::fs::write(&image_path, png).expect("write png");

    let api_origin = spawn_vlm_server(
        r#"{"base_resp":{"status_code":0,"status_msg":"success"},"content":"A terminal window showing nca output."}"#,
    );

    let messages = vec![Message {
        role: Role::User,
        content: MessageContent::Parts(vec![
            ContentPart::Text {
                text: "What is in this screenshot?".into(),
            },
            ContentPart::Image {
                media_type: "image/png".into(),
                path: image_rel.replace('\\', "/"),
            },
        ]),
        tool_call_id: None,
        tool_calls: None,
    }];

    let client = reqwest::Client::new();
    let out = materialize_minimax_user_images(
        &messages,
        temp.path(),
        &client,
        &api_origin,
        "test-vlm-key",
    )
    .await
    .expect("materialize");

    assert_eq!(out.len(), 1);
    let MessageContent::Text(text) = &out[0].content else {
        panic!("expected text-only user message");
    };
    assert!(text.contains("What is in this screenshot?"));
    assert!(text.contains("/v1/coding_plan/vlm"));
    assert!(text.contains("A terminal window showing nca output."));
}

#[tokio::test]
async fn materialize_skips_messages_without_images() {
    let messages = vec![Message::user("plain text")];
    let client = reqwest::Client::new();
    let out = materialize_minimax_user_images(
        &messages,
        Path::new("/tmp"),
        &client,
        "http://127.0.0.1:9",
        "unused",
    )
    .await
    .expect("no-op");
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].content, MessageContent::Text(_)));
}
