use assert_cmd::Command;
use chrono::{Duration, Utc};
use nca_common::message::Message;
use nca_common::session::{SessionMeta, SessionState, SessionStatus};
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_local_config(workspace: &Path) {
    write_local_config_contents(
        workspace,
        "[provider.minimax]\napi_key = \"test-key\"\n",
    );
}

fn write_local_config_contents(workspace: &Path, contents: &str) {
    let config_dir = workspace.join(".nca");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(config_dir.join("config.local.toml"), contents).expect("write local config");
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
            session_summary: None,
            orchestration: None,
        },
        messages: vec![Message::user("hello")],
        total_input_tokens: 0,
        total_output_tokens: 0,
        estimated_cost_usd: 0.0,
    };

    let json = serde_json::to_string_pretty(&session).expect("serialize session");
    fs::write(sessions_dir.join(format!("{id}.json")), json).expect("write session");
}

fn write_event_log(workspace: &Path, id: &str, lines: &str) {
    let sessions_dir = workspace.join(".nca").join("sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    fs::write(sessions_dir.join(format!("{id}.events.jsonl")), lines).expect("write event log");
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
        .code(10)
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
        .stdout(predicates::str::contains("session-newer  status=Cancelled"))
        .stdout(predicates::str::contains("session-older  status=Completed"));
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

#[test]
fn sessions_json_emits_sorted_machine_snapshots() {
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
        SessionStatus::Running,
    );

    let output = Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("sessions")
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions[0]["id"], "session-newer");
    assert_eq!(sessions[0]["status"], "running");
    assert_eq!(sessions[1]["id"], "session-older");
    assert!(payload["unreadable"].as_array().expect("unreadable array").is_empty());
}

#[test]
fn status_outputs_session_snapshot_json() {
    let temp = tempdir().expect("tempdir");
    let now = Utc::now();
    write_session(
        temp.path(),
        "session-status",
        now,
        "MiniMax-M2.5",
        SessionStatus::Completed,
    );

    let output = Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("status")
        .arg("session-status")
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(payload["id"], "session-status");
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["model"], "MiniMax-M2.5");
    assert_eq!(payload["estimated_cost_usd"], 0.0);
}

#[test]
fn cancel_json_updates_session_snapshot() {
    let temp = tempdir().expect("tempdir");
    let now = Utc::now();
    write_session(
        temp.path(),
        "session-cancel",
        now,
        "MiniMax-M2.5",
        SessionStatus::Running,
    );

    let output = Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("cancel")
        .arg("session-cancel")
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(payload["cancelled"], true);
    assert_eq!(payload["session"]["status"], "cancelled");
}

#[test]
fn attach_falls_back_to_enveloped_event_log() {
    let temp = tempdir().expect("tempdir");
    let now = Utc::now();
    write_session(
        temp.path(),
        "session-log",
        now,
        "MiniMax-M2.5",
        SessionStatus::Completed,
    );
    write_event_log(
        temp.path(),
        "session-log",
        "{\"id\":1,\"ts\":\"2026-03-14T00:00:00Z\",\"event\":{\"type\":\"SessionEnded\",\"reason\":\"Completed\"}}\n",
    );

    Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("attach")
        .arg("session-log")
        .arg("--json")
        .assert()
        .success()
        .stdout(predicates::str::contains("\"event\":{\"type\":\"SessionEnded\""));
}

#[test]
fn spawn_json_reports_machine_paths() {
    let temp = tempdir().expect("tempdir");

    let output = Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("spawn")
        .arg("--prompt")
        .arg("hello")
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).expect("json");
    let session_id = payload["session_id"].as_str().expect("session id");
    assert!(session_id.starts_with("session-"));
    assert!(
        payload["spawn_log_path"]
            .as_str()
            .expect("spawn log path")
            .ends_with(".spawn.log")
    );
    assert!(
        payload["event_log_path"]
            .as_str()
            .expect("event log path")
            .ends_with(".events.jsonl")
    );
}

#[test]
fn models_json_lists_all_provider_models() {
    let temp = tempdir().expect("tempdir");
    write_local_config_contents(
        temp.path(),
        r#"
[provider]
default = "openai"

[provider.minimax]
api_key = "minimax-key"

[provider.openai]
api_key = "openai-key"
model = "gpt-4o"

[provider.anthropic]
api_key = "anthropic-key"
model = "claude-3-7-sonnet-latest"

[provider.openrouter]
api_key = "openrouter-key"
model = "openai/gpt-4o-mini"
"#,
    );

    let output = Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("models")
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(payload["default_provider"], "OpenAI");
    assert_eq!(payload["default_model"], "gpt-4o");
    let provider_models = payload["provider_models"]
        .as_array()
        .expect("provider_models array");
    assert_eq!(provider_models.len(), 4);
    assert!(provider_models.iter().any(|entry| {
        entry["provider"] == "OpenAI" && entry["model"] == "gpt-4o" && entry["selected"] == true
    }));
}

#[test]
fn doctor_json_reports_provider_readiness_for_all_backends() {
    let temp = tempdir().expect("tempdir");
    write_local_config_contents(
        temp.path(),
        r#"
[provider]
default = "anthropic"

[provider.minimax]
api_key = "minimax-key"

[provider.anthropic]
api_key = "anthropic-key"
model = "claude-3-7-sonnet-latest"
"#,
    );

    let output = Command::cargo_bin("nca")
        .expect("binary")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("doctor")
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(payload["provider"], "Anthropic");
    assert_eq!(payload["default_model"], "claude-3-7-sonnet-latest");
    let providers = payload["providers"].as_array().expect("providers array");
    assert_eq!(providers.len(), 4);
    assert!(providers.iter().any(|entry| {
        entry["provider"] == "Anthropic"
            && entry["selected"] == true
            && entry["api_key_present"] == true
    }));
    assert!(providers.iter().any(|entry| {
        entry["provider"] == "OpenAI"
            && entry["selected"] == false
            && entry["api_key_present"] == false
    }));
}
