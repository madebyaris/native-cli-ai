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
```

A pre-commit hook (auto-installed by `cargo-husky` on first `cargo test`) runs `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt -- --check` before every commit — so commits are slow and will fail if any of those fail. To bypass locally use `git commit --no-verify` (CI still enforces the same checks). CI runs on push to all branches and PRs to all branches.

No `rust-toolchain.toml` or `rustfmt.toml` — CI pins stable channel via `dtolnay/rust-toolchain@stable`; formatting and linting use Rust defaults.

## Workspace Structure

5 crates, edition 2024, version 0.3.0:

```
common  ← leaf crate, no internal deps. Shared types: config, events, messages, tool schemas, model capabilities.
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
- **Tool execution:** shell commands must go through `runtime::pty` (PTY with timeout). Never use bare `std::process::Command` for user-visible execution. File write tools must canonicalize paths and verify they are within the workspace root or any mounted extra path. External directories can be mounted at runtime via `/mount <path>`.
- **Tool-use streaming:** incoming tool-use blocks are buffered until the closing tag is received, then executed as a batch (not streamed incrementally).
- **IPC:** newline-delimited JSON over Unix domain sockets. `AgentEvent` enum is the shared event bus.
- **Sessions:** `<workspace>/.nca/sessions/<id>.json` (state) + `<id>.events.jsonl` (event log). IPC socket at `$XDG_RUNTIME_DIR/nca/` (fallback `/tmp/nca/`).
- **Config resolution:** compiled defaults → `$XDG_CONFIG_HOME/nca/config.toml` → `<workspace>/.nca/config.local.toml` → env vars → CLI flags.
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

## TUI Architecture

The TUI runs a **dual-write bridge** (`tui/bridge.rs`) that fans out each `AgentEvent` to:
1. `TuiSessionState` (legacy mutable state in `state.rs`) — still used by the `cmd_rx` loop in `repl.rs` for synchronous reads.
2. `TuiFeedbackMsg` channel → `NcaModel` (Elm architecture in `tui/elm/`) — the actual renderer.

The Elm TUI (`tui/elm/model.rs::NcaModel`) runs inside `spawn_blocking` on a dedicated thread with its own tick/update/view loop. It owns all rendering, input handling, and component state. All events flow through `TuiFeedbackChannel` → `TuiFeedbackMsg` channel → `NcaModel::apply_event`. The legacy `app.rs::run_blocking()` and `TuiSessionState` dual-write are no longer used.

**Question-answer bypass:** While `run_turn` blocks on `ask_question`'s oneshot channel, the main `cmd_rx` loop never receives `TuiCmd::Submit` or `QuestionAnswer`. Answers must flow through the `question_answer_tx` side channel (set up in `repl.rs`). The Elm composer routes Enter keypresses through this channel when `active_question` is set.

**Dead-code suppression:** The legacy `app.rs` has `#[allow(dead_code)]` on `TuiCmd` and `ApprovalAnswer` variants. `tui/elm/mod.rs` has `#![allow(dead_code, unused_imports)]`. Do not remove `#[allow]` annotations without verifying the item is truly dead. `TuiSessionState` in `state.rs` is retained only for tests and the legacy `replay_event_log_into_state` helper.

**Shared state between Elm and runtime:** Uses `Arc<StdMutex<...>>` (not tokio::Mutex) because Elm runs on a blocking thread. Key shared handles: `active_question_id`, `active_question_payload`, `staged_images`.

## TUI & IPC Performance

- Ratatui rendering must use `is_dirty()` guards to skip frames when nothing changed — without this, idle CPU stays at 7%+ instead of target <1%. The Elm architecture has a `redraw` guard and `BlockLineCache` for incremental rendering.
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
3. Global Instructions (`$XDG_CONFIG_HOME/nca/AGENTS.md`, user-level)
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
| `crates/core/src/tools/` | Built-in tool definitions (file ops, search, edit, validation, git, AST, etc.) |
| `crates/core/src/skills.rs` | Skill discovery, resolution, and metadata |
| `crates/core/src/approval.rs` | Approval policy, permission modes, tiered command rules |
| `crates/core/src/code_intel.rs` | Fast local code intelligence, Rust symbol lookup |
| `crates/core/src/cost.rs` | Token counting and cost estimation (with cache token accounting) |
| `crates/core/src/hooks.rs` | Hook runner, lifecycle event hooks (pre/post tool execution) |
| `crates/core/src/tool_pipeline.rs` | Tool execution pipeline (approval → hooks → execution → post-hooks) |
| `crates/core/src/workspace_fs.rs` | Workspace filesystem abstraction, path sandbox, and runtime mount support |
| `crates/core/src/skill_installer.rs` | Skill installation from registries |
| `crates/runtime/src/` | Supervisor, IPC server, session persistence, PTY, worktrees, context manager, subagent |
| `crates/runtime/src/supervisor.rs` | Session lifecycle supervisor |
| `crates/runtime/src/context_manager.rs` | Token tracking, auto-summarize, sliding window |
| `crates/runtime/src/subagent.rs` | Subagent spawning and management |
| `crates/runtime/src/worktree.rs` | Git worktree creation and cleanup |
| `crates/runtime/src/memory_store.rs` | Workspace memory persistence |
| `crates/cli/src/main.rs` | Binary entrypoint |
| `crates/cli/src/repl.rs` | Line-oriented REPL (reedline-based), Elm TUI wiring (`run_with_tui`) |
| `crates/cli/src/tui/bridge.rs` | Dual-write event fanout: state + Elm feedback channel |
| `crates/cli/src/tui/elm/model.rs` | `NcaModel` (Elm architecture), `SideEffectChannels`, tick/update/view loop |
| `crates/cli/src/tui/elm/run.rs` | `run_nca_model()` entrypoint, `NcaModelParams` |
| `crates/cli/src/tui/elm/msg.rs` | `Msg` enum (Elm messages including `QuestionSubmit`, `QuestionAnswer`) |
| `crates/cli/src/tui/elm/feedback.rs` | `TuiFeedbackMsg` enum, shared state handles |
| `crates/cli/src/tui/app.rs` | Legacy TUI (dead code with `#[allow(dead_code)]`), exports `TuiCmd`, `ApprovalAnswer`, theme |
| `crates/cli/src/tui/state.rs` | `TuiSessionState`, `DisplayBlock`, event application |
| `crates/common/src/config.rs` | Config schema and resolution chain |
| `crates/common/src/event.rs` | `AgentEvent` enum, event bus types |
| `crates/common/src/model_caps.rs` | Model capability detection (vision, context window, etc.) |
| `crates/common/src/session.rs` | Session metadata, `OrchestrationContext`, orchestration env contract |

## Release & Distribution

- Release targets: `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`.
- Binary: single `nca` executable.
- Release profile: `opt-level = 3`, thin LTO, `codegen-units = 1`, stripped, abort panic.
- Linux CI requires `libssl-dev`, `pkg-config`, and `ripgrep` system packages.
- Release triggers: PR merge to `main` or tag push `v*`. GitHub Release created only on tag pushes.
