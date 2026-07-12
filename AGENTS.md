## Learned User Preferences

- Always use Rust-native solutions; no JavaScript, Node.js, or Electron involvement
- MiniMax M2.5 is the primary LLM provider; prioritize MiniMax integration over other providers
- **CLI (`nca`) is the product surface**—terminal UX, JSON/NDJSON streams, and Unix-socket IPC
- Empty provider completions must fail loudly, never silently succeed
- Use plan-first workflow: create a plan document, then implement from it
- Install CLI via `cargo build --release` then `cp target/release/nca /usr/local/bin/`
- Keep each crate focused: `common` for shared types, `core` for agent logic, `runtime` for session lifecycle, `cli` for terminal UX
- User tests CLI in separate workspace directories (e.g. `test-makan`, `for-test`)
- Prefer efficient algorithms and fast execution; the CLI is intended as a spawner for heavy tasks
- Sub-agents should use isolated worktrees; parent/child session lineage should stay visible in session metadata and events

## Learned Workspace Facts

- Rust workspace with **4 crates**: `nca-common`, `nca-core`, `nca-runtime`, `nca-cli`
- IPC between CLI and runtime uses Unix domain sockets with newline-delimited JSON
- Product home at `$NCA_HOME`, `$XDG_DATA_HOME/ncacli`, or `~/.local/share/ncacli/`; sessions/memory/last_session/cli-index under `workspaces/<workspace-id>/`
- Sessions persisted as `<id>.json` (state) + `<id>.events.jsonl` (event log) in `~/.local/share/ncacli/workspaces/<id>/sessions/`
- MiniMax provider endpoint: `https://api.minimaxi.chat/v1/text/chatcompletion_v2`
- Global config at `~/.local/share/ncacli/config.toml` (legacy `~/.nca/config.toml` read fallback)
- Git worktrees for isolated agent runs stored at `<repo>/.nca/worktrees/<session-id>`
- **Single shipped app binary:** `nca` (CLI)
- Tokio async runtime; `async-trait` for tool executor and approval handler interfaces
- Session lineage: parent/child session IDs, inherited summary, spawn reason tracked in `SessionMeta`
- `AgentEvent` enum is the shared event bus for CLI rendering, IPC broadcast, and disk persistence
- Runtime socket dir defaults to `$XDG_RUNTIME_DIR/nca/` or `/tmp/nca/`
- Dynamic harness: supervisor builds `HarnessSnapshot` and refreshes the system prompt each turn
