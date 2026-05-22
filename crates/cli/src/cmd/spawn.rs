//! Background session spawn handler.

use crate::cli::CliPermissionMode;
use crate::cmd::util::print_json;
use std::path::Path;
use std::path::PathBuf;

#[derive(serde::Serialize)]
pub(crate) struct SpawnCommandOutput {
    session_id: String,
    pid: u32,
    status_path: PathBuf,
    event_log_path: PathBuf,
    spawn_log_path: PathBuf,
    socket_path: PathBuf,
    permission_mode: String,
    safe_mode: bool,
}

pub async fn spawn_run(
    workspace_root: &Path,
    prompt: &str,
    model: Option<String>,
    safe: bool,
    permission_mode: CliPermissionMode,
    json: bool,
) -> anyhow::Result<()> {
    let session_id = format!("session-{}", chrono::Utc::now().timestamp_millis());
    let sessions_dir = workspace_root.join(".nca/sessions");
    std::fs::create_dir_all(&sessions_dir)?;
    let spawn_log = sessions_dir.join(format!("{session_id}.spawn.log"));
    let stdout = std::fs::File::create(&spawn_log)?;
    let stderr = stdout.try_clone()?;
    let exe = std::env::current_exe()?;

    let mut command = std::process::Command::new(exe);
    command
        .arg("run")
        .arg("--prompt")
        .arg(prompt)
        .arg("--stream")
        .arg("ndjson")
        .arg("--session-id")
        .arg(&session_id)
        .arg("--permission-mode")
        .arg(permission_mode.as_arg())
        .args(if safe { vec!["--safe"] } else { vec![] });

    if let Some(model) = model {
        command.arg("--model").arg(model);
    }

    let child = command.stdout(stdout).stderr(stderr).spawn()?;

    if json {
        print_json(
            &SpawnCommandOutput {
                session_id: session_id.clone(),
                pid: child.id(),
                status_path: sessions_dir.join(format!("{session_id}.json")),
                event_log_path: sessions_dir.join(format!("{session_id}.events.jsonl")),
                spawn_log_path: spawn_log,
                socket_path: nca_runtime::ipc::IpcServer::new(&session_id).socket_path(),
                permission_mode: permission_mode.as_arg().to_string(),
                safe_mode: safe,
            },
            false,
        )?;
    } else {
        println!("{session_id}");
    }
    Ok(())
}
