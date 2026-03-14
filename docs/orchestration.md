# Orchestration Contract

This document defines the public subprocess contract for running `nca` under an external orchestrator.

The goal is to make `nca` usable as a headless worker without coupling the project to any one control plane.

## Supported Commands

These commands are the supported orchestration-facing surfaces:

| Command | Purpose | Machine-readable output |
|---|---|---|
| `nca run --prompt ... --stream off --json` | Run a foreground task and return a final result | JSON object |
| `nca run --prompt ... --stream ndjson` | Run a foreground task and stream live events | NDJSON `EventEnvelope` lines |
| `nca spawn --prompt ... --json` | Start a detached session | JSON object |
| `nca status <session_id> --json` | Read the current saved session snapshot | JSON object |
| `nca sessions --json` | List known sessions | JSON object |
| `nca attach <session_id>` | Stream live event envelopes from IPC or fall back to the event log | NDJSON `EventEnvelope` lines |
| `nca logs <session_id>` | Replay persisted event envelopes from disk | NDJSON `EventEnvelope` lines |
| `nca cancel <session_id> --json` | Stop a session and persist cancelled state | JSON object |

`nca serve` exists for the local monitor/runtime flow, but it is still treated as an internal command rather than part of the public orchestration contract.

## Event Stream Shape

Machine event streams use the same envelope shape on stdout, in IPC, and in `.nca/sessions/<session-id>.events.jsonl`:

```json
{
  "id": 12,
  "ts": "2026-03-14T08:00:00Z",
  "event": {
    "type": "ToolCallStarted",
    "call_id": "call_123",
    "tool": "read_file",
    "input": {
      "path": "src/main.rs"
    }
  }
}
```

The `event` payload is the tagged `AgentEvent` enum from `crates/common/src/event.rs`.

Lifecycle-critical events:

- `SessionStarted`
- `MessageReceived`
- `ToolCallStarted`
- `ToolCallCompleted`
- `ApprovalRequested`
- `ApprovalResolved`
- `Checkpoint`
- `Response`
- `SessionEnded`
- `ChildSessionSpawned`
- `ChildSessionCompleted`

## Session Snapshot Shape

`status --json`, `sessions --json`, and the final `run --json` output are built around the shared `SessionSnapshot` shape from `crates/common/src/session.rs`.

Important fields:

- `id`
- `status`
- `workspace`
- `model`
- `pid`
- `socket_path`
- `updated_at`
- `estimated_cost_usd`
- `total_input_tokens`
- `total_output_tokens`
- `orchestration`

The `orchestration` field is optional and only appears when the run was launched with `NCA_ORCH_*` metadata.

## Command Outputs

### `run --stream off --json`

Returns:

```json
{
  "session": {
    "id": "session-123",
    "status": "completed"
  },
  "output": "final assistant text",
  "end_reason": "completed"
}
```

### `spawn --json`

Returns:

```json
{
  "session_id": "session-123",
  "pid": 4242,
  "status_path": ".nca/sessions/session-123.json",
  "event_log_path": ".nca/sessions/session-123.events.jsonl",
  "spawn_log_path": ".nca/sessions/session-123.spawn.log",
  "socket_path": "/tmp/nca/session-123.sock",
  "permission_mode": "bypass-permissions",
  "safe_mode": false
}
```

### `sessions --json`

Returns:

```json
{
  "sessions": [
    {
      "id": "session-newer",
      "status": "running"
    }
  ],
  "unreadable": []
}
```

### `cancel --json`

Returns:

```json
{
  "session": {
    "id": "session-123",
    "status": "cancelled"
  },
  "cancelled": true
}
```

## Headless Permission Guidance

For orchestrated runs, prefer one of these modes:

- `--permission-mode dont-ask`: read-only headless execution
- `--permission-mode bypass-permissions`: fully autonomous execution

Avoid `default` and `accept-edits` for unattended subprocess runs unless the orchestrator is prepared for approval failures.

If a headless run reaches a tool call that would require user approval, `nca` exits with a dedicated approval-blocked exit code instead of waiting indefinitely.

## Exit Codes

These exit codes are intended to stay stable for orchestrators:

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Unclassified/internal failure |
| `10` | Configuration failure |
| `11` | Runtime/provider/tool failure |
| `13` | Approval-blocked headless run |
| `130` | Cancelled run |

## Orchestration Metadata Environment Contract

`nca` reads optional orchestration metadata from these environment variables:

| Variable | Meaning |
|---|---|
| `NCA_ORCH_NAME` | Orchestrator name |
| `NCA_ORCH_RUN_ID` | Current external run identifier |
| `NCA_ORCH_TASK_ID` | Current task identifier |
| `NCA_ORCH_TASK_REF` | Human-readable task reference |
| `NCA_ORCH_PARENT_RUN_ID` | Parent external run identifier |
| `NCA_ORCH_CALLBACK_URL` | Callback or control endpoint hint |
| `NCA_ORCH_META_<KEY>` | Free-form metadata entries |

This metadata is persisted into session state and injected into the layered system prompt as coordination context. It does not create any implicit network behavior by itself.

## Wrapper Example

Example subprocess flow for a Paperclip-like orchestrator:

1. Export headless context:
   `NCA_ORCH_NAME=paperclip-wrapper`
   `NCA_ORCH_RUN_ID=<run-id>`
   `NCA_ORCH_TASK_ID=<task-id>`
2. Launch:
   `nca run --prompt "$PROMPT" --stream off --json --permission-mode bypass-permissions`
3. Parse the final JSON output and persisted `session.id`.
4. If live progress is needed, use:
   `nca run --prompt "$PROMPT" --stream ndjson --permission-mode bypass-permissions`
   or `nca attach <session_id>`.

## Compatibility Roadmap

This subprocess contract is the first compatibility layer.

Planned later layers:

- formal local IPC API over the existing Unix socket
- optional HTTP/SSE or A2A-style adapter on top of `runtime + common`
- orchestrator-specific wrappers only after the generic contract is stable
