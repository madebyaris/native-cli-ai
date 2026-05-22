//! IPC socket framing, multi-client broadcast, monotonic envelope ids, shutdown.

#![allow(clippy::pedantic, dead_code, unused_imports, unused_mut)]

use std::time::Duration;

use chrono::{DateTime, Utc};
use nca_common::event::{
    AgentCommand, AgentEvent, BusyState, EndReason, EventEnvelope, ToolOutputStream,
};
use nca_common::tool::ToolResult;
use nca_runtime::ipc::{IpcClient, IpcServer};
use tokio::time::timeout;

fn pinned_ts() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

async fn with_isolated_runtime_dir<F, Fut>(f: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime_dir = temp.path().join("runtime");
    std::fs::create_dir_all(&runtime_dir).expect("mkdir runtime");
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
    }
    f().await;
    unsafe {
        std::env::remove_var("XDG_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn broadcast_delivers_newline_framed_envelopes_to_clients() {
    with_isolated_runtime_dir(|| async {
        let server = IpcServer::new("ipc-framing-test");
        let mut handle = server.start().await.expect("start server");

        let path = handle.socket_path().clone();
        let client_a = IpcClient::new(path.clone());
        let client_b = IpcClient::new(path);
        let mut rx_a = client_a.connect().await.expect("client a");
        let mut rx_b = client_b.connect().await.expect("client b");

        let env = EventEnvelope {
            schema_version: 1,
            id: 7,
            ts: Some(pinned_ts()),
            event: AgentEvent::TokensStreamed {
                delta: "chunk".into(),
            },
        };
        handle.broadcast(&env).await.expect("broadcast");

        let got_a = timeout(Duration::from_secs(2), rx_a.recv())
            .await
            .expect("timeout a")
            .expect("event a");
        let got_b = timeout(Duration::from_secs(2), rx_b.recv())
            .await
            .expect("timeout b")
            .expect("event b");

        assert_eq!(got_a.id, 7);
        assert_eq!(got_b.id, 7);
        assert!(matches!(
            got_a.event,
            AgentEvent::TokensStreamed { ref delta } if delta == "chunk"
        ));
    })
    .await;
}

#[tokio::test]
async fn envelope_ids_are_monotonic_on_disk_wire() {
    with_isolated_runtime_dir(|| async {
        let server = IpcServer::new("ipc-ids-test");
        let mut handle = server.start().await.expect("start");
        let mut rx = IpcClient::new(handle.socket_path().clone())
            .connect()
            .await
            .expect("connect");
        tokio::time::sleep(Duration::from_millis(50)).await;

        for id in 1..=3_u64 {
            handle
                .broadcast(&EventEnvelope {
                    schema_version: 1,
                    id,
                    ts: Some(pinned_ts()),
                    event: AgentEvent::BusyStateChanged {
                        state: BusyState::Streaming,
                    },
                })
                .await
                .expect("broadcast");
        }

        let mut ids = Vec::new();
        for _ in 0..3 {
            let env = timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout")
                .expect("envelope");
            ids.push(env.id);
        }
        assert_eq!(ids, vec![1, 2, 3]);
    })
    .await;
}

#[tokio::test]
async fn client_commands_roundtrip_through_socket() {
    with_isolated_runtime_dir(|| async {
        let server = IpcServer::new("ipc-cmd-test");
        let mut handle = server.start().await.expect("start");
        let client = IpcClient::new(handle.socket_path().clone());

        let send = tokio::spawn(async move {
            client
                .send_command(&AgentCommand::ApproveToolCall {
                    call_id: "call-42".into(),
                })
                .await
                .expect("send approve");
            client
                .send_command(&AgentCommand::Shutdown)
                .await
                .expect("send shutdown");
        });

        let approve = timeout(Duration::from_secs(2), handle.recv_command())
            .await
            .expect("timeout approve")
            .expect("approve cmd");
        assert!(matches!(
            approve,
            AgentCommand::ApproveToolCall { ref call_id } if call_id == "call-42"
        ));

        let shutdown = timeout(Duration::from_secs(2), handle.recv_command())
            .await
            .expect("timeout shutdown")
            .expect("shutdown cmd");
        assert!(matches!(shutdown, AgentCommand::Shutdown));

        send.await.expect("send task");
    })
    .await;
}

#[tokio::test]
async fn rich_event_envelope_deserializes_over_ipc() {
    with_isolated_runtime_dir(|| async {
        let server = IpcServer::new("ipc-rich-event");
        let mut handle = server.start().await.expect("start");
        let mut rx = IpcClient::new(handle.socket_path().clone())
            .connect()
            .await
            .expect("connect");
        tokio::time::sleep(Duration::from_millis(50)).await;

        handle
            .broadcast(&EventEnvelope {
                schema_version: 1,
                id: 99,
                ts: Some(pinned_ts()),
                event: AgentEvent::ToolOutputChunk {
                    call_id: "bash-1".into(),
                    stream: ToolOutputStream::Stderr,
                    data: "warning: demo".into(),
                },
            })
            .await
            .expect("broadcast");

        handle
            .broadcast(&EventEnvelope {
                schema_version: 1,
                id: 100,
                ts: Some(pinned_ts()),
                event: AgentEvent::ToolCallCompleted {
                    call_id: "bash-1".into(),
                    output: ToolResult {
                        call_id: "bash-1".into(),
                        success: true,
                        output: "ok".into(),
                        error: None,
                    },
                },
            })
            .await
            .expect("broadcast");

        handle
            .broadcast(&EventEnvelope {
                schema_version: 1,
                id: 101,
                ts: Some(pinned_ts()),
                event: AgentEvent::SessionEnded {
                    reason: EndReason::Completed,
                },
            })
            .await
            .expect("broadcast");

        let mut ids = Vec::new();
        while ids.len() < 3 {
            let env = timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout")
                .expect("event");
            ids.push(env.id);
        }
        assert_eq!(ids, vec![99, 100, 101]);
    })
    .await;
}
