//! Integration tests for the `UiOverlay` modal FSM wired through session state.

#![allow(
    clippy::all,
    clippy::pedantic,
    dead_code,
    unused_imports,
    unused_variables
)]

use std::path::PathBuf;

use nca_tui::tui::overlay::UiOverlay;
use nca_tui::tui::state::TuiSessionState;

fn test_state() -> TuiSessionState {
    TuiSessionState::new(
        "test-session".into(),
        "MiniMax-M2.5".into(),
        "build".into(),
        "default".into(),
        PathBuf::from("."),
    )
}

#[test]
fn connect_modal_open_close_transitions() {
    let mut state = test_state();
    assert_eq!(state.overlay, UiOverlay::None);

    state.open_connect_modal();
    assert!(state.overlay.is_open());
    assert!(matches!(state.overlay, UiOverlay::ConnectModal { .. }));

    state.close_connect_modal();
    assert_eq!(state.overlay, UiOverlay::None);
}

#[test]
fn illegal_palette_while_connect_modal_stays_on_connect() {
    let mut state = test_state();
    state.open_connect_modal();
    state.open_command_palette();

    assert!(matches!(state.overlay, UiOverlay::ConnectModal { .. }));
    state.close_connect_modal();
    state.open_command_palette();
    assert!(matches!(state.overlay, UiOverlay::CommandPalette { .. }));
}

#[tokio::test]
async fn bridge_apply_event_updates_transcript_version() {
    use nca_common::event::AgentEvent;
    use nca_tui::tui::bridge::spawn_tui_bridge;
    use std::sync::{Arc, Mutex};
    use tokio::sync::{mpsc, watch};

    let state = Arc::new(Mutex::new(test_state()));
    let (event_tx, event_rx) = mpsc::channel(8);
    let (version_tx, _version_rx) = watch::channel(0u64);
    let log = tempfile::tempdir()
        .expect("tempdir")
        .path()
        .join("events.jsonl");

    let task = spawn_tui_bridge(
        event_rx,
        log,
        None,
        None,
        None,
        state.clone(),
        Some(version_tx),
    );

    let before = state.lock().expect("lock").transcript_version;
    event_tx
        .send(AgentEvent::MessageReceived {
            role: "assistant".into(),
            content: "hello from bridge".into(),
        })
        .await
        .expect("send event");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let after = state.lock().expect("lock").transcript_version;
    assert!(after > before, "bridge should bump transcript version");
    drop(event_tx);
    task.abort();
}
