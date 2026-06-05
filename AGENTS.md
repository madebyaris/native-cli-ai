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

No pre-commit hooks configured. CI runs on push to all branches and PRs to `main`/`dev`.

## Workspace Structure

5 crates, edition 2024, version 0.3.0:

```
common  ← leaf crate, no internal deps. Shared types: config, events, messages, tool schemas.
core    ← depends on common only. Agent loop, Provider trait, tool registry, harness, skills, approvals.
runtime ← depends on common + core. Supervisor, IPC, session persistence, worktrees, PTY.
cli     ← depends on all above. Binary entrypoint, TUI, REPL, stream rendering.
autoresearch ← depends on common. Metric-driven research helpers.
```

Dependency direction is strictly `cli → runtime → core → common`. Never reverse this.

## Provider Architecture

All LLM calls go through the `Provider` trait in `core::provider`. Modules:
`minimax` (default), `anthropic`, `openai`, `deepseek`, `openrouter`, `zhipuai`.

- `openai_compat` and `anthropic_compat` are shared stream parsers for OpenAI-format and Anthropic-format SSE.
- `prepare_messages_for_request()` is the hook for provider-specific message rewriting (e.g. DeepSeek strips `reasoning_content`).
- Never hard-code a model name. Always read from config (`common::config::NcaConfig`).
- Empty completions must fail loudly (never silently succeed).

## Key Conventions

- **Rust-native only.** No JavaScript, Node.js, Electron, Tauri, or webview frameworks in any crate.
- **All I/O is async** via tokio. Never call blocking I/O on the async runtime; use `spawn_blocking` if unavoidable.
- **Error handling:** library crates use `thiserror`. Application code (`cli`) may use `anyhow`. Never `.unwrap()` in library code.
- **Visibility:** default to `pub(crate)`, promote to `pub` only when another crate needs it. All public types and traits must have doc comments.
- **Tests:** inline `#[cfg(test)] mod tests` in source files. Integration tests in `crates/cli/tests/`. Use `tempfile` for filesystem tests. Never depend on network access in tests—mock the `Provider` trait.
- **IPC:** newline-delimited JSON over Unix domain sockets. `AgentEvent` enum is the shared event bus.
- **Sessions:** `<workspace>/.nca/sessions/<id>.json` (state) + `<id>.events.jsonl` (event log). IPC socket at `$XDG_RUNTIME_DIR/nca/` (fallback `/tmp/nca/`).
- **Config resolution:** compiled defaults → `~/.nca/config.toml` → `<workspace>/.nca/config.local.toml` → env vars → CLI flags.
- **Conventional Commits** for commit messages (type(scope): description).

## Context & Cost Guardrails

The system has several size guards to prevent context window overflow:

- Tool output truncated at 32KB (head+tail strategy) in `agent.rs::truncate_tool_output`.
- `list_directory` capped at 1000 entries.
- Skill descriptions clipped to 120 chars in system prompt index.
- Skills index capped at 4000 chars in `harness.rs`.
- `reasoning_content` from DeepSeek is stripped before re-upload (response-only signal, ~500 tokens saved per turn).
- Cost tracker includes cache token accounting (cache_read priced at 1/50 of normal input).

## Code Search Tools

Two search tools with different semantics:

- `search_code` — ripgrep text search. Fast, regex-capable, handles any file type.
- `ast_grep_search` — AST-aware structural search. Matches syntax tree patterns using meta-variables (`$VAR`, `$$$`). Supports 25 languages. Use when pattern matching needs to respect code structure (e.g. `def $FUNC($$$):`, `console.log($MSG)`).
- `ast_grep_replace` — AST-aware structural replace. Dry-run by default (`apply=false`); set `apply=true` to write. Uses same pattern syntax as search.

Both `ast_grep_*` tools shell out to the `ast-grep` CLI (must be installed on PATH).

## MCP Tools

MCP servers are loaded via `rmcp` crate (v1.7). `load_mcp_tools()` is async—callers must await. MCP tool results go through the same `truncate_tool_output` guardrail as built-in tools.

## System Prompt Layering

Built in `core::harness::build_system_prompt`:
1. Built-in harness prompt
2. Permission-mode guidance
3. `AGENTS.md` (full file as instructions)
4. `.ncarc` (committed project instructions)
5. `.nca/instructions.md` (local instructions)
6. Discovered skills summary
7. Orchestration context (`NCA_ORCH_*` env vars)

## Release & Distribution

- Release targets: `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`.
- Binary: single `nca` executable.
- Release profile: `opt-level = 3`, thin LTO, `codegen-units = 1`, stripped, abort panic.
- Linux CI requires `libssl-dev` and `pkg-config` system packages.
