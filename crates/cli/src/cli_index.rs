//! Generated CLI index for agents (`nca index build` / `show`).

use clap::Command as ClapCommand;
use clap::CommandFactory;
use nca_common::config::{workspace_cache_id, workspace_cli_index_path};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::Cli;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct CliIndexFile {
    pub schema_version: u32,
    pub workspace_root: String,
    pub workspace_id: String,
    pub generated_at: String,
    pub cli_entrypoint: String,
    pub global_flags: Vec<IndexFlag>,
    pub commands: Vec<IndexCommand>,
    pub areas: BTreeMap<String, serde_json::Value>,
    pub rules: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct IndexFlag {
    pub long: Option<String>,
    pub short: Option<String>,
    pub help: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IndexCommand {
    pub name: String,
    pub path: Vec<String>,
    pub hidden: bool,
    pub summary: Option<String>,
    pub flags: Vec<IndexFlag>,
    pub subcommands: Vec<String>,
    pub handler_hint: String,
    pub touchpoints: Vec<String>,
}

pub fn build_cli_index(workspace_root: &Path) -> anyhow::Result<CliIndexFile> {
    let (workspace_id, canonical) =
        workspace_cache_id(workspace_root).map_err(|e| anyhow::anyhow!("{e}"))?;
    let root = Cli::command();
    let global_flags = flags_from_command(&root);
    let mut commands = Vec::new();
    for sub in root.get_subcommands() {
        let path = vec![sub.get_name().to_string()];
        walk_subcommand(sub, path, &mut commands);
    }

    Ok(CliIndexFile {
        schema_version: SCHEMA_VERSION,
        workspace_root: canonical.to_string_lossy().into_owned(),
        workspace_id,
        generated_at: chrono::Utc::now().to_rfc3339(),
        cli_entrypoint: "crates/cli/src/main.rs".into(),
        global_flags,
        commands,
        areas: default_areas(),
        rules: default_rules(),
    })
}

pub async fn run_index_build(workspace_root: &Path, json_status: bool) -> anyhow::Result<()> {
    let path = workspace_cli_index_path(workspace_root).map_err(|e| anyhow::anyhow!("{e}"))?;
    let index = build_cli_index(workspace_root)?;
    let bytes = serde_json::to_vec_pretty(&index)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, &bytes).await?;
    if json_status {
        let status = serde_json::json!({
            "path": path.to_string_lossy(),
            "workspace_id": index.workspace_id,
            "schema_version": index.schema_version,
        });
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("wrote {}", path.display());
    }
    Ok(())
}

pub async fn run_index_show(workspace_root: &Path, json: bool) -> anyhow::Result<()> {
    let path = workspace_cli_index_path(workspace_root).map_err(|e| anyhow::anyhow!("{e}"))?;
    let raw = tokio::fs::read_to_string(&path).await.map_err(|_| {
        anyhow::anyhow!(
            "no CLI index at {} (run `nca index build` first)",
            path.display()
        )
    })?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("invalid index JSON at {}: {e}", path.display()))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        let n = v["commands"].as_array().map(|a| a.len()).unwrap_or(0);
        println!(
            "CLI index  schema={}  workspace_id={}",
            v["schema_version"], v["workspace_id"]
        );
        println!("path: {}", path.display());
        println!("commands (entries): {n}");
    }
    Ok(())
}

fn walk_subcommand(cmd: &ClapCommand, path: Vec<String>, out: &mut Vec<IndexCommand>) {
    out.push(entry_from_cmd(cmd, &path));
    for sub in cmd.get_subcommands() {
        let mut p = path.clone();
        p.push(sub.get_name().to_string());
        walk_subcommand(sub, p, out);
    }
}

fn entry_from_cmd(cmd: &ClapCommand, path: &[String]) -> IndexCommand {
    let name = path.last().cloned().unwrap_or_default();
    let hidden = cmd.is_hide_set();
    let summary = cmd
        .get_about()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let flags = flags_from_command(cmd);
    let subcommands = cmd
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    let path_key = path.join("/");
    IndexCommand {
        name,
        path: path.to_vec(),
        hidden,
        summary,
        flags,
        subcommands,
        handler_hint: handler_hint(&path_key),
        touchpoints: touchpoints_for(&path_key),
    }
}

fn flags_from_command(cmd: &ClapCommand) -> Vec<IndexFlag> {
    cmd.get_arguments()
        .filter(|a| {
            let id = a.get_id().as_str();
            id != "help" && id != "version"
        })
        .filter(|a| !a.is_hide_set())
        .map(|a| IndexFlag {
            long: a.get_long().map(str::to_string),
            short: a.get_short().map(|c| c.to_string()),
            help: a
                .get_help()
                .map(|h| h.to_string())
                .filter(|s| !s.is_empty()),
        })
        .collect()
}

fn handler_hint(path_key: &str) -> String {
    match path_key {
        "run" => "crates/cli/src/main.rs: run_one_shot".into(),
        "serve" => "crates/cli/src/main.rs: run_service_session".into(),
        "spawn" => "crates/cli/src/main.rs: spawn_run".into(),
        "sessions" => "crates/cli/src/main.rs: list_sessions".into(),
        "resume" => "crates/cli/src/main.rs: resume_session".into(),
        "logs" => "crates/cli/src/main.rs: show_logs".into(),
        "attach" => "crates/cli/src/main.rs: attach_session".into(),
        "status" => "crates/cli/src/main.rs: show_status".into(),
        "cancel" => "crates/cli/src/main.rs: cancel_session".into(),
        "skills" => "crates/cli/src/main.rs: list_skills".into(),
        "mcp" => "crates/cli/src/main.rs: list_mcp_servers".into(),
        "memory/list" | "memory/add" => {
            "crates/cli/src/main.rs: show_memory / add_memory_note".into()
        }
        "models" => "crates/cli/src/main.rs: show_models".into(),
        "doctor" => "crates/cli/src/main.rs: show_doctor".into(),
        "config" => "crates/cli/src/main.rs: show_config".into(),
        "completion" => "crates/cli/src/main.rs: generate_shell_completion".into(),
        "autoresearch/once" => "crates/cli/src/main.rs: autoresearch_once".into(),
        "index/build" | "index/show" => "crates/cli/src/cli_index.rs".into(),
        _ if path_key.starts_with("memory/") => "crates/cli/src/main.rs: try_main (Memory)".into(),
        _ if path_key.starts_with("autoresearch/") => {
            "crates/cli/src/main.rs: autoresearch_once".into()
        }
        _ => "crates/cli/src/main.rs: try_main".into(),
    }
}

fn touchpoints_for(path_key: &str) -> Vec<String> {
    let mut v = vec![
        "crates/cli/src/main.rs".into(),
        "crates/cli/tests/cli_commands.rs".into(),
    ];
    match path_key {
        k if k == "run" || k == "resume" || k.starts_with("resume") => {
            v.push("crates/cli/src/runner.rs".into());
            v.push("crates/cli/src/repl.rs".into());
            v.push("crates/runtime/src/supervisor.rs".into());
        }
        "sessions" | "logs" | "attach" | "status" | "cancel" => {
            v.push("crates/runtime/src/session_store.rs".into());
        }
        "skills" => v.push("crates/core/src/skills.rs".into()),
        "mcp" => v.push("crates/core/src/tools/mcp.rs".into()),
        "doctor" | "config" | "models" => {
            v.push("crates/common/src/config.rs".into());
        }
        "memory/list" | "memory/add" => {
            v.push("crates/runtime/src/memory_store.rs".into());
        }
        _ => {
            v.push("crates/cli/src/stream.rs".into());
            v.push("crates/cli/src/tui/".into());
        }
    }
    v
}

fn default_areas() -> BTreeMap<String, serde_json::Value> {
    let mut m = BTreeMap::new();
    m.insert(
        "cli".into(),
        serde_json::json!({
            "define_in": ["crates/cli/src/main.rs"],
            "handlers_in": ["crates/cli/src/main.rs", "crates/cli/src/runner.rs", "crates/cli/src/repl.rs"],
            "stream_render": ["crates/cli/src/stream.rs", "crates/cli/src/tui/"],
            "slash_commands": ["crates/cli/src/slash_commands.rs"],
        }),
    );
    m.insert(
        "runtime".into(),
        serde_json::json!({
            "supervisor": "crates/runtime/src/supervisor.rs",
            "sessions": "crates/runtime/src/session_store.rs",
            "ipc": "crates/runtime/src/ipc.rs",
        }),
    );
    m.insert(
        "common".into(),
        serde_json::json!({
            "config": "crates/common/src/config.rs",
            "events": "crates/common/src/event.rs",
        }),
    );
    m.insert(
        "core".into(),
        serde_json::json!({
            "providers": "crates/core/src/provider/",
            "tools": "crates/core/src/tools/",
        }),
    );
    m
}

fn default_rules() -> Vec<String> {
    vec![
        "New top-level subcommand: add variant to Command in main.rs, match arm in try_main, optional test in cli_commands.rs.".into(),
        "New AgentEvent: update crates/common/src/event.rs, emit in runtime, render in stream.rs and tui/; check tui/replay.rs.".into(),
        "Session persistence: SessionStore under workspace .nca/sessions; paths from SessionConfig in config.rs.".into(),
        "CLI index cache: $XDG_DATA_HOME/nca/workspaces/<workspace-id>/cli-index.json via nca_common::config helpers.".into(),
    ]
}
