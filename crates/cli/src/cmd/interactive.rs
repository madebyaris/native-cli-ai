//! Default interactive mode when no subcommand is given.

use crate::approval_prompts::InteractiveIpcApprovalHandler;
use crate::cli::Cli;
use crate::cmd::run::{OneShotOptions, compose_prompt_with_stdin, run_one_shot};
use crate::cmd::sessions::{latest_session_id, resume_session};
use crate::stream::{StreamMode, spawn_stream_task};
use nca_common::config::NcaConfig;
use nca_common::session::OrchestrationContext;
use nca_tui::{Repl, build_session_runtime};
use std::io::{IsTerminal, stdin, stdout};
use std::path::Path;

pub async fn run_default(
    cli: &Cli,
    mut config: NcaConfig,
    workspace_root: &Path,
    orchestration_context: Option<OrchestrationContext>,
) -> anyhow::Result<()> {
    let effective_prompt = compose_prompt_with_stdin(cli.prompt.clone(), cli.stdin_as)?;
    if let Some(prompt) = effective_prompt.as_deref() {
        if let Some(mode) = cli.permission_mode {
            config.permissions.mode = mode.into();
        }
        if cli.run {
            let (ipc_approval, approval_pending) = InteractiveIpcApprovalHandler::new();
            let mut runtime = build_session_runtime(
                config.clone(),
                workspace_root,
                cli.safe,
                true,
                cli.session_id.clone(),
                Some(ipc_approval as std::sync::Arc<dyn nca_core::approval::ApprovalHandler>),
                Some(approval_pending),
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
                    runtime.question_pending(),
                    None,
                );
                let _ = runtime.run_turn(prompt).await;
                let mut repl = Repl::new(runtime, cli.safe, true);
                repl.run().await?;
            }
        } else {
            run_one_shot(
                config,
                workspace_root,
                prompt,
                OneShotOptions {
                    stream: cli.stream,
                    json: cli.json,
                    safe: cli.safe,
                    session_id: cli.session_id.clone(),
                    orchestration_context,
                },
            )
            .await?;
        }
    } else {
        // First-run onboarding: show connect modal before building runtime
        let onboarding_tui = !cli.no_tui
            && stdout().is_terminal()
            && stdin().is_terminal()
            && matches!(cli.stream, StreamMode::Human);
        if config.needs_onboarding() && onboarding_tui {
            config = nca_tui::tui::onboarding::run_onboarding(config).await?;
        }

        if cli.resume {
            if let Some(mode) = cli.permission_mode {
                config.permissions.mode = mode.into();
            }
            let session_id = latest_session_id(&config, workspace_root).await?;
            resume_session(
                config,
                workspace_root,
                &session_id,
                None,
                cli.safe,
                cli.stream,
                cli.no_tui,
            )
            .await?;
        } else if cli.no_resume {
            start_fresh_session(cli, config, workspace_root, orchestration_context).await?;
        } else if let Ok(Some(session_id)) =
            nca_runtime::supervisor::get_last_session_id(&config, workspace_root).await
        {
            eprintln!(
                "[session] Resuming last session {} (use --no-resume to start fresh)",
                session_id
            );
            resume_session(
                config,
                workspace_root,
                &session_id,
                None,
                cli.safe,
                cli.stream,
                cli.no_tui,
            )
            .await?;
        } else {
            start_fresh_session(cli, config, workspace_root, orchestration_context).await?;
        }
    }
    Ok(())
}

pub async fn start_fresh_session(
    cli: &Cli,
    mut config: NcaConfig,
    workspace_root: &Path,
    orchestration_context: Option<OrchestrationContext>,
) -> anyhow::Result<()> {
    if cli.run {
        eprintln!("[run-mode] interactive run profile enabled");
    }
    if let Some(mode) = cli.permission_mode {
        config.permissions.mode = mode.into();
    }
    let use_tui = !cli.no_tui
        && stdout().is_terminal()
        && stdin().is_terminal()
        && matches!(cli.stream, StreamMode::Human);
    let (approval_handler, approval_pending_cfg) = if use_tui {
        (None, None)
    } else {
        let (handler, pending) = InteractiveIpcApprovalHandler::new();
        (
            Some(handler as std::sync::Arc<dyn nca_core::approval::ApprovalHandler>),
            Some(pending),
        )
    };
    let mut runtime = build_session_runtime(
        config.clone(),
        workspace_root,
        cli.safe,
        true,
        cli.session_id.clone(),
        approval_handler,
        approval_pending_cfg,
        orchestration_context,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    if !use_tui && let Some(rx) = runtime.take_event_rx() {
        let ipc_handle = runtime.take_ipc_handle();
        let approval_pending = runtime.take_ipc_approval_pending();
        let _stream_task = spawn_stream_task(
            rx,
            cli.stream,
            runtime.event_log_path(),
            ipc_handle,
            approval_pending,
            runtime.question_pending(),
            None,
        );
    }
    let mut repl = Repl::new(runtime, cli.safe, cli.run);
    if use_tui {
        repl.run_with_tui().await?;
    } else {
        repl.run().await?;
    }
    Ok(())
}
