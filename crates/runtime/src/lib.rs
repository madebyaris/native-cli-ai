//! Session supervision, IPC, persistence, PTY-backed execution, and worktree isolation.
//!
//! Owns the [`supervisor::Supervisor`] lifecycle used by the CLI and TUI.

#![allow(clippy::pedantic)]

/// PTY-backed bash tool registered with the agent loop.
pub mod bash_tool;
/// Conversation context window management and auto-summarize.
pub mod context_manager;
/// Unix-socket IPC server and client.
pub mod ipc;
/// Last-active session pointer per workspace.
pub mod last_session;
/// Workspace memory note store.
pub mod memory_store;
/// Static model context-window limits.
pub mod model_limits;
/// Optional API-backed model limit lookup.
pub mod model_limits_api;
/// Sandboxed process helpers.
pub mod process;
/// Real PTY command execution via `portable-pty`.
pub mod pty;
/// Detached service session thread entrypoint.
pub mod service;
/// Session JSON + JSONL persistence.
pub mod session_store;
/// Session lifecycle manager (create, resume, run turns, IPC).
pub mod supervisor;
/// Tmux multiplexer adapter (optional).
pub mod tmux;
/// Git worktree creation and cleanup for isolated child runs.
pub mod worktree;
