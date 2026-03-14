# Architecture

This document defines the crate boundaries, data flows, IPC model, security model, and session lifecycle for nca.

---

## Workspace Layout

```
native-cli-ai/
├── Cargo.toml              # workspace root
├── crates/
│   ├── common/             # shared types, config, event schema
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs       # NcaConfig, ModelConfig, PermissionConfig
│   │       ├── event.rs        # AgentEvent enum (tool calls, responses, approvals)
│   │       ├── message.rs      # Conversation message types
│   │       ├── tool.rs         # ToolDefinition, ToolCall, ToolResult
│   │       └── session.rs      # SessionMeta, SessionState
│   │
│   ├── core/               # agent loop, provider abstraction, tool protocol
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── agent.rs        # AgentLoop: drives conversation + tool execution
│   │       ├── provider.rs     # Provider trait + provider modules
│   │       ├── provider/
│   │       │   ├── factory.rs  # Selects the configured provider adapter
│   │       │   └── minimax.rs  # MiniMax native API adapter (default)
│   │       ├── code_intel.rs   # Fast-local facade + future language-server mode
│   │       ├── harness.rs      # Layered system prompt builder
│   │       ├── tools/
│   │       │   ├── mod.rs      # ToolRegistry
│   │       │   ├── filesystem.rs
│   │       │   ├── bash.rs
│   │       │   ├── search.rs
│   │       │   ├── web_search.rs
│   │       │   ├── fetch_url.rs
│   │       │   ├── apply_patch.rs
│   │       │   ├── edit_file.rs
│   │       │   └── ... other path/validation tools
│   │       ├── approval.rs     # ApprovalPolicy: allowed / ask / denied
│   │       └── cost.rs         # Token counting and cost estimation
│   │
│   ├── runtime/            # PTY, process management, IPC, tmux
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── pty.rs          # PtyManager: spawn, read, write, resize
│   │       ├── process.rs      # SandboxedProcess: workspace-confined execution
│   │       ├── ipc.rs          # IpcServer / IpcClient over Unix socket
│   │       ├── tmux.rs         # TmuxAdapter (Phase 3)
│   │       └── session_store.rs # Persist / load sessions to disk
│   │
│   ├── cli/                # TUI shell, run/spawn control plane, streaming
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs         # Entrypoint, clap args, launch
│   │       ├── app.rs          # App state machine
│   │       ├── repl.rs         # REPL loop: input -> agent -> render
│   │       ├── runner.rs       # Session runtime builder / persistence glue
│   │       ├── stream.rs       # Human and NDJSON event rendering
│   │       ├── render/
│   │       │   ├── mod.rs
│   │       │   ├── markdown.rs # Markdown-to-terminal rendering
│   │       │   ├── diff.rs     # Colored diff display
│   │       │   └── status.rs   # Cost bar, model info, mode indicator
│   │       └── prompt.rs       # reedline-based input with completions
│   │
│   └── monitor/            # egui desktop app (Phase 2)
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs         # eframe launch
│           ├── app.rs          # MonitorApp state
│           ├── panels/
│           │   ├── sessions.rs # Session list and selector
│           │   ├── terminal.rs # Live terminal mirror
│           │   ├── tools.rs    # Tool call history
│           │   ├── diff.rs     # Diff viewer
│           │   └── stats.rs    # Token usage, cost, model info
│           └── ipc_client.rs   # Connects to runtime IPC
│
├── docs/
│   ├── prd.md
│   ├── tech-stack.md
│   └── architecture.md     # this file
│
└── .cursor/
    └── rules/              # Cursor rules for AI-assisted development
```

---

## Crate Dependency Graph

```mermaid
flowchart TD
  Common[common]
  Core[core]
  Runtime[runtime]
  Cli[cli]
  Monitor[monitor]

  Core --> Common
  Runtime --> Common
  Runtime --> Core
  Cli --> Common
  Cli --> Core
  Cli --> Runtime
  Monitor --> Common
  Monitor --> Runtime
```

Key constraint: `monitor` depends on `common` and `runtime` (for `IpcClient`, `WorktreeManager`, and `WorkspaceRegistry`). It never imports `core` or `cli`. The CLI delegates session lifecycle to the runtime `Supervisor`.

## Desktop-First Architecture

The desktop app (`nca-monitor`) is the primary user interface. The CLI remains as a secondary/debug interface.

```mermaid
flowchart LR
  DesktopApp[Desktop App] --> WorkspaceRegistry
  DesktopApp --> CompanyProjectUi[CompanyProjectTodoAgent UI]
  DesktopApp --> SessionHub[Session Index]
  DesktopApp --> ReviewWorkbench
  WorkspaceRegistry --> LocalMetadata["~/.nca/workspaces.json"]
  CompanyProjectUi --> OrchestratorDb["~/.nca/orchestrator.db"]
  SessionHub --> RuntimeService[Supervisor]
  RuntimeService --> AgentRuns[AgentLoop]
  RuntimeService --> EventStore[EventEnvelope logs]
  RuntimeService --> OrchestratorStore[SQLite orchestration store]
  RuntimeService --> WorktreeManager
  ReviewWorkbench --> GitInspector[git diff/status]
  ReviewWorkbench --> MergeActions[git merge/worktree]
  CliSecondary[CLI] --> RuntimeService
```

### Key modules

- **`runtime::supervisor`**: Reusable session lifecycle manager. Both CLI and desktop use this.
- **`runtime::workspace_registry`**: Persisted workspace index at `~/.nca/workspaces.json`.
- **`runtime::orchestrator_store`**: SQLite-backed local store for companies, projects, todos, agents, mode preference, and run links.
- **`runtime::worktree`**: Isolated git worktree creation, cleanup, and merge per agent run.
- **`runtime::bash_tool`**: PTY-backed bash execution, registered by the supervisor.
- **`monitor::workspaces`**: Desktop workspace view-model and navigation.
- **`monitor::panels::review`**: Review workbench with changed files, diff viewer, and merge actions.
- **`monitor::panels::files`**: Changed files browser.
- **`monitor::panels::diff`**: Unified diff viewer with syntax coloring.

---

## Agent Loop

The central execution model is a **tool-use loop** driven by `core::agent::AgentLoop`.

The current default provider path is `MiniMaxProvider`, selected by `core::provider::factory`
from `common::config::NcaConfig`. The CLI resolves configuration from defaults, `~/.nca/config.toml`,
`.nca/config.local.toml`, and environment variables such as `MINIMAX_API_KEY`.

The system prompt is layered by `core::harness::build_system_prompt`:

1. built-in harness prompt
2. project instructions from `.ncarc`
3. local instructions from `.nca/instructions.md`
4. optional orchestration metadata from `NCA_ORCH_*`

```mermaid
sequenceDiagram
  participant User
  participant Repl as cli::repl
  participant Agent as core::AgentLoop
  participant Provider as core::Provider
  participant Tools as core::ToolRegistry
  participant Runtime as runtime

  User->>Repl: input message
  Repl->>Agent: send_message(text)
  loop until no more tool_use blocks
    Agent->>Provider: chat(messages)
    Provider-->>Agent: stream response
    Agent->>Agent: parse tool_use blocks
    Agent->>Tools: check approval policy
    alt approved
      Tools->>Runtime: execute tool
      Runtime-->>Tools: tool result
    else ask
      Agent->>User: request approval via CLI handler
      User-->>Agent: approve or deny
    else denied
      Agent->>Agent: inject denial message
    end
    Agent->>Agent: append tool results to messages
  end
  Agent-->>Repl: final response
  Repl->>User: render markdown
```

### Streaming

Provider responses are streamed token-by-token via `tokio::sync::mpsc` using MiniMax SSE. The CLI can render:

- human-readable live progress
- NDJSON `EventEnvelope` stream mode
- no stream, with only final output

Tool-use blocks are collected, executed by the registry, and replayed to MiniMax as `tool` messages until a final assistant response is produced.

---

## IPC and Event Bus

The runtime exposes a Unix domain socket at `$XDG_RUNTIME_DIR/nca/<session-id>.sock` (or `/tmp/nca/` as fallback). Running sessions persist status, PID, and socket path in session metadata.

### Protocol

- **Transport**: Unix stream socket, newline-delimited JSON.
- **Direction**: The runtime is the server. CLI and monitor are clients.
- **Messages**: Every `AgentEvent` from `common::event` is wrapped in `EventEnvelope` and serialized to all connected clients. Persisted logs and live IPC use the same machine-readable shape.

```mermaid
flowchart LR
  CliProcess[cli] -->|"connect"| Socket["Unix socket"]
  Socket --> RuntimeServer[runtime::IpcServer]
  MonitorProcess[monitor] -->|"connect"| Socket
  RuntimeServer -->|"broadcast events"| CliProcess
  RuntimeServer -->|"broadcast events"| MonitorProcess
  CliProcess -->|"send commands"| RuntimeServer
  MonitorProcess -->|"send commands"| RuntimeServer
```

### Event Schema (common::event)

```rust
pub enum AgentEvent {
    SessionStarted { session_id: String, workspace: PathBuf, model: String },
    MessageReceived { role: Role, content: String },
    TokensStreamed { delta: String },
    ToolCallStarted { call_id: String, tool: String, input: serde_json::Value },
    ToolCallCompleted { call_id: String, output: ToolResult },
    ApprovalRequested { call_id: String, tool: String, description: String },
    ApprovalResolved { call_id: String, approved: bool },
    CostUpdated { input_tokens: u64, output_tokens: u64, estimated_cost_usd: f64 },
    Checkpoint { phase: String, detail: String, turn: u32 },
    SessionEnded { reason: EndReason },
    Error { message: String },
    Response { response: AgentResponse },
    ChildSessionSpawned { parent_session_id: String, child_session_id: String, task: String, workspace: PathBuf, branch: Option<String> },
    ChildSessionCompleted { parent_session_id: String, child_session_id: String, status: String },
}
```

### Command Schema

```rust
pub enum AgentCommand {
    SendMessage { content: String },
    ApproveToolCall { call_id: String },
    DenyToolCall { call_id: String },
    Cancel,
    Shutdown,
}
```

---

## PTY and Process Execution

### Sandboxed Bash

`runtime::pty::PtyManager` wraps command execution to:

1. Spawn a shell in a PTY confined to the workspace root (via `chdir`).
2. Capture stdout/stderr as structured output.
3. Enforce a timeout (default 30s, configurable).
4. Kill the process on cancellation or timeout.

### Permission Check Flow

```
User request -> Agent proposes bash tool call
  -> core::approval checks command against config tiers:
     allowed_commands: ["cargo", "npm", "go", "ls", "cat", "grep", "git status", ...]
     denied_commands:  ["rm", "sudo", "chmod", "kill", "shutdown", ...]
     ask_commands:     [everything else]
  -> If "ask": prompt through the active approval handler
  -> If approved: runtime-backed bash executor runs command in workspace
  -> Result streamed back as ToolResult
```

## Session Commands

The CLI now exposes multiple session surfaces on top of the same engine:

- `run` for explicit one-shot execution
- `--run` for Claude-style interactive run mode
- `serve` for long-lived IPC-controlled sessions (used by the desktop app)
- `spawn` for background execution
- `sessions` for saved-session listing
- `resume` for continuing a saved session
- `logs` for replaying structured event output
- `attach` for live event replay over IPC
- `status` for session metadata
- `cancel` for stopping a running session

## Permission Modes

The CLI supports explicit permission handling modes:

- `default` for read/web tools auto-allowed, edits and commands ask
- `plan` for analysis/research only
- `accept-edits` for auto-accepted file edits with command caution
- `dont-ask` for readonly-only automatic execution
- `bypass-permissions` for fully trusted environments

---

## Tmux Adapter (Phase 3)

`runtime::tmux::TmuxAdapter` wraps `tmux_interface` behind a trait:

```rust
#[async_trait]
pub trait MultiplexerAdapter: Send + Sync {
    async fn create_session(&self, name: &str, cwd: &Path) -> Result<SessionHandle>;
    async fn attach(&self, handle: &SessionHandle) -> Result<()>;
    async fn detach(&self, handle: &SessionHandle) -> Result<()>;
    async fn send_keys(&self, handle: &SessionHandle, keys: &str) -> Result<()>;
    async fn capture_pane(&self, handle: &SessionHandle) -> Result<String>;
    async fn kill_session(&self, handle: &SessionHandle) -> Result<()>;
}
```

This trait allows swapping tmux for zellij or a built-in multiplexer later.

---

## Session Model

### Persistence

Sessions are stored as JSON files in `.nca/sessions/<session-id>.json`:

```json
{
  "id": "a1b2c3",
  "created_at": "2026-03-11T10:00:00Z",
  "updated_at": "2026-03-11T10:15:00Z",
  "workspace": "/home/user/project",
  "model": "claude-sonnet-4-5",
  "messages": [ ... ],
  "total_input_tokens": 12500,
  "total_output_tokens": 8300,
  "estimated_cost_usd": 0.042
}
```

The desktop orchestration layer uses a hybrid persistence model:

- `~/.nca/orchestrator.db` stores companies, projects, todos, agent profiles, run links, and desktop mode preference.
- `<workspace>/.nca/sessions/*.json` stores session snapshots and conversation state.
- `<workspace>/.nca/sessions/*.events.jsonl` stores append-only event streams for replay and live attach.

### Lifecycle

```mermaid
stateDiagram-v2
  [*] --> Idle: nca launched
  Idle --> Active: user sends message
  Active --> WaitingApproval: tool needs approval
  WaitingApproval --> Active: approved
  WaitingApproval --> Active: denied
  Active --> Idle: response complete
  Idle --> Persisted: user exits
  Persisted --> Active: nca --resume
  Active --> Cancelled: ESC / Ctrl+C
  Cancelled --> Idle: cleanup done
```

---

## Security Model

### Workspace Sandbox

```
workspace_root/
├── .nca/                    # nca data (sessions, config, instructions)
│   ├── config.local.toml    # gitignored, local overrides
│   ├── instructions.md      # personal instructions
│   └── sessions/
├── .ncarc                   # project-wide instructions (version controlled)
├── src/                     # project source -- full read/write access
└── ...
```

- **Inside workspace**: Read and write allowed by default.
- **Outside workspace**: Read only if explicitly allowed in config. Write always denied.
- **Home directory config**: `~/.nca/config.toml` for global defaults.

### Threat Model

| Threat | Mitigation |
|--------|-----------|
| LLM instructs destructive command | Tiered approval system; destructive commands in deny list |
| LLM writes outside workspace | Path canonicalization + workspace root check before every write |
| LLM exfiltrates secrets via bash | Bash runs in PTY with no inherited env vars beyond explicit allowlist |
| Malicious MCP server | MCP server commands are not covered by workspace sandbox; documented as user responsibility |
| Session file tampering | Sessions are local-only; no remote sync in MVP |

---

## Config Resolution Order

Config values are resolved with later sources overriding earlier ones:

1. Compiled defaults
2. `~/.nca/config.toml` (global)
3. `.nca/config.local.toml` (workspace, gitignored)
4. Environment variables (`NCA_API_KEY`, `NCA_MODEL`, etc.)
5. CLI flags (`--model`, `--safe`, `--verbose`)

---

## Build and Distribution

- **Dev**: `cargo run -p nca-cli`
- **Release**: `cargo build --release` produces two binaries: `nca` (cli) and `nca-monitor` (egui app).
- **Install**: `cargo install --path crates/cli` and `cargo install --path crates/monitor`.
- **CI**: GitHub Actions with `cargo test --workspace`, `cargo clippy --workspace`, `cargo fmt --check`.
- **Cross-compile**: Target `x86_64-unknown-linux-musl` for static Linux binaries. macOS and Windows use default targets.
