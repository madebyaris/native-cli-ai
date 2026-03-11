mod approval_prompt;
mod app;
mod prompt;
mod render;
mod repl;
mod runner;
mod runtime_tooling;
mod stream;

use clap::Parser;
use nca_common::config::{NcaConfig, PermissionMode};
use nca_common::event::AgentCommand;
use nca_common::event::EndReason;
use repl::Repl;
use runner::{build_resumed_session_runtime, build_session_runtime};
use std::path::PathBuf;
use stream::{spawn_stream_task, StreamMode};

#[derive(Parser, Debug)]
#[command(name = "nca", about = "Native CLI AI - a Rust-powered coding assistant")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// One-shot prompt mode
    #[arg(short, long)]
    prompt: Option<String>,

    /// Start in read-only safe mode
    #[arg(short, long)]
    safe: bool,

    /// Resume the last session
    #[arg(short, long)]
    resume: bool,

    /// Override the default model
    #[arg(long)]
    model: Option<String>,

    /// Enable extended thinking
    #[arg(short = 't', long)]
    enable_thinking: bool,

    /// Token budget for extended thinking
    #[arg(long, default_value = "5120")]
    thinking_budget: u32,

    /// Max response tokens
    #[arg(long, default_value = "8192")]
    max_tokens: u32,

    /// Verbose debug logging
    #[arg(short, long)]
    verbose: bool,

    /// Output structured JSON (for CI)
    #[arg(long)]
    json: bool,

    /// Streaming output format
    #[arg(long, value_enum, default_value_t = StreamMode::Human)]
    stream: StreamMode,

    /// Permission handling mode
    #[arg(long, value_enum, default_value_t = CliPermissionMode::Default)]
    permission_mode: CliPermissionMode,

    /// Internal session identifier for spawned runs
    #[arg(long, hide = true)]
    session_id: Option<String>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    Run {
        #[arg(long)]
        prompt: String,
        #[arg(long, value_enum, default_value_t = StreamMode::Human)]
        stream: StreamMode,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        safe: bool,
        #[arg(long, value_enum, default_value_t = CliPermissionMode::Default)]
        permission_mode: CliPermissionMode,
        #[arg(long, hide = true)]
        session_id: Option<String>,
    },
    Spawn {
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        safe: bool,
        #[arg(long, value_enum, default_value_t = CliPermissionMode::DontAsk)]
        permission_mode: CliPermissionMode,
    },
    Sessions,
    Resume {
        session_id: String,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long)]
        safe: bool,
        #[arg(long, value_enum, default_value_t = StreamMode::Human)]
        stream: StreamMode,
        #[arg(long, value_enum, default_value_t = CliPermissionMode::Default)]
        permission_mode: CliPermissionMode,
    },
    Logs {
        session_id: String,
        #[arg(long)]
        follow: bool,
    },
    Attach {
        session_id: String,
    },
    Status {
        session_id: String,
    },
    Cancel {
        session_id: String,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum CliPermissionMode {
    Default,
    Plan,
    AcceptEdits,
    DontAsk,
    BypassPermissions,
}

impl From<CliPermissionMode> for PermissionMode {
    fn from(value: CliPermissionMode) -> Self {
        match value {
            CliPermissionMode::Default => Self::Default,
            CliPermissionMode::Plan => Self::Plan,
            CliPermissionMode::AcceptEdits => Self::AcceptEdits,
            CliPermissionMode::DontAsk => Self::DontAsk,
            CliPermissionMode::BypassPermissions => Self::BypassPermissions,
        }
    }
}

impl CliPermissionMode {
    fn as_arg(self) -> &'static str {
        match self {
            CliPermissionMode::Default => "default",
            CliPermissionMode::Plan => "plan",
            CliPermissionMode::AcceptEdits => "accept-edits",
            CliPermissionMode::DontAsk => "dont-ask",
            CliPermissionMode::BypassPermissions => "bypass-permissions",
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("nca starting");
    let mut config = NcaConfig::load()?;

    if let Some(model) = &cli.model {
        config.model.default_model = model.clone();
        config.provider.minimax.model = model.clone();
    }

    config.model.max_tokens = cli.max_tokens;
    if cli.enable_thinking {
        config.model.enable_thinking = true;
        config.model.thinking_budget = cli.thinking_budget;
    }

    let workspace_root = PathBuf::from(".");
    match cli.command {
        Some(Command::Run {
            prompt,
            stream,
            json,
            safe,
            permission_mode,
            session_id,
        }) => {
            config.permissions.mode = permission_mode.into();
            run_one_shot(config, &workspace_root, &prompt, stream, json, safe, session_id).await?;
        }
        Some(Command::Spawn {
            prompt,
            safe,
            permission_mode,
        }) => {
            config.permissions.mode = permission_mode.into();
            spawn_run(&workspace_root, &prompt, safe, permission_mode).await?;
        }
        Some(Command::Sessions) => {
            list_sessions(&config, &workspace_root).await?;
        }
        Some(Command::Resume {
            session_id,
            prompt,
            safe,
            stream,
            permission_mode,
        }) => {
            config.permissions.mode = permission_mode.into();
            resume_session(config, &workspace_root, &session_id, prompt, safe, stream).await?;
        }
        Some(Command::Logs { session_id, follow }) => {
            show_logs(&config, &workspace_root, &session_id, follow).await?;
        }
        Some(Command::Attach { session_id }) => {
            attach_session(&config, &workspace_root, &session_id).await?;
        }
        Some(Command::Status { session_id }) => {
            show_status(&config, &workspace_root, &session_id).await?;
        }
        Some(Command::Cancel { session_id }) => {
            cancel_session(&config, &workspace_root, &session_id).await?;
        }
        None => {
            if let Some(prompt) = cli.prompt.as_deref() {
                config.permissions.mode = cli.permission_mode.into();
                run_one_shot(
                    config,
                    &workspace_root,
                    prompt,
                    cli.stream,
                    cli.json,
                    cli.safe,
                    cli.session_id,
                )
                .await?;
            } else if cli.resume {
                println!("Use `nca sessions` and `nca resume <session_id>` to resume a session.");
            } else {
                config.permissions.mode = cli.permission_mode.into();
                let mut runtime =
                    build_session_runtime(
                        config.clone(),
                        &workspace_root,
                        cli.safe,
                        true,
                        cli.session_id,
                    )
                    .await
                    .map_err(anyhow::Error::msg)?;
                if let Some(rx) = runtime.take_event_rx() {
                    let ipc_handle = runtime.take_ipc_handle();
                    let _stream_task =
                        spawn_stream_task(rx, cli.stream, runtime.event_log_path(), ipc_handle);
                    let mut repl = Repl::new(runtime, cli.safe);
                    repl.run().await?;
                }
            }
        }
    }

    Ok(())
}

async fn run_one_shot(
    config: NcaConfig,
    workspace_root: &PathBuf,
    prompt: &str,
    stream: StreamMode,
    json: bool,
    safe: bool,
    session_id: Option<String>,
) -> anyhow::Result<()> {
    let mut runtime =
        build_session_runtime(config.clone(), workspace_root, safe, true, session_id)
            .await
            .map_err(anyhow::Error::msg)?;
    if let Some(rx) = runtime.take_event_rx() {
        let ipc_handle = runtime.take_ipc_handle();
        let stream_task = spawn_stream_task(rx, stream, runtime.event_log_path(), ipc_handle);
        let output = runtime.run_turn(prompt).await.map_err(anyhow::Error::msg)?;
        runtime.finish(EndReason::Completed).await;
        if matches!(stream, StreamMode::Off) {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "session_id": runtime.session_id,
                        "model": runtime.model,
                        "output": output,
                    })
                );
            } else {
                println!("{output}");
            }
        } else {
            println!();
            eprintln!("[session] {}", runtime.session_id);
        }
        stream_task.abort();
    }
    Ok(())
}

async fn spawn_run(
    workspace_root: &PathBuf,
    prompt: &str,
    safe: bool,
    permission_mode: CliPermissionMode,
) -> anyhow::Result<()> {
    let session_id = format!("session-{}", chrono::Utc::now().timestamp_millis());
    let sessions_dir = workspace_root.join(".nca/sessions");
    std::fs::create_dir_all(&sessions_dir)?;
    let spawn_log = sessions_dir.join(format!("{session_id}.spawn.log"));
    let stdout = std::fs::File::create(&spawn_log)?;
    let stderr = stdout.try_clone()?;
    let exe = std::env::current_exe()?;

    std::process::Command::new(exe)
        .arg("run")
        .arg("--prompt")
        .arg(prompt)
        .arg("--stream")
        .arg("ndjson")
        .arg("--session-id")
        .arg(&session_id)
        .arg("--permission-mode")
        .arg(permission_mode.as_arg())
        .args(if safe { vec!["--safe"] } else { vec![] })
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?;

    println!("{session_id}");
    Ok(())
}

async fn list_sessions(config: &NcaConfig, workspace_root: &PathBuf) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let mut ids = store.list().await.map_err(anyhow::Error::msg)?;
    ids.sort();
    for id in ids {
        if let Ok(session) = store.load(&id).await {
            println!("{id}\t{:?}", session.meta.status);
        } else {
            println!("{id}");
        }
    }
    Ok(())
}

async fn resume_session(
    config: NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
    prompt: Option<String>,
    safe: bool,
    stream: StreamMode,
) -> anyhow::Result<()> {
    let mut runtime =
        build_resumed_session_runtime(config, workspace_root, safe, true, session_id)
            .await
            .map_err(anyhow::Error::msg)?;
    if let Some(rx) = runtime.take_event_rx() {
        let ipc_handle = runtime.take_ipc_handle();
        let _stream_task = spawn_stream_task(rx, stream, runtime.event_log_path(), ipc_handle);
        if let Some(prompt) = prompt {
            let output = runtime.run_turn(&prompt).await.map_err(anyhow::Error::msg)?;
            println!("{output}");
        } else {
            let mut repl = Repl::new(runtime, safe);
            repl.run().await?;
        }
    }
    Ok(())
}

async fn show_logs(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
    follow: bool,
) -> anyhow::Result<()> {
    if follow {
        return attach_session(config, workspace_root, session_id).await;
    }
    print_log_file(config, workspace_root, session_id).await
}

async fn attach_session(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let session = store.load(session_id).await.map_err(anyhow::Error::msg)?;

    if let Some(socket_path) = session.meta.socket_path.clone() {
        let client = nca_runtime::ipc::IpcClient::new(socket_path);
        if let Ok(mut rx) = client.connect().await {
            while let Some(event) = rx.recv().await {
                println!("{}", serde_json::to_string(&event)?);
            }
            return Ok(());
        }
    }

    print_log_file(config, workspace_root, session_id).await
}

async fn show_status(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let session = store.load(session_id).await.map_err(anyhow::Error::msg)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "id": session.meta.id,
            "status": session.meta.status,
            "pid": session.meta.pid,
            "socket_path": session.meta.socket_path,
            "updated_at": session.meta.updated_at,
            "model": session.meta.model,
            "estimated_cost_usd": session.estimated_cost_usd,
        }))?
    );
    Ok(())
}

async fn cancel_session(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let mut session = store.load(session_id).await.map_err(anyhow::Error::msg)?;

    if let Some(socket_path) = session.meta.socket_path.clone() {
        let client = nca_runtime::ipc::IpcClient::new(socket_path);
        let _ = client.send_command(&AgentCommand::Shutdown).await;
    }

    if let Some(pid) = session.meta.pid {
        let _ = tokio::process::Command::new("kill")
            .arg(pid.to_string())
            .output()
            .await;
    }

    session.meta.status = nca_common::session::SessionStatus::Cancelled;
    store.save(&session).await.map_err(anyhow::Error::msg)?;
    println!("Cancelled {session_id}");
    Ok(())
}

async fn print_log_file(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
) -> anyhow::Result<()> {
    let log_path = workspace_root
        .join(&config.session.history_dir)
        .join(format!("{session_id}.events.jsonl"));
    let data = match tokio::fs::read_to_string(&log_path).await {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!("No event log found for {session_id}");
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };
    print!("{data}");
    Ok(())
}
