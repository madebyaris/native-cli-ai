mod app;
mod approval_prompt;
mod prompt;
mod render;
mod repl;
mod runner;
mod runtime_tooling;
mod stream;

use crate::approval_prompt::IpcApprovalHandler;
use clap::Parser;
use nca_common::config::{NcaConfig, PermissionMode};
use nca_common::event::AgentCommand;
use nca_common::event::EndReason;
use repl::Repl;
use runner::{build_resumed_session_runtime, build_session_runtime};
use std::path::PathBuf;
use stream::{StreamMode, spawn_event_fanout_task, spawn_stream_task};

#[derive(Parser, Debug)]
#[command(
    name = "nca",
    about = "Native CLI AI - a Rust-powered coding assistant"
)]
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

    /// Start interactive run mode (Claude-style)
    #[arg(long)]
    run: bool,

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
        model: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        safe: bool,
        #[arg(long, value_enum, default_value_t = CliPermissionMode::Default)]
        permission_mode: CliPermissionMode,
        #[arg(long, hide = true)]
        session_id: Option<String>,
    },
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long, value_enum, default_value_t = StreamMode::Ndjson)]
        stream: StreamMode,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        safe: bool,
        #[arg(long, value_enum, default_value_t = CliPermissionMode::AcceptEdits)]
        permission_mode: CliPermissionMode,
        #[arg(long, hide = true)]
        session_id: Option<String>,
    },
    Spawn {
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        model: Option<String>,
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
        model: Option<String>,
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
            model,
            json,
            safe,
            permission_mode,
            session_id,
        }) => {
            if let Some(model) = model {
                config.model.default_model = model.clone();
                config.provider.minimax.model = model;
            }
            config.permissions.mode = permission_mode.into();
            run_one_shot(
                config,
                &workspace_root,
                &prompt,
                stream,
                json,
                safe,
                session_id,
            )
            .await?;
        }
        Some(Command::Serve {
            prompt,
            stream,
            model,
            safe,
            permission_mode,
            session_id,
        }) => {
            if let Some(model) = model {
                config.model.default_model = model.clone();
                config.provider.minimax.model = model;
            }
            config.permissions.mode = permission_mode.into();
            run_service_session(config, &workspace_root, prompt, stream, safe, session_id).await?;
        }
        Some(Command::Spawn {
            prompt,
            model,
            safe,
            permission_mode,
        }) => {
            config.permissions.mode = permission_mode.into();
            spawn_run(
                &workspace_root,
                &prompt,
                model.as_deref(),
                safe,
                permission_mode,
            )
            .await?;
        }
        Some(Command::Sessions) => {
            list_sessions(&config, &workspace_root).await?;
        }
        Some(Command::Resume {
            session_id,
            prompt,
            model,
            safe,
            stream,
            permission_mode,
        }) => {
            if let Some(model) = model {
                config.model.default_model = model.clone();
                config.provider.minimax.model = model;
            }
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
                if cli.run {
                    let ipc_approval = IpcApprovalHandler::new();
                    let mut runtime = build_session_runtime(
                        config.clone(),
                        &workspace_root,
                        cli.safe,
                        true,
                        cli.session_id,
                        Some(ipc_approval.clone()),
                    )
                    .await
                    .map_err(anyhow::Error::msg)?;
                    if let Some(rx) = runtime.take_event_rx() {
                        let ipc_handle = runtime.take_ipc_handle();
                        let approval_pending = runtime.take_ipc_approval_pending();
                        let _stream_task = spawn_stream_task(
                            rx,
                            cli.stream,
                            runtime.event_log_path(),
                            ipc_handle,
                            approval_pending,
                            None,
                        );
                        let _ = runtime.run_turn(prompt).await;
                        let mut repl = Repl::new(runtime, cli.safe, true);
                        repl.run().await?;
                    }
                } else {
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
                }
            } else if cli.resume {
                println!("Use `nca sessions` and `nca resume <session_id>` to resume a session.");
            } else {
                if cli.run {
                    eprintln!("[run-mode] interactive run profile enabled");
                }
                config.permissions.mode = cli.permission_mode.into();
                let ipc_approval = IpcApprovalHandler::new();
                let mut runtime = build_session_runtime(
                    config.clone(),
                    &workspace_root,
                    cli.safe,
                    true,
                    cli.session_id,
                    Some(ipc_approval.clone()),
                )
                .await
                .map_err(anyhow::Error::msg)?;
                if let Some(rx) = runtime.take_event_rx() {
                    let ipc_handle = runtime.take_ipc_handle();
                    let approval_pending = runtime.take_ipc_approval_pending();
                    let _stream_task = spawn_stream_task(
                        rx,
                        cli.stream,
                        runtime.event_log_path(),
                        ipc_handle,
                        approval_pending,
                        None,
                    );
                    let mut repl = Repl::new(runtime, cli.safe, cli.run);
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
    let ipc_approval = IpcApprovalHandler::new();
    let mut runtime = build_session_runtime(
        config.clone(),
        workspace_root,
        safe,
        true,
        session_id,
        Some(ipc_approval.clone()),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    if let Some(rx) = runtime.take_event_rx() {
        let ipc_handle = runtime.take_ipc_handle();
        let approval_pending = runtime.take_ipc_approval_pending();
        let stream_task = spawn_stream_task(
            rx,
            stream,
            runtime.event_log_path(),
            ipc_handle,
            approval_pending,
            None,
        );

        let spawn_task = if let Some(spawn_rx) = runtime.take_spawn_rx() {
            Some(nca_runtime::supervisor::spawn_subagent_consumer(
                spawn_rx,
                runtime.session_id().to_string(),
                runtime.workspace_root().to_path_buf(),
                config.clone(),
                runtime.messages().to_vec(),
                None,
            ))
        } else {
            None
        };

        let result = runtime.run_turn(prompt).await;
        match result {
            Ok(output) => {
                runtime.finish(EndReason::Completed).await;
                if matches!(stream, StreamMode::Off) {
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({
                                "session_id": runtime.session_id(),
                                "model": runtime.model(),
                                "output": output,
                            })
                        );
                    } else {
                        println!("{output}");
                    }
                } else {
                    println!();
                    eprintln!("[session] {}", runtime.session_id());
                }
            }
            Err(e) => {
                eprintln!("[session:error] {e}");
                runtime.finish(EndReason::Error).await;
            }
        }
        stream_task.abort();
        if let Some(st) = spawn_task {
            st.abort();
        }
    }
    Ok(())
}

async fn run_service_session(
    config: NcaConfig,
    workspace_root: &PathBuf,
    initial_prompt: Option<String>,
    stream: StreamMode,
    safe: bool,
    session_id: Option<String>,
) -> anyhow::Result<()> {
    use nca_runtime::session_store::SessionStore;
    use nca_runtime::supervisor::{SessionControlCommand, spawn_command_consumer_with_store};
    use std::sync::atomic::Ordering;

    let ipc_approval = IpcApprovalHandler::new();
    let mut runtime = build_session_runtime(
        config.clone(),
        workspace_root,
        safe,
        true,
        session_id,
        Some(ipc_approval.clone()),
    )
    .await
    .map_err(anyhow::Error::msg)?;

    let event_rx = runtime
        .take_event_rx()
        .ok_or_else(|| anyhow::anyhow!("missing event receiver"))?;
    let approval_pending = runtime.take_ipc_approval_pending();
    let mut command_rx = None;
    let mut event_tx_ipc = None;
    if let Some(ipc_handle) = runtime.take_ipc_handle() {
        let (etx, crx) = ipc_handle.into_parts();
        event_tx_ipc = Some(etx);
        command_rx = Some(crx);
    }

    let fanout_task =
        spawn_event_fanout_task(event_rx, stream, runtime.event_log_path(), event_tx_ipc);

    let subagent_task = if let Some(spawn_rx) = runtime.take_spawn_rx() {
        Some(nca_runtime::supervisor::spawn_subagent_consumer(
            spawn_rx,
            runtime.session_id().to_string(),
            runtime.workspace_root().to_path_buf(),
            config.clone(),
            runtime.messages().to_vec(),
            runtime.event_tx(),
        ))
    } else {
        None
    };

    let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (control_tx, mut control_rx) =
        tokio::sync::mpsc::unbounded_channel::<SessionControlCommand>();

    let command_task = command_rx.map(|crx| {
        spawn_command_consumer_with_store(
            crx,
            approval_pending,
            None,
            Some(SessionStore::new(
                runtime.workspace_root().join(&config.session.history_dir),
            )),
            runtime.event_tx(),
            Some(prompt_tx.clone()),
            Some(control_tx.clone()),
        )
    });

    if let Some(prompt) = initial_prompt {
        let _ = prompt_tx.send(prompt);
    }

    let cancel_handle = runtime.cancel_handle();
    let mut reason = EndReason::UserExit;

    loop {
        let prompt = tokio::select! {
            control = control_rx.recv() => {
                match control {
                    Some(SessionControlCommand::Cancel) => {
                        cancel_handle.store(true, Ordering::SeqCst);
                        reason = EndReason::Cancelled;
                        break;
                    }
                    Some(SessionControlCommand::Shutdown) => {
                        cancel_handle.store(true, Ordering::SeqCst);
                        reason = EndReason::UserExit;
                        break;
                    }
                    None => break,
                }
            }
            prompt = prompt_rx.recv() => match prompt {
                Some(prompt) => prompt,
                None => break,
            }
        };

        let run_fut = runtime.run_turn(&prompt);
        tokio::pin!(run_fut);

        let result = tokio::select! {
            result = &mut run_fut => result,
            control = control_rx.recv() => {
                match control {
                    Some(SessionControlCommand::Cancel) => {
                        cancel_handle.store(true, Ordering::SeqCst);
                        reason = EndReason::Cancelled;
                    }
                    Some(SessionControlCommand::Shutdown) => {
                        cancel_handle.store(true, Ordering::SeqCst);
                        reason = EndReason::UserExit;
                    }
                    None => {}
                }
                run_fut.await
            }
        };

        if let Err(error) = result {
            if error.to_string().contains("run cancelled") {
                if matches!(reason, EndReason::Cancelled | EndReason::UserExit) {
                    break;
                }
                continue;
            }
            eprintln!("[session:error] {error}");
            reason = EndReason::Error;
            break;
        }
    }

    runtime.finish(reason).await;
    fanout_task.abort();
    if let Some(task) = command_task {
        task.abort();
    }
    if let Some(task) = subagent_task {
        task.abort();
    }
    Ok(())
}

async fn spawn_run(
    workspace_root: &PathBuf,
    prompt: &str,
    model: Option<&str>,
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

    command.stdout(stdout).stderr(stderr).spawn()?;

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
    let mut runtime = build_resumed_session_runtime(config, workspace_root, safe, true, session_id)
        .await
        .map_err(anyhow::Error::msg)?;
    if let Some(rx) = runtime.take_event_rx() {
        let ipc_handle = runtime.take_ipc_handle();
        let approval_pending = runtime.take_ipc_approval_pending();
        let _stream_task = spawn_stream_task(
            rx,
            stream,
            runtime.event_log_path(),
            ipc_handle,
            approval_pending,
            None,
        );
        if let Some(prompt) = prompt {
            let output = runtime
                .run_turn(&prompt)
                .await
                .map_err(anyhow::Error::msg)?;
            println!("{output}");
        } else {
            let mut repl = Repl::new(runtime, safe, true);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_level_run_mode() {
        let cli = Cli::try_parse_from(["nca", "--run"]).expect("should parse run mode");
        assert!(cli.run);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_run_subcommand_model_override() {
        let cli =
            Cli::try_parse_from(["nca", "run", "--prompt", "hello", "--model", "MiniMax-M2.5"])
                .expect("should parse run subcommand");

        match cli.command {
            Some(Command::Run { model, .. }) => {
                assert_eq!(model.as_deref(), Some("MiniMax-M2.5"));
            }
            _ => panic!("expected run subcommand"),
        }
    }
}
