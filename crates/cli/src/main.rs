mod app;
mod prompt;
mod render;
mod repl;
mod runner;
mod runtime_tooling;
mod stream;

use clap::Parser;
use nca_common::config::NcaConfig;
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
        #[arg(long, hide = true)]
        session_id: Option<String>,
    },
    Spawn {
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        safe: bool,
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
    },
    Logs {
        session_id: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
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
            session_id,
        }) => {
            run_one_shot(config, &workspace_root, &prompt, stream, json, safe, session_id).await?;
        }
        Some(Command::Spawn { prompt, safe }) => {
            spawn_run(&workspace_root, &prompt, safe).await?;
        }
        Some(Command::Sessions) => {
            list_sessions(&config, &workspace_root).await?;
        }
        Some(Command::Resume {
            session_id,
            prompt,
            safe,
            stream,
        }) => {
            resume_session(config, &workspace_root, &session_id, prompt, safe, stream).await?;
        }
        Some(Command::Logs { session_id }) => {
            show_logs(&config, &workspace_root, &session_id).await?;
        }
        None => {
            if let Some(prompt) = cli.prompt.as_deref() {
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
                let mut runtime =
                    build_session_runtime(config.clone(), &workspace_root, cli.safe, cli.session_id)
                        .map_err(anyhow::Error::msg)?;
                if let Some(rx) = runtime.take_event_rx() {
                    let _stream_task =
                        spawn_stream_task(rx, cli.stream, runtime.event_log_path());
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
    let mut runtime = build_session_runtime(config.clone(), workspace_root, safe, session_id)
        .map_err(anyhow::Error::msg)?;
    if let Some(rx) = runtime.take_event_rx() {
        let stream_task = spawn_stream_task(rx, stream, runtime.event_log_path());
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

async fn spawn_run(workspace_root: &PathBuf, prompt: &str, safe: bool) -> anyhow::Result<()> {
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
        println!("{id}");
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
        build_resumed_session_runtime(config, workspace_root, safe, session_id)
            .await
            .map_err(anyhow::Error::msg)?;
    if let Some(rx) = runtime.take_event_rx() {
        let _stream_task = spawn_stream_task(rx, stream, runtime.event_log_path());
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

async fn show_logs(config: &NcaConfig, workspace_root: &PathBuf, session_id: &str) -> anyhow::Result<()> {
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
