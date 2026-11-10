//! `nca add` / `nca plugin ...`: manage WASM plugins.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use nca_plugin::install::{self, ApprovalRequest, InstallOptions, UpdateOutcome};
use nca_plugin::registry::{Entry, Registry};

#[derive(clap::Subcommand, Debug)]
pub enum PluginCommand {
    /// List installed plugins
    List {
        #[arg(long)]
        json: bool,
    },
    /// Install a plugin from a git URL (same as `nca add`)
    Add(AddArgs),
    /// Show manifest, grants and integrity status of a plugin
    Info { name: String },
    /// Enable a plugin
    Enable { name: String },
    /// Disable a plugin without removing it
    Disable { name: String },
    /// Re-fetch plugins from their source (all when no name is given)
    Update {
        name: Option<String>,
        /// Approve changed hooks/capabilities without asking
        #[arg(short, long)]
        yes: bool,
    },
    /// Remove a plugin and its data
    Remove { name: String },
}

#[derive(clap::Args, Debug)]
pub struct AddArgs {
    /// Git URL (https or ssh, public or private), optionally pinned: `<url>@<tag|branch|sha>`.
    /// `owner/repo` expands to a GitHub URL. Private repos use your own git credentials.
    pub source: String,
    /// Install for the current workspace only instead of globally
    #[arg(long)]
    pub project: bool,
    /// Skip the approval prompt
    #[arg(short, long)]
    pub yes: bool,
}

fn workspace(workspace_root: &Path) -> anyhow::Result<PathBuf> {
    Ok(workspace_root.canonicalize()?)
}

/// Replace control characters (newlines, ANSI escapes) in untrusted text with `?`.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn prompt_approval(yes: bool) -> impl Fn(&ApprovalRequest<'_>) -> bool {
    move |req| {
        let m = req.manifest;
        // Manifest text comes from the plugin's repository: strip control characters so it
        // cannot move the cursor or fake the trust summary and prompt below.
        println!(
            "Plugin   {} v{}",
            printable(&m.plugin.name),
            printable(&m.plugin.version)
        );
        if let Some(desc) = &m.plugin.description {
            println!("         {}", printable(desc));
        }
        println!(
            "Source   {} @ {}",
            printable(req.source),
            &req.commit[..req.commit.len().min(12)]
        );
        println!("Wasm     sha256:{}", req.wasm_sha256);
        println!("Hooks    {}", m.hooks.names().join(", "));
        let caps = m.capabilities.describe();
        if caps.is_empty() {
            println!("Grants   none (sandboxed: no network, files or environment)");
        } else {
            println!("Grants");
            for line in caps {
                println!("  - {}", printable(&line));
            }
        }
        if let Some(prev) = req.previous
            && (prev.hooks != m.hooks
                || prev.capabilities != m.capabilities
                || prev.config != m.config)
        {
            println!("Changed  hooks, capabilities or config differ from the installed version");
            if prev.config != m.config {
                for (key, value) in &m.config {
                    if prev.config.get(key) != Some(value) {
                        println!("  config {} = {}", printable(key), printable(value));
                    }
                }
                for key in prev.config.keys().filter(|k| !m.config.contains_key(*k)) {
                    println!("  config {} removed", printable(key));
                }
            }
        }
        if yes {
            return true;
        }
        if !std::io::stdin().is_terminal() {
            eprintln!(
                "Refusing to install without confirmation; pass --yes in non-interactive use."
            );
            return false;
        }
        print!("Install and allow this plugin? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).is_ok()
            && matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}

pub fn add(args: &AddArgs, workspace_root: &Path) -> anyhow::Result<()> {
    let approve = prompt_approval(args.yes);
    let ws = if args.project {
        Some(workspace(workspace_root)?)
    } else {
        None
    };
    let entry = install::add(
        &args.source,
        &InstallOptions {
            workspace: ws,
            approve: &approve,
        },
    )?;
    println!(
        "Installed {} v{} ({}) to {}",
        entry.name,
        entry.version,
        if entry.workspace.is_some() {
            "project"
        } else {
            "global"
        },
        entry.dir.display()
    );
    Ok(())
}

/// Locate an installed plugin by name: this workspace's copy first, then global.
fn locate(name: &str, workspace_root: &Path) -> anyhow::Result<Entry> {
    let registry = Registry::load()?;
    let ws = workspace(workspace_root)?;
    registry
        .find(name, Some(&ws))
        .or_else(|| registry.find(name, None))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("plugin `{name}` is not installed"))
}

fn scope_of(entry: &Entry) -> Option<PathBuf> {
    entry.workspace.as_ref().map(PathBuf::from)
}

pub fn run(command: PluginCommand, workspace_root: &Path) -> anyhow::Result<()> {
    match command {
        PluginCommand::List { json } => list(json, workspace_root),
        PluginCommand::Add(args) => add(&args, workspace_root),
        PluginCommand::Info { name } => info(&name, workspace_root),
        PluginCommand::Enable { name } => set_enabled(&name, true, workspace_root),
        PluginCommand::Disable { name } => set_enabled(&name, false, workspace_root),
        PluginCommand::Update { name, yes } => update(name.as_deref(), yes, workspace_root),
        PluginCommand::Remove { name } => {
            let entry = locate(&name, workspace_root)?;
            install::remove(&entry.name, scope_of(&entry).as_deref())?;
            println!("Removed plugin: {name}");
            Ok(())
        }
    }
}

fn list(json: bool, workspace_root: &Path) -> anyhow::Result<()> {
    let ws = workspace(workspace_root)?;
    let registry = Registry::load()?;
    let entries: Vec<&Entry> = registry
        .plugins
        .iter()
        .filter(|e| e.applies_to(&ws))
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!("No plugins installed. Try: nca add <git-url>");
        return Ok(());
    }
    for e in entries {
        let hooks = e
            .load_verified()
            .map(|(m, _)| m.hooks.names().join(","))
            .unwrap_or_else(|err| format!("INVALID: {err}"));
        println!(
            "{:<20} v{:<10} {:<8} {:<8} {}  [{}]",
            e.name,
            e.version,
            if e.workspace.is_some() {
                "project"
            } else {
                "global"
            },
            if e.enabled { "enabled" } else { "disabled" },
            e.source,
            hooks
        );
    }
    Ok(())
}

fn info(name: &str, workspace_root: &Path) -> anyhow::Result<()> {
    let e = locate(name, workspace_root)?;
    println!("name       {}", e.name);
    println!("version    {}", e.version);
    println!("source     {}", e.source);
    if let Some(r) = &e.requested_ref {
        println!("ref        {r}");
    }
    println!("commit     {}", e.commit);
    println!("wasm       sha256:{}", e.wasm_sha256);
    println!("installed  {}", e.installed_at);
    println!("enabled    {}", e.enabled);
    println!("dir        {}", e.dir.display());
    match e.load_verified() {
        Ok((m, _)) => {
            println!("integrity  ok");
            println!("hooks      {}", m.hooks.names().join(", "));
            let caps = m.capabilities.describe();
            if caps.is_empty() {
                println!("grants     none");
            }
            for line in caps {
                println!("grant      {line}");
            }
        }
        Err(err) => println!("integrity  FAILED: {err}"),
    }
    Ok(())
}

fn set_enabled(name: &str, enabled: bool, workspace_root: &Path) -> anyhow::Result<()> {
    let e = locate(name, workspace_root)?;
    install::set_enabled(&e.name, scope_of(&e).as_deref(), enabled)?;
    println!(
        "{} plugin: {name}",
        if enabled { "Enabled" } else { "Disabled" }
    );
    Ok(())
}

fn update(name: Option<&str>, yes: bool, workspace_root: &Path) -> anyhow::Result<()> {
    let ws = workspace(workspace_root)?;
    let targets: Vec<Entry> = match name {
        Some(n) => vec![locate(n, workspace_root)?],
        None => Registry::load()?
            .plugins
            .into_iter()
            .filter(|e| e.applies_to(&ws))
            .collect(),
    };
    let approve = prompt_approval(yes);
    for e in targets {
        match install::update(&e.name, scope_of(&e).as_deref(), &approve)? {
            UpdateOutcome::Unchanged => println!("{}: up to date", e.name),
            UpdateOutcome::Updated(n) => println!(
                "{}: updated to v{} ({})",
                n.name,
                n.version,
                &n.commit[..12]
            ),
        }
    }
    Ok(())
}
