mod approval_prompt;
mod prompt;
mod repl;
mod runner;
mod stream;

use crate::approval_prompt::IpcApprovalHandler;
use clap::Parser;
use nca_common::config::{NcaConfig, PermissionMode, ProviderKind};
use nca_common::event::EndReason;
use nca_common::event::{AgentCommand, EventEnvelope};
use nca_common::session::{OrchestrationContext, SessionSnapshot, SessionStatus};
use nca_core::skills::SkillCatalog;
use nca_runtime::memory_store::{MemoryNote, MemoryStore};
use repl::Repl;
use runner::{build_resumed_session_runtime, build_session_runtime};
use std::path::PathBuf;
use std::process::ExitCode;
use stream::{StreamMode, spawn_stream_task};

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

    /// Permission handling mode (default: from config, fallback to `default`)
    #[arg(long, value_enum)]
    permission_mode: Option<CliPermissionMode>,

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
        #[arg(long, value_enum)]
        permission_mode: Option<CliPermissionMode>,
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
        // Serve (IPC-driven) defaults to accept-edits since the monitor handles UX
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
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum)]
        permission_mode: Option<CliPermissionMode>,
    },
    Sessions {
        #[arg(long)]
        json: bool,
    },
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
        #[arg(long, value_enum)]
        permission_mode: Option<CliPermissionMode>,
    },
    Logs {
        session_id: String,
        #[arg(long)]
        follow: bool,
        #[arg(long)]
        json: bool,
    },
    Attach {
        session_id: String,
        #[arg(long)]
        json: bool,
    },
    Status {
        session_id: String,
        #[arg(long)]
        json: bool,
    },
    Cancel {
        session_id: String,
        #[arg(long)]
        json: bool,
    },
    Skills {
        #[arg(long)]
        json: bool,
    },
    Mcp {
        #[arg(long)]
        json: bool,
    },
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
        #[arg(long)]
        json: bool,
    },
    Models {
        #[arg(long)]
        json: bool,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
    Config {
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
enum MemoryCommand {
    List,
    Add {
        text: String,
        #[arg(long, default_value = "note")]
        kind: String,
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
async fn main() -> ExitCode {
    match try_main().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            classify_exit_code(&error)
        }
    }
}

async fn try_main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("nca starting");
    let mut config = NcaConfig::load()?;
    let orchestration_context = OrchestrationContext::from_env();

    if let Some(model) = &cli.model {
        config.apply_model_override(model);
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
                config.apply_model_override(&model);
            }
            if let Some(mode) = permission_mode {
                config.permissions.mode = mode.into();
            }
            run_one_shot(
                config,
                &workspace_root,
                &prompt,
                stream,
                json,
                safe,
                session_id,
                orchestration_context.clone(),
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
                config.apply_model_override(&model);
            }
            // Serve always uses an explicit default (accept-edits) — not overridable from config
            config.permissions.mode = permission_mode.into();
            run_service_session(
                config,
                &workspace_root,
                prompt,
                stream,
                safe,
                session_id,
                orchestration_context.clone(),
            )
            .await?;
        }
        Some(Command::Spawn {
            prompt,
            model,
            safe,
            json,
            permission_mode,
        }) => {
            let effective_mode = permission_mode.unwrap_or(CliPermissionMode::AcceptEdits);
            config.permissions.mode = effective_mode.into();
            spawn_run(
                &workspace_root,
                &prompt,
                model
                    .as_deref()
                    .map(|model| config.model.resolve_alias(model)),
                safe,
                effective_mode,
                json,
            )
            .await?;
        }
        Some(Command::Sessions { json }) => {
            list_sessions(&config, &workspace_root, json).await?;
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
                config.apply_model_override(&model);
            }
            if let Some(mode) = permission_mode {
                config.permissions.mode = mode.into();
            }
            resume_session(config, &workspace_root, &session_id, prompt, safe, stream).await?;
        }
        Some(Command::Logs {
            session_id,
            follow,
            json,
        }) => {
            show_logs(&config, &workspace_root, &session_id, follow, json).await?;
        }
        Some(Command::Attach { session_id, json }) => {
            attach_session(&config, &workspace_root, &session_id, json).await?;
        }
        Some(Command::Status { session_id, json }) => {
            show_status(&config, &workspace_root, &session_id, json).await?;
        }
        Some(Command::Cancel { session_id, json }) => {
            cancel_session(&config, &workspace_root, &session_id, json).await?;
        }
        Some(Command::Skills { json }) => {
            list_skills(&config, &workspace_root, json)?;
        }
        Some(Command::Mcp { json }) => {
            list_mcp_servers(&config, json)?;
        }
        Some(Command::Memory { command, json }) => match command {
            MemoryCommand::List => show_memory(&config, &workspace_root, json).await?,
            MemoryCommand::Add { text, kind } => {
                add_memory_note(&config, &workspace_root, &kind, &text, json).await?
            }
        },
        Some(Command::Models { json }) => {
            show_models(&config, json)?;
        }
        Some(Command::Doctor { json }) => {
            show_doctor(&config, &workspace_root, json)?;
        }
        Some(Command::Config { json }) => {
            show_config(&config, &workspace_root, json)?;
        }
        None => {
            if let Some(prompt) = cli.prompt.as_deref() {
                if let Some(mode) = cli.permission_mode {
                    config.permissions.mode = mode.into();
                }
                if cli.run {
                    let ipc_approval = IpcApprovalHandler::new();
                    let mut runtime = build_session_runtime(
                        config.clone(),
                        &workspace_root,
                        cli.safe,
                        true,
                        cli.session_id,
                        Some(ipc_approval.clone()),
                        orchestration_context.clone(),
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
                        orchestration_context.clone(),
                    )
                    .await?;
                }
            } else if cli.resume {
                if let Some(mode) = cli.permission_mode {
                    config.permissions.mode = mode.into();
                }
                let session_id = latest_session_id(&config, &workspace_root).await?;
                resume_session(
                    config,
                    &workspace_root,
                    &session_id,
                    None,
                    cli.safe,
                    cli.stream,
                )
                .await?;
            } else {
                if cli.run {
                    eprintln!("[run-mode] interactive run profile enabled");
                }
                if let Some(mode) = cli.permission_mode {
                    config.permissions.mode = mode.into();
                }
                let ipc_approval = IpcApprovalHandler::new();
                let mut runtime = build_session_runtime(
                    config.clone(),
                    &workspace_root,
                    cli.safe,
                    true,
                    cli.session_id,
                    Some(ipc_approval.clone()),
                    orchestration_context.clone(),
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
    orchestration_context: Option<OrchestrationContext>,
) -> anyhow::Result<()> {
    let mut runtime = build_session_runtime(
        config.clone(),
        workspace_root,
        safe,
        false,
        session_id,
        None,
        orchestration_context,
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
        let outcome = match result {
            Ok(output) => {
                runtime.finish(EndReason::Completed).await;
                if matches!(stream, StreamMode::Off) {
                    if json {
                        print_json(
                            &RunCommandOutput {
                                session: runtime.snapshot(),
                                output,
                                end_reason: "completed",
                            },
                            false,
                        )?;
                    } else {
                        println!("{output}");
                    }
                } else {
                    println!();
                    eprintln!("[session] {}", runtime.session_id());
                }
                Ok(())
            }
            Err(error) => {
                runtime.finish(EndReason::Error).await;
                Err(anyhow::Error::msg(error.to_string()))
            }
        };
        stream_task.abort();
        if let Some(st) = spawn_task {
            st.abort();
        }
        outcome?;
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
    orchestration_context: Option<OrchestrationContext>,
) -> anyhow::Result<()> {
    let _ = stream;
    nca_runtime::service::run_service_session(nca_runtime::service::ServiceSessionRequest {
        config,
        workspace_root: workspace_root.clone(),
        safe_mode: safe,
        initial_prompt,
        orchestration_context,
        launch_context: None,
        kind: nca_runtime::service::ServiceSessionKind::New { session_id },
    })
    .await
    .map_err(anyhow::Error::msg)
}

async fn spawn_run(
    workspace_root: &PathBuf,
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

async fn list_sessions(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    json: bool,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let ids = store.list().await.map_err(anyhow::Error::msg)?;
    let mut sessions = Vec::new();
    let mut unreadable = Vec::new();

    for id in ids {
        match store.load_snapshot(&id).await {
            Ok(session) => sessions.push(session),
            Err(_) => unreadable.push(id),
        }
    }

    sessions.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    unreadable.sort();

    if json {
        print_json(
            &SessionListOutput {
                sessions,
                unreadable,
            },
            false,
        )?;
    } else {
        for session in sessions {
            print_human_session(&session);
        }
        for id in unreadable {
            println!("{id}\tUnreadable");
        }
    }
    Ok(())
}

async fn latest_session_id(config: &NcaConfig, workspace_root: &PathBuf) -> anyhow::Result<String> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let ids = store.list().await.map_err(anyhow::Error::msg)?;
    let mut latest = None;

    for id in ids {
        let Ok(session) = store.load(&id).await else {
            continue;
        };

        let should_replace = latest
            .as_ref()
            .map(|(_, updated_at)| session.meta.updated_at > *updated_at)
            .unwrap_or(true);
        if should_replace {
            latest = Some((session.meta.id, session.meta.updated_at));
        }
    }

    latest
        .map(|(id, _)| id)
        .ok_or_else(|| anyhow::anyhow!("no saved sessions found to resume"))
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
    json: bool,
) -> anyhow::Result<()> {
    if follow {
        return attach_session(config, workspace_root, session_id, json).await;
    }
    print_log_file(config, workspace_root, session_id, json).await
}

async fn attach_session(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
    json: bool,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let session = store.load(session_id).await.map_err(anyhow::Error::msg)?;

    if let Some(socket_path) = session.meta.socket_path.clone() {
        let client = nca_runtime::ipc::IpcClient::new(socket_path);
        if let Ok(mut rx) = client.connect().await {
            while let Some(envelope) = rx.recv().await {
                print_event_envelope(&envelope, json)?;
            }
            return Ok(());
        }
    }

    print_log_file(config, workspace_root, session_id, json).await
}

async fn show_status(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
    json: bool,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let snapshot = store
        .load_snapshot(session_id)
        .await
        .map_err(anyhow::Error::msg)?;
    if json {
        print_json(&snapshot, false)?;
    } else {
        print_human_session(&snapshot);
    }
    Ok(())
}

async fn cancel_session(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
    json: bool,
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

    session.meta.status = SessionStatus::Cancelled;
    session.meta.updated_at = chrono::Utc::now();
    session.meta.pid = None;
    session.meta.socket_path = None;
    store.save(&session).await.map_err(anyhow::Error::msg)?;
    if json {
        print_json(
            &CancelCommandOutput {
                session: session.snapshot(),
                cancelled: true,
            },
            false,
        )?;
    } else {
        println!("Cancelled {session_id}");
    }
    Ok(())
}

async fn print_log_file(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    session_id: &str,
    json: bool,
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
    for line in data.lines() {
        let envelope: EventEnvelope = serde_json::from_str(line)?;
        print_event_envelope(&envelope, json)?;
    }
    Ok(())
}

fn print_human_session(session: &SessionSnapshot) {
    println!(
        "{}  status={:?}  model={}  updated={}  children={}",
        session.id,
        session.status,
        session.model,
        session.updated_at.to_rfc3339(),
        session.child_session_ids.len()
    );
    if let Some(summary) = &session.session_summary {
        println!("  summary: {}", summary.replace('\n', " "));
    }
}

fn print_event_envelope(envelope: &EventEnvelope, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string(envelope)?);
    } else {
        stream::render_human_event(&envelope.event);
    }
    Ok(())
}

fn list_skills(config: &NcaConfig, workspace_root: &PathBuf, json: bool) -> anyhow::Result<()> {
    let skills = SkillCatalog::discover(workspace_root, &config.harness.skill_directories)
        .map_err(anyhow::Error::msg)?;
    if json {
        let output: Vec<_> = skills
            .into_iter()
            .map(|skill| SkillOutput {
                name: skill.name,
                command: skill.command,
                description: skill.description,
                model: skill.model,
                permission_mode: skill.permission_mode.map(|mode| format!("{mode:?}")),
                context: format!("{:?}", skill.context),
                directory: skill.directory,
            })
            .collect();
        print_json(&output, false)?;
    } else if skills.is_empty() {
        println!("No skills found");
    } else {
        for skill in skills {
            println!("{}", skill.summary_line());
        }
    }
    Ok(())
}

fn list_mcp_servers(config: &NcaConfig, json: bool) -> anyhow::Result<()> {
    if json {
        print_json(&config.mcp, false)?;
    } else if config.mcp.servers.is_empty() {
        println!("No MCP servers configured");
    } else {
        for server in config.mcp.servers.iter().filter(|server| server.enabled) {
            println!(
                "{}  command={} {}",
                server.name,
                server.command,
                server.args.join(" ")
            );
        }
    }
    Ok(())
}

async fn show_memory(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    json: bool,
) -> anyhow::Result<()> {
    let store = workspace_memory_store(config, workspace_root);
    let state = store.load().await.map_err(anyhow::Error::msg)?;
    if json {
        print_json(&state, false)?;
    } else if state.notes.is_empty() {
        println!("No memory notes stored");
    } else {
        for note in state.notes {
            println!(
                "{}  {}  {}",
                note.id,
                note.kind,
                note.title.unwrap_or_else(|| note.created_at.to_rfc3339())
            );
            println!("  {}", note.content.replace('\n', " "));
        }
    }
    Ok(())
}

async fn add_memory_note(
    config: &NcaConfig,
    workspace_root: &PathBuf,
    kind: &str,
    text: &str,
    json: bool,
) -> anyhow::Result<()> {
    let store = workspace_memory_store(config, workspace_root);
    let note = MemoryNote {
        id: format!("{}-{}", kind, chrono::Utc::now().timestamp_millis()),
        created_at: chrono::Utc::now(),
        kind: kind.to_string(),
        title: None,
        content: text.trim().to_string(),
    };
    let state = store
        .append_note(note.clone(), config.memory.max_notes)
        .await
        .map_err(anyhow::Error::msg)?;
    if json {
        print_json(&note, false)?;
    } else {
        println!("Stored memory note {} ({})", note.id, kind);
        println!("Memory path: {}", store.path().display());
        println!("Total notes: {}", state.notes.len());
    }
    Ok(())
}

fn show_models(config: &NcaConfig, json: bool) -> anyhow::Result<()> {
    let output = ModelCatalogOutput {
        default_provider: config.provider.default.display_name().to_string(),
        default_model: config.model.default_model.clone(),
        provider_models: ProviderKind::ALL
            .into_iter()
            .map(|provider| ProviderModelOutput {
                provider: provider.display_name().to_string(),
                model: config.provider.model_for(provider).to_string(),
                base_url: config.provider.base_url_for(provider).to_string(),
                selected: provider == config.provider.default,
            })
            .collect(),
        aliases: config.model.aliases.clone(),
        thinking_enabled: config.model.enable_thinking,
        thinking_budget: config.model.thinking_budget,
    };
    if json {
        print_json(&output, false)?;
    } else {
        println!(
            "Default provider/model: {} / {}",
            output.default_provider, output.default_model
        );
        println!(
            "Thinking: {} (budget {})",
            if output.thinking_enabled { "on" } else { "off" },
            output.thinking_budget
        );
        println!("Provider models:");
        for provider in &output.provider_models {
            println!(
                "  {}{} -> {} ({})",
                provider.provider,
                if provider.selected { " [selected]" } else { "" },
                provider.model,
                provider.base_url
            );
        }
        for (alias, target) in output.aliases {
            println!("  {alias} -> {target}");
        }
    }
    Ok(())
}

fn show_doctor(config: &NcaConfig, workspace_root: &PathBuf, json: bool) -> anyhow::Result<()> {
    let skills = SkillCatalog::discover(workspace_root, &config.harness.skill_directories)
        .map(|skills| skills.len())
        .unwrap_or(0);
    let output = DoctorOutput {
        provider: config.provider.default.display_name().to_string(),
        default_model: config.model.default_model.clone(),
        providers: ProviderKind::ALL
            .into_iter()
            .map(|provider| ProviderDoctorStatus {
                provider: provider.display_name().to_string(),
                selected: provider == config.provider.default,
                api_key_present: config.provider.api_key_present_for(provider),
                api_key_env: config.provider.api_key_env_for(provider).to_string(),
                model: config.provider.model_for(provider).to_string(),
                base_url: config.provider.base_url_for(provider).to_string(),
            })
            .collect(),
        mcp_server_count: config
            .mcp
            .servers
            .iter()
            .filter(|server| server.enabled)
            .count(),
        skill_count: skills,
        memory_path: if config.memory.file_path.is_absolute() {
            config.memory.file_path.clone()
        } else {
            workspace_root.join(&config.memory.file_path)
        },
    };
    if json {
        print_json(&output, false)?;
    } else {
        println!("Provider: {}", output.provider);
        println!("Default model: {}", output.default_model);
        println!("Provider readiness:");
        for provider in &output.providers {
            println!(
                "  {}{}: api_key={} ({}) model={} base_url={}",
                provider.provider,
                if provider.selected { " [selected]" } else { "" },
                if provider.api_key_present {
                    "configured"
                } else {
                    "missing"
                },
                provider.api_key_env,
                provider.model,
                provider.base_url
            );
        }
        println!("Skills discovered: {}", output.skill_count);
        println!("MCP servers enabled: {}", output.mcp_server_count);
        println!("Memory path: {}", output.memory_path.display());
        println!("MiniMax remains the default recommended path for this workspace.");
    }
    Ok(())
}

fn show_config(config: &NcaConfig, workspace_root: &PathBuf, json: bool) -> anyhow::Result<()> {
    if json {
        print_json(config, false)?;
    } else {
        println!(
            "Global config: {}",
            nca_common::config::global_config_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unavailable>".into())
        );
        println!(
            "Workspace config: {}",
            nca_common::config::workspace_config_path(workspace_root).display()
        );
        println!("Default provider: {:?}", config.provider.default);
        println!("Default model: {}", config.model.default_model);
        println!("Permission mode: {:?}", config.permissions.mode);
        println!("Provider endpoints:");
        for provider in ProviderKind::ALL {
            println!(
                "  {} -> model={} base_url={}",
                provider.display_name(),
                config.provider.model_for(provider),
                config.provider.base_url_for(provider)
            );
        }
        println!(
            "Memory path: {}",
            workspace_memory_store(config, workspace_root)
                .path()
                .display()
        );
        println!("Skill directories:");
        for path in &config.harness.skill_directories {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                workspace_root.join(path)
            };
            println!("  {}", resolved.display());
        }
    }
    Ok(())
}

fn workspace_memory_store(config: &NcaConfig, workspace_root: &PathBuf) -> MemoryStore {
    if config.memory.file_path.is_absolute() {
        MemoryStore::new(config.memory.file_path.clone())
    } else {
        MemoryStore::new(workspace_root.join(&config.memory.file_path))
    }
}

#[derive(serde::Serialize)]
struct RunCommandOutput {
    session: SessionSnapshot,
    output: String,
    end_reason: &'static str,
}

#[derive(serde::Serialize)]
struct SpawnCommandOutput {
    session_id: String,
    pid: u32,
    status_path: PathBuf,
    event_log_path: PathBuf,
    spawn_log_path: PathBuf,
    socket_path: PathBuf,
    permission_mode: String,
    safe_mode: bool,
}

#[derive(serde::Serialize)]
struct SessionListOutput {
    sessions: Vec<SessionSnapshot>,
    unreadable: Vec<String>,
}

#[derive(serde::Serialize)]
struct CancelCommandOutput {
    session: SessionSnapshot,
    cancelled: bool,
}

#[derive(serde::Serialize)]
struct SkillOutput {
    name: String,
    command: String,
    description: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    context: String,
    directory: PathBuf,
}

#[derive(serde::Serialize)]
struct ModelCatalogOutput {
    default_provider: String,
    default_model: String,
    provider_models: Vec<ProviderModelOutput>,
    aliases: std::collections::BTreeMap<String, String>,
    thinking_enabled: bool,
    thinking_budget: u32,
}

#[derive(serde::Serialize)]
struct DoctorOutput {
    provider: String,
    default_model: String,
    providers: Vec<ProviderDoctorStatus>,
    mcp_server_count: usize,
    skill_count: usize,
    memory_path: PathBuf,
}

#[derive(serde::Serialize)]
struct ProviderModelOutput {
    provider: String,
    model: String,
    base_url: String,
    selected: bool,
}

#[derive(serde::Serialize)]
struct ProviderDoctorStatus {
    provider: String,
    selected: bool,
    api_key_present: bool,
    api_key_env: String,
    model: String,
    base_url: String,
}

fn print_json<T: serde::Serialize>(value: &T, pretty: bool) -> anyhow::Result<()> {
    let rendered = if pretty {
        serde_json::to_string_pretty(value)?
    } else {
        serde_json::to_string(value)?
    };
    println!("{rendered}");
    Ok(())
}

fn classify_exit_code(error: &anyhow::Error) -> ExitCode {
    const EXIT_CONFIGURATION: u8 = 10;
    const EXIT_RUNTIME: u8 = 11;
    const EXIT_APPROVAL: u8 = 13;
    const EXIT_CANCELLED: u8 = 130;

    let mut combined = String::new();
    for (idx, cause) in error.chain().enumerate() {
        if idx > 0 {
            combined.push_str(" | ");
        }
        combined.push_str(&cause.to_string().to_ascii_lowercase());
    }

    let code = if combined.contains("requires approval in headless mode")
        || combined.contains("requires approval; request was denied")
        || combined.contains("denied by policy")
    {
        EXIT_APPROVAL
    } else if combined.contains("run cancelled") {
        EXIT_CANCELLED
    } else if combined.contains("missing minimax api key")
        || combined.contains("failed to parse config file")
        || combined.contains("unable to determine the home directory")
        || combined.contains("invalid workspace root")
    {
        EXIT_CONFIGURATION
    } else if combined.contains("provider")
        || combined.contains("tool `")
        || combined.contains("empty response")
        || combined.contains("turn budget exceeded")
    {
        EXIT_RUNTIME
    } else {
        1
    };

    ExitCode::from(code)
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
