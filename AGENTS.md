## Learned User Preferences

- Always use Rust-native solutions; no JavaScript, Node.js, or Electron involvement
- MiniMax M2.5 is the primary LLM provider; prioritize MiniMax integration over other providers
- Desktop app (`nca-monitor`) is the primary user experience; CLI (`nca`) is secondary/debug
- Conductor.build is the UX inspiration for the desktop monitor app
- Sub-agents should be spawned child sessions with their own worktrees, visible in the desktop dashboard
- Empty provider completions must fail loudly, never silently succeed
- Use plan-first workflow: create a plan document, then implement from it
- Install binaries via `cargo build --release` then `cp target/release/{nca,nca-monitor} /usr/local/bin/`
- Use egui (eframe) for the desktop monitor, not Tauri or Dioxus
- Keep each crate focused: `common` for shared types, `core` for agent logic, `runtime` for session lifecycle, `cli` for terminal UX, `monitor` for desktop UX
- User tests CLI in separate workspace directories (e.g. `test-makan`, `for-test`)
- Prefer efficient algorithms and fast execution; the CLI is intended as a spawner for heavy tasks

## Learned Workspace Facts

- Rust workspace with 5 crates: `nca-common`, `nca-core`, `nca-runtime`, `nca-cli`, `nca-monitor`
- `monitor` depends on `common` and `runtime` (for worktree); never imports `core` or `cli`
- IPC between CLI/monitor and runtime uses Unix domain sockets with newline-delimited JSON
- Sessions persisted as `<id>.json` (state) + `<id>.events.jsonl` (event log) in `<workspace>/.nca/sessions/`
- MiniMax provider endpoint: `https://api.minimaxi.chat/v1/text/chatcompletion_v2`
- Global config at `~/.nca/config.toml`; workspace registry at `~/.nca/workspaces.json`
- Git worktrees for isolated agent runs stored at `<repo>/.nca/worktrees/<session-id>`
- Two output binaries: `nca` (CLI) and `nca-monitor` (desktop egui app)
- Tokio async runtime; `async-trait` for tool executor and approval handler interfaces
- Session lineage: parent/child session IDs, inherited summary, spawn reason tracked in `SessionMeta`
- `AgentEvent` enum is the shared event bus for CLI rendering, IPC broadcast, and disk persistence
- Runtime socket dir defaults to `$XDG_RUNTIME_DIR/nca/` or `/tmp/nca/`
