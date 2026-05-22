//! CLI argument definitions (clap derive structs).

use crate::stream::StreamMode;
use clap::Parser;
use nca_common::config::PermissionMode;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "nca",
    about = "Native CLI AI - a Rust-powered coding assistant"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// One-shot prompt mode
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Start in read-only safe mode
    #[arg(short, long)]
    pub safe: bool,

    /// Resume the last session
    #[arg(short, long)]
    pub resume: bool,

    /// Start a new session instead of resuming the last one
    #[arg(long)]
    pub no_resume: bool,

    /// Start interactive run mode (Claude-style)
    #[arg(long)]
    pub run: bool,

    /// Override the default model
    #[arg(short = 'm', long)]
    pub model: Option<String>,

    /// Enable extended thinking
    #[arg(short = 't', long)]
    pub enable_thinking: bool,

    /// Token budget for extended thinking
    #[arg(long, default_value = "5120")]
    pub thinking_budget: u32,

    /// Max response tokens
    #[arg(long, default_value = "8192")]
    pub max_tokens: u32,

    /// Verbose debug logging
    #[arg(short, long)]
    pub verbose: bool,

    /// Output structured JSON (for CI)
    #[arg(long)]
    pub json: bool,

    /// Streaming output format
    #[arg(short = 'S', long, value_enum, default_value_t = StreamMode::Human)]
    pub stream: StreamMode,

    /// Line-oriented REPL instead of full-screen TUI (scripts, CI, or broken approval prompts)
    #[arg(long)]
    pub no_tui: bool,

    /// Permission handling mode (default: from config, fallback to `default`)
    #[arg(long, value_enum)]
    pub permission_mode: Option<CliPermissionMode>,

    /// Max turns per run (overrides config)
    #[arg(long)]
    pub max_turns: Option<u32>,

    /// Internal session identifier for spawned runs
    #[arg(long, hide = true)]
    pub session_id: Option<String>,

    /// In one-shot mode, consume stdin and combine it with `--prompt`.
    /// Selector controls how stdin is merged with the prompt:
    ///   - `append` (default): `<prompt>\n\n<stdin>`
    ///   - `prefix`: `<stdin>\n\n<prompt>`
    ///   - `only`:   use stdin as the entire prompt (ignore `--prompt`)
    #[arg(long = "stdin-as", value_enum)]
    pub stdin_as: Option<StdinMerge>,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum StdinMerge {
    Append,
    Prefix,
    Only,
}

#[derive(clap::Subcommand, Debug)]
pub enum Command {
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
        // Serve (IPC-driven) defaults to accept-edits for non-interactive service sessions
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
        /// Filter sessions by status (running, completed, cancelled, failed)
        #[arg(long, value_enum)]
        status: Option<SessionStatusFilter>,
        /// Filter sessions updated in the last N hours
        #[arg(long)]
        since_hours: Option<u32>,
        /// Search sessions by content/pattern
        #[arg(long)]
        search: Option<String>,
        /// Limit number of sessions shown
        #[arg(long, default_value = "20")]
        limit: usize,
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
        #[arg(long)]
        no_tui: bool,
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
    /// Manage skills: list, add, remove, update
    Skills {
        /// Output as JSON (shorthand for `nca skills list --json`)
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        command: Option<SkillsCommand>,
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
        /// Include transport/retry settings (empty-response retries + backoff).
        #[arg(long)]
        verbose: bool,
    },
    Doctor {
        #[arg(long)]
        json: bool,
        /// Attempt to auto-fix common misconfigurations (creates .nca/, seeds defaults, etc).
        #[arg(long)]
        fix: bool,
    },
    Config {
        #[arg(long)]
        json: bool,
    },
    /// Scaffold `.nca/` in the current workspace with sane defaults.
    Init {
        /// Overwrite existing workspace config files.
        #[arg(long)]
        force: bool,
    },
    /// Upgrade the nca binary (cargo install --path . from the local source tree).
    Upgrade {
        /// Install path (defaults to `/usr/local/bin`).
        #[arg(long)]
        install_dir: Option<PathBuf>,
        /// Skip running tests before install.
        #[arg(long)]
        no_test: bool,
    },
    /// Shell completion generation and installation
    Completion {
        #[command(subcommand)]
        command: CompletionCmd,
    },
    /// Autonomous research helpers (see `crates/autoresearch`, program `.md` files).
    Autoresearch {
        #[command(subcommand)]
        command: AutoresearchCmd,
    },
    /// Build or show a cached CLI index under ~/.nca/workspaces/<id>/ (for agents and tooling).
    Index {
        #[command(subcommand)]
        command: IndexCmd,
    },
    /// Show cumulative token usage and estimated cost across recent sessions.
    Cost {
        /// Only consider sessions updated within the last N hours.
        #[arg(long)]
        since_hours: Option<u32>,
        /// Show at most N sessions (default: 20).
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Emit machine-readable JSON instead of the human table.
        #[arg(long)]
        json: bool,
    },
    /// Export a session transcript to Markdown, JSON, or HTML (with inline images
    /// for vision sessions).
    Export {
        /// Session id, or a unique id prefix (exactly one stored id may match).
        session_id: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ExportFormat::Markdown)]
        format: ExportFormat,
        /// Write to a file instead of stdout.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        /// Include `system` messages in the export (off by default).
        #[arg(long)]
        include_system: bool,
        /// Include tool-result messages in the export (off by default).
        #[arg(long)]
        include_tool_results: bool,
        /// Whether to embed vision attachments as base64 data URIs.
        /// `auto`: on for HTML, off for Markdown.
        #[arg(long, value_enum, default_value_t = InlineImages::Auto)]
        inline_images: InlineImages,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, Default)]
pub enum InlineImages {
    /// Inline for HTML only; Markdown uses file paths / links.
    #[default]
    Auto,
    On,
    Off,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ExportFormat {
    Markdown,
    Json,
    Html,
}

pub struct ExportArgs {
    pub format: ExportFormat,
    pub output: Option<PathBuf>,
    pub include_system: bool,
    pub include_tool_results: bool,
    pub inline_images: bool,
}

#[derive(clap::Subcommand, Debug)]
pub enum IndexCmd {
    /// Generate `cli-index.json` for the current workspace (canonical path → stable id).
    Build {
        /// Print JSON status (path, workspace_id) instead of a one-line message
        #[arg(long)]
        json: bool,
    },
    /// Print the last generated index (requires `index build` first).
    Show {
        /// Pretty-print full JSON; otherwise print a short summary
        #[arg(long)]
        json: bool,
    },
    /// Rebuild the BM25 code search index at `.nca/index/`.
    /// Only available when built with `--features semantic-index`.
    Rebuild {
        /// Glob patterns for files to include (e.g. `*.rs`). Empty = everything
        /// outside standard ignore dirs (`target/`, `node_modules/`, `.git/`).
        #[arg(long)]
        include: Vec<String>,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Query the BM25 code search index.
    Search {
        query: String,
        #[arg(long, default_value = "10")]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum AutoresearchCmd {
    /// Run the program's metric shell command once and print the parsed metric.
    Once {
        /// Path to research program markdown (e.g. `docs/research/cli-dx-research.md`)
        program: PathBuf,
        /// Working directory (defaults to current directory)
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ClapShell {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Elvish,
}

#[derive(clap::Subcommand, Debug)]
pub enum CompletionCmd {
    /// Print a completion script to stdout (default: bash).
    Generate {
        #[arg(value_enum, default_value_t = ClapShell::Bash)]
        shell: ClapShell,
    },
    /// Install completions into the appropriate user-level location.
    Install {
        /// Shell to install completions for (defaults to the value of $SHELL).
        #[arg(value_enum)]
        shell: Option<ClapShell>,
        /// Override the install path (defaults to a shell-specific well-known location).
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum MemoryCommand {
    List,
    Add {
        text: String,
        #[arg(long, default_value = "note")]
        kind: String,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum SkillsCommand {
    /// List installed skills
    List {
        #[arg(long)]
        json: bool,
    },
    /// Install skills from a GitHub repo or local path
    Add {
        /// Source: owner/repo, GitHub URL, or local path
        source: String,
        /// Install specific skills by name (default: all)
        #[arg(short, long)]
        skill: Vec<String>,
        /// Install to ~/.nca/skills/ instead of .nca/skills/
        #[arg(short, long)]
        global: bool,
    },
    /// Remove an installed skill
    Remove {
        /// Skill command name to remove
        name: String,
        /// Remove from ~/.nca/skills/ instead of .nca/skills/
        #[arg(short, long)]
        global: bool,
    },
    /// Update installed skills from their source
    Update {
        /// Specific skill to update (default: all)
        name: Option<String>,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum CliPermissionMode {
    Default,
    Plan,
    AcceptEdits,
    DontAsk,
    BypassPermissions,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum SessionStatusFilter {
    Running,
    Completed,
    Cancelled,
    Failed,
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
    pub fn as_arg(self) -> &'static str {
        match self {
            CliPermissionMode::Default => "default",
            CliPermissionMode::Plan => "plan",
            CliPermissionMode::AcceptEdits => "accept-edits",
            CliPermissionMode::DontAsk => "dont-ask",
            CliPermissionMode::BypassPermissions => "bypass-permissions",
        }
    }
}

#[cfg(test)]
mod tests {
    pub use super::*;
    pub use clap::Parser;

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

    #[test]
    fn parses_run_subcommand_args() {
        let cli = Cli::try_parse_from([
            "nca",
            "run",
            "--prompt",
            "hello",
            "--stream",
            "ndjson",
            "--json",
            "--safe",
            "--permission-mode",
            "accept-edits",
            "--session-id",
            "session-123",
        ])
        .expect("should parse run subcommand");

        match cli.command {
            Some(Command::Run {
                prompt,
                stream,
                json,
                safe,
                permission_mode,
                session_id,
                model,
            }) => {
                assert_eq!(prompt, "hello");
                assert!(matches!(stream, StreamMode::Ndjson));
                assert!(json);
                assert!(safe);
                assert_eq!(permission_mode, Some(CliPermissionMode::AcceptEdits));
                assert_eq!(session_id.as_deref(), Some("session-123"));
                assert!(model.is_none());
            }
            _ => panic!("expected run subcommand"),
        }
    }

    #[test]
    fn parses_spawn_subcommand_args() {
        let cli = Cli::try_parse_from([
            "nca",
            "spawn",
            "--prompt",
            "task",
            "--model",
            "MiniMax-M2.5",
            "--safe",
            "--json",
            "--permission-mode",
            "dont-ask",
        ])
        .expect("should parse spawn subcommand");

        match cli.command {
            Some(Command::Spawn {
                prompt,
                model,
                safe,
                json,
                permission_mode,
            }) => {
                assert_eq!(prompt, "task");
                assert_eq!(model.as_deref(), Some("MiniMax-M2.5"));
                assert!(safe);
                assert!(json);
                assert_eq!(permission_mode, Some(CliPermissionMode::DontAsk));
            }
            _ => panic!("expected spawn subcommand"),
        }
    }

    #[test]
    fn parses_attach_subcommand_args() {
        let cli = Cli::try_parse_from(["nca", "attach", "session-abc", "--json"])
            .expect("should parse attach subcommand");

        match cli.command {
            Some(Command::Attach { session_id, json }) => {
                assert_eq!(session_id, "session-abc");
                assert!(json);
            }
            _ => panic!("expected attach subcommand"),
        }
    }

    #[test]
    fn parses_logs_subcommand_args() {
        let cli = Cli::try_parse_from(["nca", "logs", "session-xyz", "--follow", "--json"])
            .expect("should parse logs subcommand");

        match cli.command {
            Some(Command::Logs {
                session_id,
                follow,
                json,
            }) => {
                assert_eq!(session_id, "session-xyz");
                assert!(follow);
                assert!(json);
            }
            _ => panic!("expected logs subcommand"),
        }
    }

    #[test]
    fn parses_cancel_subcommand_args() {
        let cli = Cli::try_parse_from(["nca", "cancel", "session-dead", "--json"])
            .expect("should parse cancel subcommand");

        match cli.command {
            Some(Command::Cancel { session_id, json }) => {
                assert_eq!(session_id, "session-dead");
                assert!(json);
            }
            _ => panic!("expected cancel subcommand"),
        }
    }

    #[test]
    fn parses_index_subcommand_args() {
        let cli = Cli::try_parse_from([
            "nca",
            "index",
            "search",
            "query text",
            "--limit",
            "5",
            "--json",
        ])
        .expect("should parse index subcommand");

        match cli.command {
            Some(Command::Index { command }) => match command {
                IndexCmd::Search { query, limit, json } => {
                    assert_eq!(query, "query text");
                    assert_eq!(limit, 5);
                    assert!(json);
                }
                _ => panic!("expected index search subcommand"),
            },
            _ => panic!("expected index subcommand"),
        }
    }
}
