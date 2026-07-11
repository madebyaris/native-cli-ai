//! Terminal UI layer for `nca`: full-screen TUI, line-oriented REPL, and the
//! input helpers (file mentions, slash commands, image attach, prompt config)
//! shared between them.
//!
//! Extracted from `nca-cli` so the CLI entrypoint only contains argument
//! parsing, session bootstrap, stream mode, and glue.

pub mod file_mentions;
pub mod image_attach;
pub mod ipc_pending;
pub mod prompt;
pub mod repl;
pub mod runner;
pub mod slash_commands;
pub mod tui;

pub use repl::Repl;
pub use runner::{
    SessionRuntime, build_resumed_session_runtime, build_session_runtime, dispatch_question_answer,
    dispatch_tool_approval,
};
pub use tui::{
    DisplayBlock, ModelPickerAction, ModelPickerEntry, TuiCmd, TuiSessionState, git_create_branch,
    git_current_branch, git_list_branches, git_switch_branch, replay_event_log_into_state,
    run_blocking, spawn_tui_bridge,
};
