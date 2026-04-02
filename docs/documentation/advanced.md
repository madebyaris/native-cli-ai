# Advanced

Sub-agents, MCP integration, lifecycle hooks, orchestration, memory, and IPC.

## Sub-Agents

nca can spawn child agent sessions to handle tasks in parallel. The parent agent uses the `spawn_subagent` tool to delegate work.

### How Sub-Agents Work

```
Parent Session
├── spawn_subagent(task: "write tests for auth")
│   └── Child Session (new session ID)
│       ├── Inherits conversation context summary
│       ├── Runs in isolated git worktree (optional)
│       ├── Uses bypass-permissions (no interactive prompts)
│       ├── Executes the task autonomously
│       └── Returns result to parent
└── Continues with child's output
```

### Spawning

The agent calls `spawn_subagent` with:

```json
{
    "task": "Write comprehensive tests for the authentication module",
    "focus_files": ["src/auth.rs", "src/auth/middleware.rs"],
    "use_worktree": true
}
```

### Git Worktrees

When `use_worktree` is true (default), the child session runs in an isolated git worktree:

- Worktree path: `<repo>/.nca/worktrees/<session-id>`
- Branch: `nca/<session-id>` (created from current `HEAD`)
- Changes in the worktree don't affect the parent's working directory

### Child Session Behavior

- **Permissions:** `bypass-permissions` (no interactive approval needed)
- **Approvals:** Auto-deny handler (no blocking waits)
- **Context:** Inherits a summary of the parent's last ~10 messages
- **Timeout:** 600 seconds (10 minutes)
- **Lineage:** Parent and child session IDs are cross-referenced in metadata

### Viewing Sub-Agents

```
/agents                    # List child sessions in interactive mode
```

```bash
nca sessions --search "child"   # Find sub-agent sessions via CLI
```

### Sub-Agent Events

| Event | Description |
|-------|-------------|
| `ChildSessionSpawned` | Child session created |
| `ChildSessionCompleted` | Child session finished |
| `ChildSessionActivity` | Intermediate activity from child |

---

## MCP Servers

nca supports the [Model Context Protocol](https://modelcontextprotocol.io/) for integrating external tool servers.

### Configuration

Add MCP servers in your config:

```toml
[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allowed/dir"]
enabled = true

[[mcp.servers]]
name = "database"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-postgres"]
env = { DATABASE_URL = "postgresql://localhost/mydb" }
enabled = true

[[mcp.servers]]
name = "custom"
command = "/usr/local/bin/my-mcp-server"
args = ["--port", "3000"]
cwd = "/opt/my-server"
env = { API_KEY = "secret" }
enabled = true
```

### MCP Server Config Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | **required** | Server identifier (used in tool names) |
| `command` | string | **required** | Command to start the server |
| `args` | string[] | `[]` | Command arguments |
| `env` | map | `{}` | Environment variables |
| `cwd` | string | — | Working directory |
| `enabled` | bool | `true` | Whether the server is active |

### How MCP Tools Appear

MCP tools are exposed with the naming convention:

```
mcp__<server_name>__<tool_name>
```

For example, a server named `database` with a tool `query` becomes `mcp__database__query`.

### Listing MCP Servers

```bash
nca mcp              # CLI
nca mcp --json       # JSON output
```

```
/mcp                  # Interactive mode
```

### Safe Mode

By default, MCP tools are **not available** in safe mode. To allow them:

```toml
[mcp]
expose_in_safe_mode = true
```

---

## Lifecycle Hooks

Hooks let you run shell commands at various points in the session lifecycle.

### Available Hook Points

| Hook | When It Fires |
|------|---------------|
| `session_start` | Session begins |
| `session_end` | Session ends |
| `pre_tool_use` | Before a tool executes |
| `post_tool_use` | After a tool succeeds |
| `post_tool_failure` | After a tool fails |
| `approval_requested` | When user approval is needed |
| `subagent_start` | When a sub-agent is spawned |
| `subagent_stop` | When a sub-agent completes |

### Configuration

```toml
[[hooks.session_start]]
command = "echo 'Session started' >> /tmp/nca.log"
blocking = false

[[hooks.pre_tool_use]]
command = "my-audit-script --tool $NCA_TOOL_NAME"
matcher = "execute_bash"    # Only fire for bash tool
blocking = true             # Wait for completion before proceeding

[[hooks.session_end]]
command = "notify-send 'nca session ended'"
blocking = false
```

### Hook Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | string | **required** | Shell command to execute |
| `matcher` | string | `""` | Regex pattern to filter when the hook fires |
| `blocking` | bool | `false` | Whether to wait for the hook to complete |

---

## Persistent Memory

The memory system stores notes that persist across sessions.

### CLI Commands

```bash
nca memory                                    # List all notes
nca memory list --json                        # JSON output
nca memory add "prefer async/await patterns"  # Add a note
nca memory add "uses PostgreSQL" --kind note  # With type
```

### Interactive Commands

```
/memory                    # List notes
/memory always use anyhow for error handling   # Add a note
```

### Configuration

```toml
[memory]
file_path = ".nca/memory.json"    # Storage location
max_notes = 128                    # Maximum number of notes
```

### How Memory Works

- Notes are stored as JSON in the workspace
- Notes are included in the agent's context on each turn
- The agent can reference stored notes for consistency across sessions
- Notes are workspace-scoped (stored in `.nca/memory.json`)

---

## Orchestration

nca can be driven by external orchestrators (CI systems, automation scripts, custom tooling) via environment variables and structured output.

### Orchestration Environment Variables

Set these to inject orchestration context into the session:

```bash
export NCA_ORCH_NAME="github-actions"
export NCA_ORCH_RUN_ID="run-123"
export NCA_ORCH_TASK_ID="task-456"
export NCA_ORCH_TASK_REF="PR-42"
export NCA_ORCH_PARENT_RUN_ID="parent-run-789"
export NCA_ORCH_CALLBACK_URL="https://my-ci/callback"
export NCA_ORCH_META_REPO="my-org/my-repo"
export NCA_ORCH_META_BRANCH="feature/auth"
```

Orchestration metadata is:
- Stored in session state
- Injected into the system prompt
- Available to hooks

### Headless Execution

For CI/automation, combine orchestration with non-interactive flags:

```bash
nca run \
    --prompt "run tests and report results" \
    --permission-mode bypass-permissions \
    --json \
    --stream ndjson
```

### NDJSON Event Stream

Use `--stream ndjson` to get machine-readable events:

```bash
nca run --prompt "fix the build" --stream ndjson | while read -r event; do
    echo "$event" | jq '.event_type'
done
```

Each line is a JSON object with:
- `id` — monotonic event ID
- `ts` — timestamp
- Event-specific fields (message content, tool calls, approvals, etc.)

---

## IPC Protocol

Running sessions expose a Unix domain socket for external control.

### Socket Location

```
$XDG_RUNTIME_DIR/nca/<session-id>.sock
# Fallback:
/tmp/nca/<session-id>.sock
```

### Protocol

Newline-delimited JSON over Unix domain socket.

**Server → Client (events):**

Events are broadcast as `EventEnvelope` objects:

```json
{"id": 1, "ts": "2026-04-02T10:30:00Z", "event": {"type": "MessageReceived", ...}}
{"id": 2, "ts": "2026-04-02T10:30:01Z", "event": {"type": "ToolCallStarted", ...}}
```

**Client → Server (commands):**

```json
{"type": "SendMessage", "content": "continue with the refactoring"}
{"type": "ApproveToolCall", "call_id": "abc123"}
{"type": "DenyToolCall", "call_id": "abc123"}
{"type": "AnswerQuestion", "question_id": "q1", "answer": "option-a"}
{"type": "Cancel"}
{"type": "Shutdown"}
```

### Event Types

| Event | Description |
|-------|-------------|
| `SessionStarted` | Session initialized |
| `SessionEnded` | Session finished (with reason) |
| `MessageReceived` | User or assistant message |
| `TokensStreamed` | Streaming token delta |
| `CostUpdated` | Token usage update |
| `ToolCallStarted` | Tool execution began |
| `ToolCallCompleted` | Tool execution finished |
| `ApprovalRequested` | Waiting for user approval |
| `ApprovalResolved` | Approval decision made |
| `QuestionRequested` | Agent asking user a question |
| `QuestionResolved` | Question answered |
| `ChildSessionSpawned` | Sub-agent created |
| `ChildSessionCompleted` | Sub-agent finished |
| `ChildSessionActivity` | Sub-agent intermediate activity |
| `ContextWarning` | Context window getting full |
| `ContextCompaction` | Context was compacted |
| `BusyStateChanged` | Agent state changed (Thinking, Streaming, Idle) |
| `Checkpoint` | Session checkpoint saved |
| `Error` | Error occurred |

---

## Custom Instructions

nca loads instructions from multiple sources, merged in order:

### Loading Order

1. **Built-in system prompt** — nca's core behavior rules (disable with `harness.built_in_enabled = false`)
2. **`AGENTS.md`** — project-level instructions in the workspace root
3. **`.ncarc`** — nca-specific project instructions (configurable path)
4. **`.nca/instructions.md`** — personal local instructions (gitignored)
5. **Skills** — available skills listed for on-demand invocation
6. **Orchestration context** — injected when orchestration env vars are present

### `AGENTS.md`

Placed in the workspace root. Compatible with other AI tools (Claude Code, etc.):

```markdown
## Project Rules

- Use axum for HTTP servers
- All errors must use thiserror
- Write tests for every public function
```

### `.ncarc`

nca-specific project instructions:

```markdown
## Stack

- Rust + Tokio async runtime
- PostgreSQL via sqlx
- Redis via deadpool-redis

## Conventions

- Module names are snake_case
- Error types end with Error
- Tests live in the same file as the code
```

### `.nca/instructions.md`

Personal instructions (add to `.gitignore`):

```markdown
## My Preferences

- I prefer verbose variable names
- Always add doc comments to public items
- Use `tracing` for logging, not `log`
```

---

## Auto-Research

nca includes an auto-research feature for automated investigation:

```bash
nca autoresearch once <program> [--workspace <path>]
```

This runs a single automated research pass on a program or topic, generating structured output in the workspace.

---

## Doctor Command

Run diagnostics to verify your setup:

```bash
nca doctor
```

Checks:
- Configuration file validity
- API key availability for configured providers
- Provider connectivity
- Required tool dependencies (git, rg)
- File system permissions
