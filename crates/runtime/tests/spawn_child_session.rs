//! Child session spawn lifecycle (mock provider, no real git worktree).

#![allow(clippy::pedantic, dead_code, unused_imports, unused_mut)]

use std::thread;
use std::time::Duration;

use nca_common::config::{
    CustomProviderConfig, NcaConfig, PermissionMode, ProviderCompatibility, ProviderKind,
};
use nca_common::event::AgentEvent;
use nca_runtime::supervisor::{ChildSessionConfig, spawn_child_session};
use tiny_http::{Header, Response, Server, StatusCode};
use tokio::sync::mpsc;
use tokio::time::timeout;

fn spawn_openai_mock_server(response_text: &str) -> String {
    let body = format!(
        concat!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}},\"index\":0,\"finish_reason\":null}}]}}\n\n",
            "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1}}}}\n\n",
            "data: [DONE]\n\n"
        ),
        text = response_text.replace('\\', "\\\\").replace('"', "\\\"")
    );

    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base_url = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => format!("http://{addr}"),
        other => panic!("unexpected addr: {other:?}"),
    };

    thread::spawn(move || {
        loop {
            let Ok(request) = server.recv() else {
                break;
            };
            if request.url().starts_with("/v1/models") {
                let response = Response::from_string(r#"{"data":[]}"#)
                    .with_status_code(StatusCode(200))
                    .with_header(
                        Header::from_bytes("Content-Type", "application/json")
                            .expect("content-type"),
                    );
                let _ = request.respond(response);
                continue;
            }
            assert_eq!(request.url(), "/v1/chat/completions");
            let response = Response::from_string(body)
                .with_status_code(StatusCode(200))
                .with_header(
                    Header::from_bytes("Content-Type", "text/event-stream").expect("content-type"),
                );
            let _ = request.respond(response);
            break;
        }
    });

    base_url
}

#[tokio::test]
async fn spawn_child_session_completes_with_mock_provider() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".nca/sessions")).expect("sessions dir");

    let base_url = spawn_openai_mock_server("subagent finished");

    let mut config = NcaConfig::default();
    config.provider.default = ProviderKind::Custom;
    config.provider.custom = CustomProviderConfig {
        api_key: Some("spawn-test-key".into()),
        base_url,
        compatibility: ProviderCompatibility::OpenAi,
        model: "mock-model".into(),
        ..Default::default()
    };
    config.permissions.mode = PermissionMode::BypassPermissions;
    config.memory.context.query_provider_models_api = false;
    config.memory.context.enable_auto_summarize = false;

    let (event_tx, mut event_rx) = mpsc::channel(32);

    let cfg = ChildSessionConfig {
        parent_session_id: "parent-sess-1".into(),
        task: "Summarize the parser module".into(),
        workspace_root: workspace.clone(),
        config,
        parent_summary: "[User]: fix parser\n\n".into(),
        use_worktree: false,
        focus_files: vec!["src/parser.rs".into()],
        skills: vec![],
    };

    let result = timeout(
        Duration::from_secs(30),
        spawn_child_session(cfg, Some(event_tx)),
    )
    .await
    .expect("spawn timeout")
    .expect("spawn ok");

    assert!(!result.child_session_id.is_empty());
    assert_eq!(result.status, "completed");
    assert!(result.output.contains("subagent finished"));
    assert_eq!(
        std::path::Path::new(&result.workspace)
            .canonicalize()
            .expect("canonicalize result workspace"),
        workspace.canonicalize().expect("canonicalize workspace")
    );
    assert!(result.branch.is_none());
    assert!(result.worktree_path.is_none());

    let mut saw_spawned = false;
    while let Ok(event) = event_rx.try_recv() {
        if let AgentEvent::ChildSessionSpawned {
            parent_session_id,
            child_session_id,
            task,
            ..
        } = event
        {
            assert_eq!(parent_session_id, "parent-sess-1");
            assert_eq!(child_session_id, result.child_session_id);
            assert!(task.contains("parser"));
            saw_spawned = true;
        }
    }
    assert!(saw_spawned, "expected ChildSessionSpawned event");
}

#[tokio::test]
async fn spawn_child_session_skips_worktree_when_not_git_repo() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".nca/sessions")).expect("sessions dir");

    let base_url = spawn_openai_mock_server("done");

    let mut config = NcaConfig::default();
    config.provider.default = ProviderKind::Custom;
    config.provider.custom = CustomProviderConfig {
        api_key: Some("spawn-test-key".into()),
        base_url,
        compatibility: ProviderCompatibility::OpenAi,
        model: "mock-model".into(),
        ..Default::default()
    };
    config.permissions.mode = PermissionMode::BypassPermissions;
    config.memory.context.query_provider_models_api = false;
    config.memory.context.enable_auto_summarize = false;

    let cfg = ChildSessionConfig {
        parent_session_id: "parent-2".into(),
        task: "noop".into(),
        workspace_root: workspace,
        config,
        parent_summary: String::new(),
        use_worktree: true,
        focus_files: vec![],
        skills: vec![],
    };

    let result = timeout(Duration::from_secs(30), spawn_child_session(cfg, None))
        .await
        .expect("timeout")
        .expect("spawn");

    assert_eq!(result.status, "completed");
    assert!(result.worktree_path.is_none());
}
