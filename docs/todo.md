# TODO

## Done

- Built the core Rust-native CLI agent loop with MiniMax as the first-class provider.
- Added session-based CLI commands: `run`, `spawn`, `sessions`, `resume`, `logs`, `attach`, `status`, and `cancel`.
- Added human-readable and NDJSON streaming output.
- Added permission modes: `default`, `plan`, `accept-edits`, `dont-ask`, and `bypass-permissions`.
- Added web research tools: `web_search` and `fetch_url`.
- Added fast local code-intelligence with `query_symbols`.
- Added richer file and workflow tools: `apply_patch`, `edit_file`, `write_file`, `create_directory`, `rename_path`, `move_path`, `copy_path`, `delete_path`, `git_status`, `git_diff`, and `run_validation`.
- Upgraded `search_code` to structured ripgrep JSON results with explicit literal/regex controls and empty-result success semantics.
- Added `replace_match` for precise search-result-based edits, and hardened `edit_file` / `apply_patch` to reject ambiguous single-match replacements.
- Added persisted session metadata, token/cost tracking, and IPC socket support for live session control.
- Updated the docs for CLI usage, parity progress, and architecture.

## CLI + runtime hardening

- Verify `spawn`, `status`, `attach`, and `cancel` under automation (`--json` / NDJSON).
- Normalize event schemas for stable JSON/NDJSON consumers.
- Improve IPC error handling and reconnect behavior.

## Later

- Add richer session search and filtering in CLI.
- Tmux / multiplexer awareness (see Phase 3 in PRD).

