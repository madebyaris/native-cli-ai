//! TUI command protocol, terminal setup, and git helpers.

use std::io::{Stdout, stdout};
use std::path::Path;

use crossterm::{
    cursor::{Hide, Show},
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use nca_common::config::ProviderKind;
use ratatui::{Terminal, backend::CrosstermBackend};

/// Message from TUI to the approval dispatch task.
#[derive(Debug)]
#[allow(dead_code)] // variants will be wired when Elm approval popup is implemented
pub enum ApprovalAnswer {
    Verdict { call_id: String, approved: bool },
    AllowPattern { call_id: String, pattern: String },
}

/// Commands dispatched from the Elm TUI to the runtime (via `repl.rs`).
#[derive(Debug)]
#[allow(dead_code)] // some variants not yet wired in Elm (QuestionAnswer, CycleAgent, Exit, OpenBranchPicker)
pub enum TuiCmd {
    Submit(String),
    /// Answer for the current `ask_question` (from question mode or `/auto-answer`).
    QuestionAnswer(nca_common::event::QuestionSelection),
    CycleAgent,
    CancelTurn,
    Exit,
    /// Open the branch picker popup.
    OpenBranchPicker,
    /// Switch to the given branch name.
    SwitchBranch(String),
    /// Create a new branch with the given name and switch to it.
    CreateBranch(String),
    /// Apply workspace default provider (from TUI picker).
    ApplyDefaultProvider(ProviderKind),
    /// Open API key modal for provider; bool indicates whether to connect after save/confirm.
    PromptApiKey(ProviderKind, bool),
    /// Apply a model name (from the model picker).
    ApplyModel(String),
    /// Switch provider (from the model picker).
    ApplyModelProvider(ProviderKind),
    /// Apply permission mode (from the permission picker).
    ApplyPermission(usize),
    /// Switch agent profile (from the agent picker).
    SwitchAgent(usize),
    /// Open external editor via leader key.
    OpenEditor,
    /// Start a new session.
    NewSession,
    /// Run compact.
    RunCompact,
    /// Open model picker (triggered by leader key or command palette).
    OpenModelPicker,
    /// Open status info modal.
    OpenStatus,
    /// Open help info modal.
    OpenHelp,
    /// Open agent picker.
    OpenAgentPicker,

    /// Open sessions picker/info.
    OpenSessions,
    /// Cycle to the next recent model (F2 forward, Shift+F2 backward).
    CycleModel(bool),
    /// Validate an API key for onboarding (provider, api_key).
    /// The repl handler looks up base_url from config.
    ValidateApiKey(ProviderKind, String),

    /// Resume a different session by ID.
    ResumeSession(String),
}

/// Run a git command synchronously and return stdout.
fn git_run(args: &[&str], cwd: Option<&Path>) -> Option<String> {
    let cwd = cwd?;
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Get the current git branch name for `workspace`.
pub fn git_current_branch(workspace: &Path) -> Option<String> {
    git_run(&["rev-parse", "--abbrev-ref", "HEAD"], Some(workspace))
}

/// List local git branches for `workspace`. Current branch is marked with `*`.
pub fn git_list_branches(workspace: &Path) -> Vec<String> {
    git_run(&["branch", "--no-color"], Some(workspace))
        .map(|out| {
            out.lines()
                .map(|l| {
                    l.trim_start_matches("* ")
                        .trim_start_matches("+ ")
                        .trim()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Create a new branch `name` and check it out in `workspace`.
pub fn git_create_branch(workspace: &Path, name: &str) -> bool {
    git_run(&["checkout", "-b", name], Some(workspace)).is_some()
}

/// Switch to an existing branch `name` in `workspace`.
pub fn git_switch_branch(workspace: &Path, name: &str) -> bool {
    git_run(&["checkout", name], Some(workspace)).is_some()
}

pub fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().map_err(|e| anyhow::anyhow!("enable_raw_mode: {e}"))?;
    let res: anyhow::Result<Terminal<CrosstermBackend<Stdout>>> = (|| {
        let mut out = stdout();
        execute!(out, EnterAlternateScreen)?;
        execute!(out, EnableMouseCapture)?;
        execute!(out, EnableBracketedPaste)?;
        execute!(out, Hide)?;
        execute!(out, Clear(ClearType::All))?;
        Ok(Terminal::new(CrosstermBackend::new(out))?)
    })();
    if res.is_err() {
        let _ = disable_raw_mode();
    }
    res
}

pub fn restore_terminal() {
    let mut out = stdout();
    let _ = execute!(out, Show);
    let _ = execute!(out, DisableMouseCapture);
    let _ = execute!(out, DisableBracketedPaste);
    let _ = execute!(out, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}
