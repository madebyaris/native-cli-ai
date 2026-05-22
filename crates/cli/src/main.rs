#![allow(
    clippy::all,
    clippy::pedantic,
    dead_code,
    unused_imports,
    unused_variables
)]

mod approval_prompts;
mod cli;
mod cli_index;
mod cmd;
mod stream;

use crate::cli::{
    AutoresearchCmd, Cli, CliPermissionMode, Command, CompletionCmd, ExportArgs, ExportFormat,
    IndexCmd, InlineImages, MemoryCommand, SkillsCommand,
};
use crate::cmd::attach::{attach_session, show_logs};
use crate::cmd::autoresearch::autoresearch_once;
use crate::cmd::completion::{generate_shell_completion, install_shell_completion};
use crate::cmd::doctor::show_doctor;
use crate::cmd::export::run_export;
use crate::cmd::index::{run_index_rebuild, run_index_search};
use crate::cmd::init::{run_init, show_config};
use crate::cmd::interactive::run_default;
use crate::cmd::mcp::list_mcp_servers;
use crate::cmd::memory::{add_memory_note, show_memory};
use crate::cmd::models::show_models;
use crate::cmd::run::{OneShotOptions, run_one_shot, run_service_session};
use crate::cmd::sessions::{cancel_session, list_sessions, resume_session, run_cost_dashboard};
use crate::cmd::skills::{
    handle_skills_add, handle_skills_remove, handle_skills_update, list_skills,
};
use crate::cmd::spawn::spawn_run;
use crate::cmd::upgrade::run_upgrade;
use clap::Parser;
use nca_common::config::NcaConfig;
use nca_common::session::OrchestrationContext;
use std::path::PathBuf;
use std::process::ExitCode;

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
    if let Some(max_turns) = cli.max_turns {
        config.session.max_turns_per_run = max_turns;
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
                OneShotOptions {
                    stream,
                    json,
                    safe,
                    session_id,
                    orchestration_context: orchestration_context.clone(),
                },
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
        Some(Command::Sessions {
            json,
            status,
            since_hours,
            search,
            limit,
        }) => {
            list_sessions(
                &config,
                &workspace_root,
                json,
                status,
                since_hours,
                search,
                limit,
            )
            .await?;
        }
        Some(Command::Resume {
            session_id,
            prompt,
            model,
            safe,
            stream,
            no_tui,
            permission_mode,
        }) => {
            if let Some(model) = model {
                config.apply_model_override(&model);
            }
            if let Some(mode) = permission_mode {
                config.permissions.mode = mode.into();
            }
            resume_session(
                config,
                &workspace_root,
                &session_id,
                prompt,
                safe,
                stream,
                no_tui,
            )
            .await?;
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
            cmd::sessions::show_status(&config, &workspace_root, &session_id, json).await?;
        }
        Some(Command::Cancel { session_id, json }) => {
            cancel_session(&config, &workspace_root, &session_id, json).await?;
        }
        Some(Command::Skills { json, command }) => match command {
            None => {
                list_skills(&config, &workspace_root, json)?;
            }
            Some(SkillsCommand::List { json: j }) => {
                list_skills(&config, &workspace_root, j || json)?;
            }
            Some(SkillsCommand::Add {
                source,
                skill,
                global,
            }) => {
                handle_skills_add(&source, &skill, global, &workspace_root)?;
            }
            Some(SkillsCommand::Remove { name, global }) => {
                handle_skills_remove(&name, global, &workspace_root)?;
            }
            Some(SkillsCommand::Update { name }) => {
                handle_skills_update(name.as_deref(), &workspace_root)?;
            }
        },
        Some(Command::Mcp { json }) => {
            list_mcp_servers(&config, json)?;
        }
        Some(Command::Memory { command, json }) => match command {
            MemoryCommand::List => show_memory(&config, &workspace_root, json).await?,
            MemoryCommand::Add { text, kind } => {
                add_memory_note(&config, &workspace_root, &kind, &text, json).await?
            }
        },
        Some(Command::Models { json, verbose }) => {
            show_models(&config, json, verbose)?;
        }
        Some(Command::Doctor { json, fix }) => {
            show_doctor(&config, &workspace_root, json, fix).await?;
        }
        Some(Command::Config { json }) => {
            show_config(&config, &workspace_root, json)?;
        }
        Some(Command::Init { force }) => {
            run_init(&workspace_root, force)?;
        }
        Some(Command::Upgrade {
            install_dir,
            no_test,
        }) => {
            run_upgrade(install_dir, no_test)?;
        }
        Some(Command::Completion { command }) => match command {
            CompletionCmd::Generate { shell } => generate_shell_completion(shell),
            CompletionCmd::Install { shell, path } => install_shell_completion(shell, path)?,
        },
        Some(Command::Index { command }) => match command {
            IndexCmd::Build { json } => {
                cli_index::run_index_build(&workspace_root, json).await?;
            }
            IndexCmd::Show { json } => {
                cli_index::run_index_show(&workspace_root, json).await?;
            }
            IndexCmd::Rebuild { include, json } => {
                run_index_rebuild(&workspace_root, &include, json).await?;
            }
            IndexCmd::Search { query, limit, json } => {
                run_index_search(&workspace_root, &query, limit, json).await?;
            }
        },
        Some(Command::Autoresearch { command }) => match command {
            AutoresearchCmd::Once { program, workspace } => {
                let ws = workspace.unwrap_or_else(|| workspace_root.clone());
                autoresearch_once(program, ws).await?;
            }
        },
        Some(Command::Cost {
            since_hours,
            limit,
            json,
        }) => {
            run_cost_dashboard(&config, &workspace_root, since_hours, limit, json).await?;
        }
        Some(Command::Export {
            session_id,
            format,
            output,
            include_system,
            include_tool_results,
            inline_images,
        }) => {
            let inline = match inline_images {
                InlineImages::Auto => matches!(format, ExportFormat::Html),
                InlineImages::On => true,
                InlineImages::Off => false,
            };
            run_export(
                &config,
                &workspace_root,
                &session_id,
                ExportArgs {
                    format,
                    output,
                    include_system,
                    include_tool_results,
                    inline_images: inline,
                },
            )
            .await?;
        }
        None => {
            run_default(&cli, config, &workspace_root, orchestration_context).await?;
        }
    }

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
