## Build & Verify Commands

```bash
# Full CI pipeline (matches .github/workflows/ci.yml order)
cargo fmt --check --all
cargo clippy --workspace -- -D warnings
cargo build --workspace
cargo test --workspace

# Single crate
cargo test --package nca-core
cargo test --package nca-common

# Single test
cargo test --package nca-core -- test_name

# Install after build
cargo build --release && cp target/release/nca /usr/local/bin/

# Dev run (debug)
cargo run -p nca-cli

# Check formatting (non-destructive)
cargo fmt -- --check

# Lint with clippy
cargo clippy --workspace -- -D warnings
```

No pre-commit hooks are installed (despite `cargo-husky` appearing in cli dev-dependencies). CI runs on push to all branches and PRs to all branches.

No `rust-toolchain.toml` or `rustfmt.toml` — CI pins stable channel via `dtolnay/rust-toolchain@stable`; formatting and linting use Rust defaults.

## Workspace Structure

5 crates, edition 2024, version 0.3.0:

```
common  ← leaf crate, no internal deps. Shared types: config, events, messages, tool schemas.
core    ← depends on common only. Agent loop, Provider trait, tool registry, harness, skills, approvals.
runtime ← depends on common + core. Supervisor, IPC, session persistence, worktrees, PTY.
cli     ← depends on all above. Binary entrypoint, TUI, REPL, stream rendering.
autoresearch ← depends on common only (parallel to core). Metric-driven research helpers.
```

Dependency direction is strictly `cli → runtime → core → common` and `cli → autoresearch → common`. Never reverse this. `autoresearch` and `core` are siblings — neither depends on the other.

Key UI dependencies in `cli`: `ratatui 0.30` + `crossterm 0.29` (TUI), `reedline 0.38` (REPL), `syntect 5` (syntax highlighting), `pulldown-cmark 0.12` (markdown rendering), `arboard 3` (clipboard).

## Provider Architecture

All LLM calls go through the `Provider` trait in `core::provider`. Provider modules:
`minimax` (default), `minimax_vlm` (vision-language model), `anthropic`, `openai`, `deepseek`, `openrouter`, `zhipuai`.

Infrastructure modules in the same directory:
- `factory.rs` — provider instantiation from config.
- `openai_compat` / `anthropic_compat` — shared SSE stream parsers for their respective formats.
- `test_support.rs` / `validate.rs` — test harness helpers and provider config validation.

- `core` depends on `genai 0.5` (multi-provider LLM library) and `rmcp 1.7` (MCP client).
- `prepare_messages_for_request()` is the hook for provider-specific message rewriting (e.g. DeepSeek strips `reasoning_content`).
- Never hard-code a model name. Always read from config (`common::config::NcaConfig`).
- Empty completions must fail loudly (never silently succeed).

## Key Conventions

- **Rust-native only.** No JavaScript, Node.js, Electron, Tauri, or webview frameworks in any crate.
- **All I/O is async** via tokio. Never call blocking I/O on the async runtime; use `spawn_blocking` if unavoidable.
- **Error handling:** library crates use `thiserror`. Application code (`cli`) may use `anyhow`. Never `.unwrap()` in library code. Narrow exception: `.expect()` is allowed only in application startup code for reading required config (e.g. `main.rs`).
- **Visibility:** default to `pub(crate)`, promote to `pub` only when another crate needs it. All public types and traits must have doc comments.
- **Tests:** inline `#[cfg(test)] mod tests` in source files. Integration tests in `crates/cli/tests/`. Use `tempfile` for filesystem tests. Never depend on network access in tests—mock the `Provider` trait. `core` has a `tiny_http` dev-dependency for mock HTTP servers in tests. `cli` uses `insta` for TUI snapshot tests.
- **Channels:** prefer `tokio::sync::mpsc`. Use `tokio::sync::broadcast` only when multiple independent consumers need the same stream.
- **Tool execution:** shell commands must go through `runtime::pty` (PTY with timeout). Never use bare `std::process::Command` for user-visible execution. File write tools must canonicalize paths and verify they are within the workspace root.
- **Tool-use streaming:** incoming tool-use blocks are buffered until the closing tag is received, then executed as a batch (not streamed incrementally).
- **IPC:** newline-delimited JSON over Unix domain sockets. `AgentEvent` enum is the shared event bus.
- **Sessions:** `<workspace>/.nca/sessions/<id>.json` (state) + `<id>.events.jsonl` (event log). IPC socket at `$XDG_RUNTIME_DIR/nca/` (fallback `/tmp/nca/`).
- **Config resolution:** compiled defaults → `~/.nca/config.toml` → `<workspace>/.nca/config.local.toml` → env vars → CLI flags.
- **Conventional Commits** for commit messages (type(scope): description).
- **Do not edit `for-test/`** — it is gitignored and used for transient test artifacts.
- **Doc sync (same commit):** adding/removing deps → update `docs/tech-stack.md`; changing crate boundaries → update `docs/architecture.md`; changing MVP scope → update `docs/prd.md`.

## Context & Cost Guardrails

The system has several size guards to prevent context window overflow:

- Tool output truncated at 32KB (head+tail strategy) in `agent.rs::truncate_tool_output`.
- `list_directory` capped at 1000 entries.
- Skill descriptions clipped to 120 chars in system prompt index.
- Skills index capped at 4000 chars in `harness.rs`.
- `reasoning_content` from DeepSeek is stripped before re-upload (response-only signal, ~500 tokens saved per turn).
- Cost tracker includes cache token accounting (cache_read priced at 1/50 of normal input).

## TUI & IPC Performance

- Ratatui rendering must use `is_dirty()` guards to skip frames when nothing changed — without this, idle CPU stays at 7%+ instead of target <1%.
- IPC channels must be bounded (buffer=100) to prevent unbounded message accumulation.
- Use `Vec::with_capacity` when size is known at allocation time.
- Bash PTY does not inherit environment variables; only an explicit whitelist passes through.

## System Dependencies

Linux builds require `libssl-dev`, `pkg-config`, and `ripgrep`. macOS builds may need the Homebrew equivalents.

## MCP Tools

MCP servers are loaded via `rmcp` crate (v1.7, features: `client`, `transport-async-rw`). `load_mcp_tools()` is async—callers must await. MCP tool results go through the same `truncate_tool_output` guardrail as built-in tools.

## System Prompt Layering

Built in `core::harness::build_system_prompt`:
1. Built-in harness prompt
2. Permission-mode guidance
3. Global Instructions (`~/.nca/AGENTS.md`, user-level)
4. `AGENTS.md` (workspace repo-level instructions)
5. `.ncarc` (committed project instructions, if present)
6. `.nca/instructions.md` (local instructions)
7. Discovered skills summary
8. Orchestration context (`NCA_ORCH_*` env vars)

## Key File Map

| Path | Purpose |
|---|---|
| `crates/core/src/agent.rs` | Main agent loop, `truncate_tool_output`, token estimation |
| `crates/core/src/harness.rs` | System prompt builder, skills index assembly |
| `crates/core/src/provider/` | All provider implementations, factory, stream parsers |
| `crates/core/src/tools/` | Built-in tool definitions (file ops, search, shell, etc.) |
| `crates/core/src/skills.rs` | Skill discovery and resolution |
| `crates/runtime/src/` | Supervisor, IPC server, session persistence, PTY, worktrees |
| `crates/cli/src/main.rs` | Binary entrypoint |
| `crates/cli/src/tui/` | Full-screen TUI implementation |
| `crates/cli/src/repl.rs` | Line-oriented REPL (reedline-based) |
| `crates/common/src/config.rs` | Config schema and resolution chain |

## Release & Distribution

- Release targets: `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`.
- Binary: single `nca` executable.
- Release profile: `opt-level = 3`, thin LTO, `codegen-units = 1`, stripped, abort panic.
- Linux CI requires `libssl-dev`, `pkg-config`, and `ripgrep` system packages.
- Release triggers: PR merge to `main` or tag push `v*`. GitHub Release created only on tag pushes.
