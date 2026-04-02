# Sessions

nca persists every conversation as a session. Sessions can be resumed, inspected, attached to, and managed through the CLI.

## Session Lifecycle

```
nca (start) → Session Created → Agent Turns → Session Ended
                    ↓                              ↓
              .nca/sessions/<id>.json         .nca/sessions/<id>.events.jsonl
                    ↓                              ↓
              State snapshot                  Full event log
```

### States

| Status | Description |
|--------|-------------|
| `running` | Session is currently active |
| `completed` | Session finished successfully |
| `cancelled` | Session was cancelled by the user |
| `failed` | Session ended with an error |

## Session Storage

Sessions are stored in the workspace under `.nca/sessions/`:

```
.nca/sessions/
├── a1b2c3d4.json            # Session state snapshot
├── a1b2c3d4.events.jsonl    # Event log (NDJSON)
├── e5f6g7h8.json
├── e5f6g7h8.events.jsonl
└── ...
```

### State File (`.json`)

Contains session metadata:

- Session ID and timestamps (created, updated)
- Workspace path and working directory
- Model and provider configuration
- Session status and PID
- Socket path for IPC
- Parent/child session relationships
- Session summary
- Git branch information
- Worktree path (for sub-agent sessions)

### Event Log (`.events.jsonl`)

Append-only NDJSON log of every event in the session:

- Messages sent and received
- Token usage and streaming events
- Tool call starts and completions
- Approval requests and resolutions
- Child session spawns and completions
- Context warnings and compaction events
- Errors and status changes

Each event has a monotonic `id` and `ts` (timestamp).

## Managing Sessions

### List Sessions

```bash
nca sessions                           # List recent sessions (default: 20)
nca sessions --limit 50                # Show more
nca sessions --status running          # Filter by status
nca sessions --since-hours 24          # Last 24 hours
nca sessions --search "auth"           # Search by content
nca sessions --json                    # JSON output
```

### Resume a Session

Resume picks up a session with full conversation context:

```bash
# Resume by ID
nca resume <session-id>

# Resume with a follow-up prompt
nca resume <session-id> --prompt "continue with the tests"

# Resume with a different model
nca resume <session-id> --model "claude-3-7-sonnet-latest"

# Resume the most recent session (shorthand)
nca -r
nca --resume
```

### Auto-Resume Behavior

By default, when you run `nca` without any arguments:

1. nca checks `.nca/.last_session` for the most recent session ID
2. If a valid recent session exists, nca **auto-resumes** it with a hint to stderr
3. If no valid session exists, a new session starts

Override this:

```bash
nca --no-resume    # Always start fresh
nca --resume       # Always resume last session
```

### View Session Logs

```bash
nca logs <session-id>              # Dump event log
nca logs <session-id> --follow     # Stream in real-time
nca logs <session-id> --json       # Raw JSON events
```

### Attach to a Running Session

```bash
nca attach <session-id>            # Attach to output stream
nca attach <session-id> --json     # JSON event stream
```

### Check Session Status

```bash
nca status <session-id>            # Show metadata
nca status <session-id> --json     # JSON output
```

### Cancel a Running Session

```bash
nca cancel <session-id>
```

### Interactive Session Switching

In the TUI, use `/sessions` to open a session picker or press `Ctrl+X L`:

```
/sessions                  # Open session picker
/new                       # Start a new session
```

## Session Context Management

### Context Compaction

As conversations grow, nca can summarize and compact the context to stay within the model's context window:

```
/compact                   # Manually compact context
```

Automatic compaction is controlled by:

```toml
[memory.context]
auto_summarize_threshold = 75    # Trigger at 75% of context window
enable_auto_summarize = true
max_retained_messages = 50
```

When auto-summarize triggers, nca:
1. Emits a `ContextWarning` event
2. Summarizes the conversation history
3. Replaces older messages with the summary
4. Emits a `ContextCompaction` event

### Checkpointing

Sessions are checkpointed periodically to prevent data loss:

```toml
[session]
checkpoint_interval = 5    # Save every 5 turns
```

### Session Export

Export a session to markdown:

```
/export
```

## Session Configuration

```toml
[session]
history_dir = ".nca/sessions"         # Where sessions are stored
max_turns_per_run = 128               # Max turns before session ends
max_tool_calls_per_turn = 200         # Max tools per single turn
checkpoint_interval = 5               # Checkpoint frequency
last_session_file = ".nca/.last_session"  # Last session pointer
auto_compact_on_finish = false        # Summarize on session end
```

## Background Sessions

Use `nca spawn` to create sessions that run in the background:

```bash
nca spawn --prompt "refactor the auth module"
```

Spawned sessions:
- Run without interactive input
- Default to `accept-edits` permission mode
- Can be monitored with `nca logs`, `nca attach`, `nca status`
- Can be cancelled with `nca cancel`

## IPC (Inter-Process Communication)

Running sessions expose a Unix domain socket for real-time event streaming and control:

```
$XDG_RUNTIME_DIR/nca/<session-id>.sock
# or
/tmp/nca/<session-id>.sock
```

The IPC protocol uses newline-delimited JSON:

**Events (session → client):** `EventEnvelope` objects containing `AgentEvent` variants.

**Commands (client → session):**
- `SendMessage` — send a user message
- `ApproveToolCall` — approve a pending tool call
- `DenyToolCall` — deny a pending tool call
- `AnswerQuestion` — answer an agent question
- `Cancel` — cancel the current operation
- `Shutdown` — shut down the session

This enables building external UIs, monitoring dashboards, and automation scripts that interact with running sessions.

## Parent-Child Sessions

When the agent spawns sub-agents, a parent-child relationship is tracked:

- Parent session records child session IDs
- Child session records parent session ID and inherited summary
- The spawn reason is stored in child metadata
- Child sessions can optionally run in isolated git worktrees

View child sessions:

```
/agents                    # List child sessions in interactive mode
```

```bash
nca sessions --search "child"   # Search for sub-agent sessions
```
