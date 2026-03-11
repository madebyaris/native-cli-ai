# Product Requirements Document: Native CLI AI

## Product Vision

A native-first, Rust-powered AI coding assistant that runs entirely in the terminal with zero JavaScript dependencies. It provides an interactive agent loop for code generation, file editing, command execution, and project understanding -- comparable in capability to Claude Code and OpenAI Codex CLI, but built as a single compiled binary with sub-100ms startup, low memory footprint, and an optional native desktop monitor for visual session control.

The product name is **nca** (native-cli-ai) throughout this document.

---

## User Personas

### Primary: Power Developer

- Lives in the terminal (tmux, neovim, zsh).
- Wants an AI assistant that fits into their existing workflow, not a browser tab.
- Cares about startup speed, memory, and not pulling in Node/Python runtimes.
- Comfortable with config files, environment variables, and CLI flags.

### Secondary: Team Lead / Reviewer

- Wants to monitor multiple agent sessions running across worktrees.
- Needs a dashboard to see what the AI is doing, approve dangerous operations, and review diffs before they land.
- Uses the desktop monitor app alongside the terminal.

### Tertiary: CI / Automation User

- Runs nca in headless/one-shot mode inside scripts, pipelines, or cron jobs.
- Needs deterministic exit codes, structured output (JSON), and no interactive prompts.

---

## Core Workflows

### 1. Interactive REPL Session

User launches `nca` in a project directory. The agent enters a multi-turn conversation loop. The user describes tasks in natural language. The agent proposes tool calls (file reads, writes, shell commands, searches), streams results, and iterates until the task is done or the user exits.

### 2. One-Shot Command

`nca --prompt "add error handling to src/main.rs"` runs a single task through the same agent/tool loop used by REPL, prints the result, and exits with a status code. Suitable for scripting and CI.

### 2.1 Streamed Run Mode

`nca run --prompt "..." --stream human|ndjson|off` exposes the same engine with explicit stream control for human monitoring or machine-readable event consumption.

### 2.2 Spawner / Control Plane

The CLI also acts as a spawner:

- `nca spawn --prompt "..."` launches a background session
- `nca sessions` lists resumable sessions
- `nca resume <session-id>` continues a saved session
- `nca logs <session-id>` shows structured event output

### 3. Safe / Read-Only Mode

`nca --safe` restricts the agent to read-only tools: file reads, directory listings, code search, and web search. No writes, no shell execution. Useful for exploration and code review.

### 3.1 Layered Harness

System behavior is guided by a layered harness prompt:

1. Built-in nca system prompt
2. Project instructions from `.ncarc`
3. Local instructions from `.nca/instructions.md`

### 4. Session Resume

Sessions are persisted to disk. `nca --resume` picks up the last session with full conversation context.

### 5. Desktop Monitor

A separate `nca-monitor` binary (egui) connects to one or more running nca sessions over a local Unix socket. It displays live terminal output, tool call history, diffs, token usage, and approval prompts.

### 6. Tmux / Multiplexer Integration

`nca` can attach to or create tmux sessions, enabling long-running background agents that the user reconnects to later.

---

## MVP Scope (Phase 1)

### In Scope

- Interactive REPL with multi-turn conversation
- Session spawn/list/resume/logs workflow
- Provider abstraction supporting Anthropic Claude and OpenAI
- MiniMax-first provider path with direct API integration
- Tool loop: read file, write file, list directory, code search (ripgrep), bash execution
- Web research tools: `web_search`, `fetch_url`
- Rich edit tools: patch/edit/move/copy/delete/validation
- Git helpers and fast local symbol query support
- Layered harness loading from built-in + `.ncarc` + `.nca/instructions.md`
- Permission/approval system with explicit modes: `default`, `plan`, `accept-edits`, `dont-ask`, `bypass-permissions`
- Sandboxed file operations (workspace-only writes by default)
- Session persistence and resume
- Safe/read-only mode
- One-shot prompt mode with structured exit codes
- Config via TOML (`~/.nca/config.toml` and `.nca/config.local.toml`)
- Custom instructions (`.ncarc` project file, `.nca/instructions.md` personal file)
- Markdown-rendered responses with syntax highlighting in the terminal
- Token usage and cost tracking per session
- Colored diffs for file changes

### Out of Scope for MVP

- Desktop monitor app (Phase 2)
- MCP server/client integration (Phase 3)
- Tmux session management (Phase 3)
- Full language-server-backed LSP mode (phased after fast local code-intel)
- Image/vision input (Phase 4)
- Multi-agent orchestration (Phase 4)
- Plugin/extension system (Phase 4)
- Remote/SSH agent execution (Future)

---

## Non-Goals

- nca is not an IDE. It does not provide LSP, autocomplete, or inline suggestions.
- nca is not a chat UI. The monitor app is a control plane, not a conversation frontend.
- nca does not bundle or depend on Node.js, Python, or any non-Rust runtime.
- nca does not aim for feature parity with Claude Code on day one. It ships a tight core and expands.

---

## Constraints

- **Single binary**: The CLI ships as one statically-linked (where possible) Rust binary.
- **No JS/webview in the default path**: The CLI and core library must never require a JavaScript runtime. The monitor app uses egui, not a webview.
- **Workspace sandbox**: By default, file writes and shell commands are restricted to the current workspace root. Escaping requires explicit config.
- **Async-first**: All I/O (network, filesystem, PTY) goes through tokio. No blocking the main thread.
- **Offline-tolerant config**: Config, sessions, and custom instructions work without network access. Only LLM calls require connectivity.

---

## Security Model

### Tiered Permissions

| Tier | Behavior | Examples |
|------|----------|---------|
| Allowed | Auto-execute, no prompt | `read_file`, `list_directory`, `search_code` |
| Ask | Prompt user for approval | Unknown bash commands, writes outside workspace |
| Denied | Always blocked | `rm -rf /`, `sudo`, commands in deny list |

### Sandbox Rules

- File writes confined to workspace root unless explicitly allowed in config.
- Bash execution confined to workspace root.
- Environment variable injection blocked by default.
- Config supports glob-based allow/deny lists for both file paths and commands.

---

## Success Metrics

| Metric | Target |
|--------|--------|
| Cold startup time | < 100ms |
| Memory at idle | < 30MB RSS |
| Binary size (release, stripped) | < 15MB |
| Time to first token streamed | < 500ms after API response starts |
| Tool call round-trip (local file read) | < 10ms |
| Session resume load time | < 200ms |

---

## User Experience Principles

1. **Terminal-native**: Every interaction must work in a standard terminal. No mouse required.
2. **Predictable**: The agent always shows what it intends to do before doing it (for ask-tier operations).
3. **Interruptible**: ESC or Ctrl+C cleanly cancels any in-flight operation.
4. **Transparent**: Token costs, tool calls, and model responses are always visible.
5. **Fast**: Startup, tool execution, and rendering must feel instant.

---

## Naming and CLI Interface

```
nca                          # Start interactive REPL
nca --prompt "..."           # One-shot mode
nca run --prompt "..."       # Explicit run command with stream modes
nca spawn --prompt "..."     # Background session
nca sessions                 # List sessions
nca resume <session-id>      # Resume a saved session
nca logs <session-id>        # Show structured event log
nca attach <session-id>      # Attach to session output
nca status <session-id>      # Show session metadata
nca cancel <session-id>      # Cancel a running session
nca --safe                   # Read-only mode
nca --resume                 # Resume last session
nca --model MiniMax-M2.5     # Override model
nca --stream ndjson          # NDJSON event streaming
nca --permission-mode plan   # Analysis only, no edits or shell execution
nca --verbose                # Debug logging
nca --json                   # Structured JSON output (for CI)
nca-monitor                  # Launch desktop monitor (separate binary)
```

## Post-MVP Parity Roadmap

After the current parity batch, the next Claude Code-like features to add are:

- slash-command style task presets
- custom subagents / agent profiles
- MCP integration
- multi-directory context
- durable memory and session summaries

---

## Open Questions

- Should the event bus between CLI and monitor use Unix sockets, named pipes, or local TCP?
- Should session files be plain JSON or a more compact binary format?
- How should multi-workspace support work in the monitor app?
- What is the right granularity for the approval prompt (per-tool-call vs per-task)?
