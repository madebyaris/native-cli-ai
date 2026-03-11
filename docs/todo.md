# TODO

## Done

- Built the core Rust-native CLI agent loop with MiniMax as the first-class provider.
- Added session-based CLI commands: `run`, `spawn`, `sessions`, `resume`, `logs`, `attach`, `status`, and `cancel`.
- Added human-readable and NDJSON streaming output.
- Added permission modes: `default`, `plan`, `accept-edits`, `dont-ask`, and `bypass-permissions`.
- Added web research tools: `web_search` and `fetch_url`.
- Added fast local code-intelligence with `query_symbols`.
- Added richer file and workflow tools: `apply_patch`, `edit_file`, `write_file`, `create_directory`, `rename_path`, `move_path`, `copy_path`, `delete_path`, `git_status`, `git_diff`, and `run_validation`.
- Added persisted session metadata, token/cost tracking, and IPC socket support for live session control.
- Updated the docs for CLI usage, parity progress, and architecture.

## Next Up: Desktop Monitor

- Define the desktop MVP scope for `nca-monitor`.
- Show live session list with running/completed/cancelled status.
- Connect the monitor to session IPC sockets for live event streaming.
- Add a session detail view for messages, tool calls, checkpoints, and costs.
- Add a log viewer for NDJSON event history.
- Add approval controls for ask-tier tool calls.
- Add a diff panel for file-edit activity.
- Add controls for attach, cancel, and resume from the desktop UI.
- Surface model, token, and cost metadata clearly.
- Make the monitor work well with multiple concurrent sessions.

## CLI + Desktop Integration

- Verify `spawn`, `status`, `attach`, and `cancel` work cleanly with the monitor.
- Normalize event schemas used by both CLI and desktop.
- Make session metadata robust for reconnects after app restarts.
- Improve IPC error handling and reconnect behavior.

## Later

- Add richer session search and filtering.
- Add desktop notifications for approvals and finished runs.
- Add tmux or multiplexer awareness in the monitor.
- Add saved layouts and per-session tabs.

