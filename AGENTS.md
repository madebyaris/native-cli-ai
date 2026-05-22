## Learned User Preferences

- Always use Rust-native solutions; no JavaScript, Node.js, or Electron involvement
- MiniMax M2.5 is the primary LLM provider; prioritize MiniMax integration over other providers
- **CLI (`nca`) is the product surface**—terminal UX, JSON/NDJSON streams, and Unix-socket IPC
- Empty provider completions must fail loudly, never silently succeed
- Stream/parse failures must not masquerade as model text — propagate typed errors to the user
- Use plan-first workflow: create a plan document, then implement from it
- Install CLI via `cargo build --release` then `cp target/release/nca /usr/local/bin/`
- Keep each crate focused: `common` for shared types, `core` for agent logic, `runtime` for session lifecycle, `cli` for terminal UX
- User tests CLI in separate workspace directories (e.g. `test-makan`, `for-test`)
- Prefer efficient algorithms and fast execution; the CLI is intended as a spawner for heavy tasks
- Sub-agents should use isolated worktrees; parent/child session lineage should stay visible in session metadata and events

## Learned Workspace Facts

- Rust workspace with **5 crates**: `nca-common`, `nca-core`, `nca-runtime`, `nca-tui`, `nca-cli`
- IPC between CLI and runtime uses Unix domain sockets with newline-delimited JSON
- Sessions persisted as `<id>.json` (state) + `<id>.events.jsonl` (event log) in `<workspace>/.nca/sessions/`
- Session and event envelopes include `schema_version` (currently `1`) for forward-compatible loading
- MiniMax provider endpoint: `https://api.minimaxi.chat/v1/text/chatcompletion_v2`
- Provider request bodies for MiniMax/OpenAI/Anthropic are snapshot-tested in `crates/core/tests/provider_request_bodies.rs` (`INSTA_UPDATE=never` in CI)
- Global config at `~/.nca/config.toml`
- Git worktrees for isolated agent runs stored at `<repo>/.nca/worktrees/<session-id>`
- **Single shipped app binary:** `nca` (CLI)
- Tokio async runtime; `async-trait` for tool executor and approval handler interfaces
- Session lineage: parent/child session IDs, inherited summary, spawn reason tracked in `SessionMeta`
- `AgentEvent` enum is the shared event bus for CLI rendering, IPC broadcast, and disk persistence
- Runtime socket dir defaults to `$XDG_RUNTIME_DIR/nca/` or `/tmp/nca/`
- Cooperative cancel: IPC `AgentCommand::Cancel` → `CancellationToken` in the agent loop; session state is persisted on finish
- Bash tools run in a real PTY via `portable-pty`, not piped `std::process::Command`

## Test discipline

- **Bug fixes require a regression test** that would have caught the bug (see `.cursor/rules/test-discipline.mdc`)
- Run the full gate before marking work done: `cargo nextest run --workspace --features semantic-index`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check --all`
- Never depend on network access in unit tests — mock the `Provider` trait
- Empty provider completions: assert `AgentEvent::Error`, never silent success
- Stream/parse failures: assert typed errors propagate, never model text
