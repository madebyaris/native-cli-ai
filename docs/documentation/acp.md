# Desktop Clients (ACP)

`nca acp` runs nca as an **Agent Client Protocol** server on stdin/stdout. Desktop
clients that speak ACP — T3 Code, Termul, Zed, and others — can launch it as a
normal local agent, with no Node.js, no bridge process, and no second binary.

```bash
nca acp
```

The client spawns the process, writes JSON-RPC to its stdin, and reads JSON-RPC
from its stdout. nca runs the same agent loop, tools, approvals, and session
store it uses in the terminal; ACP is only a transport and a translation layer.

## Add it to a client

Every ACP client takes an executable plus literal arguments. The entry is the
same everywhere:

| Field | Value |
|---|---|
| Command | `nca` (absolute path if it is not on `PATH`) |
| Arguments | `acp` |
| Working directory | the project the client wants nca to work on |

- **T3 Code** — Settings → Providers → Local ACP command. Paste `nca` and `acp`.
- **Termul** — `AgentConfig { command: "nca", args: ["acp"] }`.
- **Zed** — an `agent_servers` entry with `command: "nca"`, `args: ["acp"]`.

Optional flags:

| Flag | Effect |
|---|---|
| `--model <name>` | Override the model for every session (also a config option in the client). |
| `--safe` | Read-only tools only; shell and writes are unavailable. |
| `--permission-mode <mode>` | Starting permission mode: `default`, `plan`, `accept-edits`, `dont-ask`, `bypass-permissions`. |

The client's working directory decides the workspace. nca loads that
workspace's `.nca/config.local.toml` on top of the global config, exactly like a
terminal run in that directory.

## Requirements

- **An API key must already be configured.** `nca acp` never prompts: stdin is
  the protocol channel. Put the key in `~/.local/share/ncacli/config.toml`, the
  workspace's `.nca/config.local.toml`, or the provider's environment variable
  (for example `MINIMAX_API_KEY`). If it is missing, `session/new` fails with a
  JSON-RPC error naming the missing setting.
- **Nothing but JSON-RPC goes to stdout.** Logs go to stderr. Do not wrap `nca`
  in a script that prints to stdout.

## What the client gets

`initialize` answers protocol version **1** and advertises:

- `loadSession` — reopen a saved session and replay its history.
- `sessionCapabilities.list` — list sessions for the workspace.
- `sessionCapabilities.delete` — delete a listed session and its files.
- `promptCapabilities.image` and `embeddedContext` — image prompts, file
  mentions, and embedded text resources.
- `agentInfo` — `nca` plus the binary version.

### Sessions

| Method | Behaviour |
|---|---|
| `session/new` | Creates a session in the requested `cwd`, persisted like any other nca session. |
| `session/prompt` | Runs one turn. Text, images, `resource_link`, and text resources are accepted. |
| `session/cancel` | Interrupts the running turn and answers `stopReason: "cancelled"`. |
| `session/load` | Replays the saved event log as `session/update` notifications, then resumes the session. |
| `session/list` | Lists saved sessions for the `cwd`, newest first. |
| `session/delete` | Stops the live supervisor, then removes the session JSON and event log. A running prompt blocks deletion. |

`session/load` finishes replaying before it responds, so a client can treat the
response as "history is complete".

### Modes and model picker

Modes map onto nca's permission modes:

| Mode id | Label | Meaning |
|---|---|---|
| `plan` | Plan | Read-only planning; no edits or commands. |
| `ask` | Ask | Same rules as Accept edits: ask only for dangerous actions. |
| `accept-edits` | Accept edits | Run edits and safe commands; ask only for dangerous ones. |
| `dont-ask` | Don't ask | Never prompt; deny anything that would need approval. |
| `bypass` | Bypass | Skip all permission checks. |

`configOptions` also exposes `mode` and `model`. The `model` option has
`category: "model"`, which is what clients use to build a model picker; choosing
a model applies it to the live session and rebuilds the provider. Choices come
from the configured default model, aliases, and recently used models.

### Slash commands

After `session/new` and `session/load`, the agent sends an
`available_commands_update`, so clients can show these in their `/` menu:

| Command | Effect |
|---|---|
| `/compact` | Summarize older context now and report the resulting token count. |
| `/rename <title>` | Set the session title and emit `session_info_update`. |
| `/model <name>` | Switch the model for this session. |
| `/mode <mode>` | Switch permission mode. |

Commands run inside the agent, without a model request. An unknown `/...`
prompt is sent to the model unchanged, so a message such as
`/etc/hosts is broken` still works.

### Approvals

Tool approvals become `session/request_permission` requests with four options:
allow once, allow for this session, reject, and reject for this session.
"Allow for this session" installs nca's usual allow pattern, so repeated calls
of the same shape do not ask again. A cancelled permission request stops the
turn instead of hanging.

### Streaming and tool cards

- Text deltas are coalesced to at most one `agent_message_chunk` per ~16 ms or
  256 bytes, whichever comes first. The last chunk is flushed before the prompt
  response, so clients never lose the tail of an answer.
- Tool calls become `tool_call` / `tool_call_update` with a kind (`read`,
  `edit`, `delete`, `move`, `search`, `execute`, `fetch`, `think`), a title, the
  touched file paths, and the raw input/output. Tool output is truncated at
  16,000 characters before it reaches the client.
- `update_todos` becomes an ACP `plan`.
- Every turn ends with a `usage_update` carrying estimated context tokens, the
  context window size, and cost in USD, so the client's context meter works.
- When the client advertises `session.compaction`, nca also sends
  `compaction_update` notifications for the start and end of a context
  compaction, with the summary text when one was produced.

## Context: large windows, bounded memory

ACP sessions use the same context pipeline as the CLI:

- The context window is resolved from the model (including the provider's
  `/v1/models` metadata when available), so large-window models are not capped
  at a fixed size. MiniMax M2.5 is resolved to 204,800 tokens; M1 to
  1,000,000.
- Before every request the agent fits the conversation to
  `context_window − max_tokens`. When it is close to full, the oldest message
  groups are evicted first, tool-call groups stay atomic, the system prompt,
  the first user task, and the newest turn are never evicted, and a note tells
  the model that older groups were dropped. A single oversized older message is
  shrunk instead of dropping the group; the current request is never truncated.
- AI summaries are stored as a system message and survive the per-turn system
  prompt refresh. An empty summary is treated as a failure and falls back to the
  recent-message window, so real history is never replaced with nothing.
- Sessions are persisted as JSON plus a JSONL event log, so history costs disk,
  not memory.

Idle supervisors are bounded: at most **3** sessions stay loaded by default
(`NCA_ACP_MAX_LIVE_SESSIONS` overrides it). Older idle sessions are saved and
dropped; the next prompt resumes them from disk.

Measured on macOS with the release build (physical footprint):

| State | Footprint |
|---|---|
| After `initialize`, no session | 3.5 MB |
| One live session, idle | 4.8 MB |
| Streaming a reply | 5.2 MB |
| After 10 turns | 5.6 MB |
| 3 live sessions | 5.7 MB |
| 5 sessions opened, 3 live after eviction | 5.8 MB |

Footprint varies by about ±0.2 MB between runs.

## Limits

- ACP v1 only. A client that asks for v2 gets `protocolVersion: 1`.
- One prompt at a time per session; a second concurrent prompt is rejected.
- Client-provided `mcpServers` in `session/new` are accepted but not connected;
  nca uses the MCP servers from its own config.
- The client's `fs/*` capability is not used yet: nca reads and writes the
  workspace directly, so unsaved editor buffers are not visible to its tools.

## Troubleshooting

| Symptom | Cause |
|---|---|
| Client says the agent exited immediately | The command is wrong, or nca is not on `PATH`. Try `nca acp` in a terminal and look at stderr. |
| `session/new` returns "missing ... API key" | Configure the key in config or the provider env var; nca will not prompt on stdin. |
| Garbled protocol / "invalid JSON" | Something else is writing to stdout. Remove wrappers that echo output. |
| No model choices | No models are configured; set `model.default_model` or add aliases. |
| Turns end with `cancelled` | The client sent `session/cancel` (or the turn was interrupted). |
