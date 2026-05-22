//! `run` and one-shot execution handlers.

use crate::approval_prompts::InteractiveIpcApprovalHandler;
use crate::cli::StdinMerge;
use crate::cmd::util::print_json;
use crate::stream::{StreamMode, spawn_stream_task};
use nca_common::config::NcaConfig;
use nca_common::event::EndReason;
use nca_common::session::{OrchestrationContext, SessionSnapshot};
use nca_tui::{Repl, build_session_runtime};
use std::io::{self, IsTerminal, Read};
use std::path::Path;

pub struct OneShotOptions {
    pub stream: StreamMode,
    pub json: bool,
    pub safe: bool,
    pub session_id: Option<String>,
    pub orchestration_context: Option<OrchestrationContext>,
}

#[derive(serde::Serialize)]
pub(crate) struct RunCommandOutput {
    session: SessionSnapshot,
    output: String,
    end_reason: &'static str,
}

pub async fn run_one_shot(
    config: NcaConfig,
    workspace_root: &Path,
    prompt: &str,
    opts: OneShotOptions,
) -> anyhow::Result<()> {
    let OneShotOptions {
        stream,
        json,
        safe,
        session_id,
        orchestration_context,
    } = opts;
    let mut runtime = build_session_runtime(
        config.clone(),
        workspace_root,
        safe,
        false,
        session_id,
        None,
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
            runtime.question_pending(),
            None,
        );

        let spawn_task = runtime.take_spawn_rx().map(|spawn_rx| {
            nca_runtime::supervisor::spawn_subagent_consumer(
                spawn_rx,
                runtime.session_id().to_string(),
                runtime.workspace_root().to_path_buf(),
                config.clone(),
                runtime.messages().to_vec(),
                None,
            )
        });

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

pub async fn run_service_session(
    config: NcaConfig,
    workspace_root: &Path,
    initial_prompt: Option<String>,
    stream: StreamMode,
    safe: bool,
    session_id: Option<String>,
    orchestration_context: Option<OrchestrationContext>,
) -> anyhow::Result<()> {
    let _ = stream;
    nca_runtime::service::run_service_session(nca_runtime::service::ServiceSessionRequest {
        config,
        workspace_root: workspace_root.to_path_buf(),
        safe_mode: safe,
        initial_prompt,
        orchestration_context,
        kind: nca_runtime::service::ServiceSessionKind::New { session_id },
    })
    .await
    .map_err(anyhow::Error::msg)
}

pub fn compose_prompt_with_stdin(
    cli_prompt: Option<String>,
    mode: Option<StdinMerge>,
) -> anyhow::Result<Option<String>> {
    use std::io::{self, IsTerminal, Read};

    let stdin_is_tty = io::stdin().is_terminal();
    let want_stdin = mode.is_some() || !stdin_is_tty;
    let stdin_text: Option<String> = if want_stdin {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        let trimmed = buf.trim_end_matches('\n').to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    } else {
        None
    };

    let prompt = cli_prompt.and_then(|s| {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    });

    let effective_mode = mode.unwrap_or(StdinMerge::Append);
    let combined = match (prompt, stdin_text) {
        (Some(p), Some(s)) => match effective_mode {
            StdinMerge::Only => Some(s),
            StdinMerge::Prefix => Some(format!("{s}\n\n{p}")),
            StdinMerge::Append => Some(format!("{p}\n\n{s}")),
        },
        (Some(p), None) => Some(p),
        (None, Some(s)) => Some(s),
        (None, None) => None,
    };
    Ok(combined)
}
