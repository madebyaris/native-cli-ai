//! Snapshot tests for provider request body shaping.
//!
//! These ensure that serialized HTTP bodies for MiniMax (Anthropic-compatible)
//! and OpenAI-compatible providers stay stable across refactors.

#![allow(clippy::pedantic, dead_code, unused_imports)]

use std::path::Path;

use nca_common::message::{Message, MessageContent, Role};
use nca_common::tool::ToolDefinition;
use nca_core::provider::anthropic_compat::anthropic_request_body;
use nca_core::provider::openai_compat::openai_request_body;
use serde_json::json;

fn sample_messages() -> Vec<Message> {
    vec![
        Message {
            role: Role::System,
            content: MessageContent::Text("You are nca, a Rust-native coding agent.".into()),
            tool_call_id: None,
            tool_calls: None,
        },
        Message {
            role: Role::User,
            content: MessageContent::Text("list the files in src/".into()),
            tool_call_id: None,
            tool_calls: None,
        },
    ]
}

fn sample_tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "list_files".into(),
        description: "List workspace files matching a glob.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "glob": { "type": "string" }
            },
            "required": ["glob"]
        }),
    }]
}

#[test]
fn anthropic_request_body_stable_shape() {
    let workspace = Path::new("/tmp/workspace");
    let body = anthropic_request_body(
        &sample_messages(),
        &sample_tools(),
        "MiniMax-M2.5",
        4096,
        0.7,
        workspace,
    )
    .expect("body builds");

    insta::assert_json_snapshot!("anthropic_request_body", body);
}

#[test]
fn openai_request_body_stable_shape() {
    let workspace = Path::new("/tmp/workspace");
    let body = openai_request_body(
        &sample_messages(),
        &sample_tools(),
        "gpt-4o-mini",
        4096,
        0.7,
        workspace,
    )
    .expect("body builds");

    insta::assert_json_snapshot!("openai_request_body", body);
}

#[test]
fn minimax_request_body_stable_shape() {
    let workspace = Path::new("/tmp/workspace");
    // MiniMax uses the Anthropic-compatible body builder with temperature=1.0
    // (required for extended-thinking / reasoning models).
    let body = anthropic_request_body(
        &sample_messages(),
        &sample_tools(),
        "MiniMax-M2.5",
        4096,
        1.0,
        workspace,
    )
    .expect("body builds");

    insta::assert_json_snapshot!("minimax_request_body", body);
}
