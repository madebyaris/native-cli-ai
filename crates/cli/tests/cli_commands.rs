use assert_cmd::Command;
use chrono::{Duration, Utc};
use nca_common::message::Message;
use nca_common::session::{SessionMeta, SessionState, SessionStatus};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_local_config(workspace: &Path) {
    let config_dir = workspace.join(".nca");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(
        config_dir.join("config.local.toml"),
        "[provider.minimax]\napi_key = \"test-key\"\n",
    )
    .expect("write local config");
}

fn write_session(
    workspace: &Path,
    id: &str,
    updated_at: chrono::DateTime<Utc>,
    model: &str,
    status: SessionStatus,
) {
    let sessions_dir = workspace.join(".nca").join("sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");

    let session = SessionState {
        meta: SessionMeta {
            id: id.to_string(),
            created_at: updated_at - Duration::minutes(1),
            updated_at,
            workspace: workspace.to_path_buf(),
            model: model.to_string(),
            status,
            pid: None,
            socket_path: None,
            worktree_path: None,
            branch: None,
            base_branch: None,
            parent_session_id: None,
            child_session_ids: Vec::new(),
            inherited_summary: None,
            spawn_reason: None,
        },
        messages: vec![Message::user("hello")],
        total_input_tokens: 0,
        total_output_tokens: 0,
        estimated_cost_usd: 0.0,
    };

    let json = serde_json::to_string_pretty(&session).expect("serialize session");
    fs::write(sessions_dir.join(format!("{id}.json")), json).expect("write session");
}

#[test]
fn run_without_config_exits_nonzero() {
    let temp = tempdir().expect("tempdir");

    Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env_remove("MINIMAX_API_KEY")
        .arg("run")
        .arg("--prompt")
        .arg("hello")
        .arg("--stream")
        .arg("off")
        .assert()
        .failure()
        .stderr(predicates::str::contains("missing MiniMax API key"));
}

#[test]
fn sessions_lists_newest_saved_sessions_first_with_status() {
    let temp = tempdir().expect("tempdir");
    let now = Utc::now();

    write_session(
        temp.path(),
        "session-older",
        now - Duration::minutes(5),
        "MiniMax-M2.5",
        SessionStatus::Completed,
    );
    write_session(
        temp.path(),
        "session-newer",
        now,
        "MiniMax-M2.5",
        SessionStatus::Cancelled,
    );

    Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("sessions")
        .assert()
        .success()
        .stdout(predicates::str::contains("session-newer\tCancelled\nsession-older\tCompleted"));
}

#[test]
fn top_level_resume_uses_latest_session() {
    let temp = tempdir().expect("tempdir");
    let now = Utc::now();

    write_local_config(temp.path());
    write_session(
        temp.path(),
        "session-older",
        now - Duration::minutes(5),
        "MiniMax-M2.5",
        SessionStatus::Completed,
    );
    write_session(
        temp.path(),
        "session-newer",
        now,
        "MiniMax-M2.5-latest",
        SessionStatus::Completed,
    );

    Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("--resume")
        .write_stdin("/status\n/exit\n")
        .assert()
        .success()
        .stdout(predicates::str::contains("session=session-newer"));
}
