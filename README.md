# nca - Native CLI AI

A Rust-native AI coding assistant for the terminal. No JavaScript, no webview, single binary.

## Toolchain

This workspace uses Rust edition 2024. Use a recent toolchain (Rust/Cargo that supports edition 2024) before building.

## Quick Start

```bash
# Build the workspace
cargo build --workspace

# Configure MiniMax
export MINIMAX_API_KEY="your-api-key"

# Run the CLI
cargo run -p nca-cli

# Run with explicit interactive profile
cargo run -p nca-cli -- --run

# Run a one-shot MiniMax prompt
cargo run -p nca-cli -- --prompt "Explain this repository"

# Preferred command form
cargo run -p nca-cli -- run --prompt "Build a login page" --stream human

# Override model in run/spawn/resume surfaces
cargo run -p nca-cli -- run --prompt "Review code" --model MiniMax-M2.5
cargo run -p nca-cli -- spawn --prompt "Implement tests" --model MiniMax-M2.5
cargo run -p nca-cli -- resume <session_id> --model MiniMax-M2.5

# Run in safe mode (read/search/list only)
cargo run -p nca-cli -- --safe

# Inspect only, no edits or shell execution
cargo run -p nca-cli -- run --permission-mode plan --prompt "Review this repo"

# Spawn a background session
cargo run -p nca-cli -- spawn --prompt "Inspect the repo and draft a plan"

# List and resume sessions
cargo run -p nca-cli -- sessions
cargo run -p nca-cli -- resume <session_id>
cargo run -p nca-cli -- logs <session_id>
cargo run -p nca-cli -- status <session_id>
cargo run -p nca-cli -- attach <session_id>
cargo run -p nca-cli -- cancel <session_id>

# Run the desktop monitor (Phase 2)
cargo run -p nca-monitor
```

MiniMax is the default provider path. The CLI loads config from `~/.nca/config.toml`,
`.nca/config.local.toml`, and environment variables such as `MINIMAX_API_KEY`,
`MINIMAX_BASE_URL`, and `MINIMAX_MODEL`.

## Harness (System Prompt Layers)

nca builds a layered system prompt in this order:

1. Built-in base harness (always-on by default)
2. Project instructions from `.ncarc`
3. Local instructions from `.nca/instructions.md`

You can commit `.ncarc` for team conventions and keep `.nca/instructions.md` local.

## Tools

Current tool-running path supports:

- `read_file`
- `search_code` (ripgrep-backed)
- `list_directory`
- `write_file`
- `create_directory`
- `git_status`
- `git_diff`
- `query_symbols` (fast local code-intel)
- `web_search`
- `fetch_url`
- `apply_patch`
- `edit_file`
- `rename_path`
- `move_path`
- `copy_path`
- `delete_path`
- `run_validation`
- `execute_bash` (runtime-backed command execution; denied in `--safe`)

## Modes

- Interactive REPL
- One-shot `run`
- Background `spawn`
- Session `resume`
- Event `logs`
- Live `attach`
- Per-session `status`
- `cancel` for spawned work
- Stream modes: `off`, `human`, `ndjson`
- Permission modes: `default`, `plan`, `accept-edits`, `dont-ask`, `bypass-permissions`

## Claude Code Parity Roadmap

Follow-up parity work is planned for:

- slash-command style task presets
- custom subagents / agent profiles
- MCP integration
- multi-directory context similar to `--add-dir`
- durable memory and session summaries

## Project Structure

| Crate | Description |
|-------|-------------|
| `crates/common` | Shared types, config, events |
| `crates/core` | Agent loop, provider abstraction, tools |
| `crates/runtime` | PTY, process sandbox, IPC, sessions |
| `crates/cli` | Terminal UI and interactive REPL |
| `crates/monitor` | Native egui desktop monitor |

## Documentation

- [Product Requirements](docs/prd.md)
- [Tech Stack](docs/tech-stack.md)
- [Architecture](docs/architecture.md)

## License

MIT
