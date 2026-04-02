# Commands

Complete reference for all `nca` CLI commands, subcommands, and flags.

## Global Usage (No Subcommand)

```bash
nca [OPTIONS]
```

When invoked without a subcommand, nca starts an interactive session. Behavior depends on flags:

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--prompt` | `-p` | string | — | Run a one-shot prompt and exit |
| `--safe` | `-s` | flag | false | Start in read-only safe mode |
| `--resume` | `-r` | flag | false | Resume the most recent session |
| `--no-resume` | — | flag | false | Force a new session (skip auto-resume) |
| `--run` | — | flag | false | Start interactive run mode |
| `--model` | — | string | — | Override the default model |
| `--enable-thinking` | `-t` | flag | false | Enable extended thinking/reasoning |
| `--thinking-budget` | — | u32 | 5120 | Token budget for extended thinking |
| `--max-tokens` | — | u32 | 8192 | Max response tokens |
| `--verbose` | `-v` | flag | false | Verbose debug logging |
| `--json` | — | flag | false | Output structured JSON (for CI) |
| `--stream` | — | enum | `human` | Stream format: `human`, `ndjson`, or `off` |
| `--no-tui` | — | flag | false | Use line-oriented REPL instead of full-screen TUI |
| `--permission-mode` | — | enum | — | Permission mode (see [Permissions](./permissions.md)) |
| `--max-turns` | — | u32 | — | Max agent turns per run |

### Examples

```bash
# Start interactive TUI session
nca

# One-shot prompt
nca -p "refactor the error handling in src/lib.rs"

# Safe mode (read-only analysis)
nca -s

# Resume last session
nca -r

# Force new session
nca --no-resume

# Use a specific model
nca --model "claude-3-7-sonnet-latest"

# Enable thinking with custom budget
nca -t --thinking-budget 10000

# CI-friendly JSON output
nca -p "list all TODO comments" --json

# NDJSON event stream
nca -p "fix the tests" --stream ndjson

# Line REPL (no full-screen TUI)
nca --no-tui

# Bypass all permission prompts
nca --permission-mode bypass-permissions
```

---

## Subcommands

### `nca run`

Run a one-shot task with explicit stream control.

```bash
nca run --prompt "..." [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--prompt` | string | **required** | The task to execute |
| `--stream` | enum | `human` | Stream format: `human`, `ndjson`, `off` |
| `--model` | string | — | Override model |
| `--json` | flag | false | Structured JSON output |
| `--safe` | flag | false | Read-only mode |
| `--permission-mode` | enum | — | Permission mode |

```bash
nca run --prompt "add input validation" --stream ndjson
nca run --prompt "analyze code quality" --safe --json
```

---

### `nca spawn`

Launch a background session that runs without interactive input.

```bash
nca spawn --prompt "..." [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--prompt` | string | **required** | The task to execute |
| `--model` | string | — | Override model |
| `--safe` | flag | false | Read-only mode |
| `--json` | flag | false | Structured JSON output |
| `--permission-mode` | enum | `accept-edits` | Permission mode |

```bash
nca spawn --prompt "write comprehensive tests for the auth module"
nca spawn --prompt "document all public APIs" --model "MiniMax-M2.7"
```

---

### `nca sessions`

List and filter saved sessions.

```bash
nca sessions [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | JSON output |
| `--status` | enum | — | Filter by status: `running`, `completed`, `cancelled`, `failed` |
| `--since-hours` | u32 | — | Show sessions from the last N hours |
| `--search` | string | — | Search sessions by text |
| `--limit` | usize | 20 | Max number of sessions to show |

```bash
nca sessions
nca sessions --status running
nca sessions --since-hours 24 --limit 5
nca sessions --search "auth" --json
```

---

### `nca resume`

Resume a previously saved session.

```bash
nca resume <SESSION_ID> [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--prompt` | string | — | Send a follow-up prompt after resuming |
| `--model` | string | — | Override model for this session |
| `--safe` | flag | false | Resume in read-only mode |
| `--stream` | enum | `human` | Stream format |
| `--no-tui` | flag | false | Use line REPL |
| `--permission-mode` | enum | — | Permission mode |

```bash
nca resume abc123
nca resume abc123 --prompt "continue where you left off"
nca resume abc123 --model "claude-3-7-sonnet-latest"
```

---

### `nca logs`

Stream or dump the event log for a session.

```bash
nca logs <SESSION_ID> [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--follow` | flag | false | Follow the log in real-time (like `tail -f`) |
| `--json` | flag | false | Raw JSON output |

```bash
nca logs abc123
nca logs abc123 --follow
nca logs abc123 --json
```

---

### `nca attach`

Attach to a running session's output stream.

```bash
nca attach <SESSION_ID> [--json]
```

---

### `nca status`

Show metadata for a session.

```bash
nca status <SESSION_ID> [--json]
```

---

### `nca cancel`

Cancel a running session.

```bash
nca cancel <SESSION_ID> [--json]
```

---

### `nca skills`

Manage agent skills.

```bash
nca skills [--json]              # List all discovered skills
nca skills list [--json]         # Same as above
nca skills add <SOURCE> [OPTIONS]
nca skills remove <NAME> [OPTIONS]
nca skills update [NAME]         # Update one or all skills
```

**`nca skills add`:**

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--skill` | `-s` | string[] | all | Specific skills to install (repeatable) |
| `--global` | `-g` | flag | false | Install globally to `~/.nca/skills/` |

**`nca skills remove`:**

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--global` | `-g` | flag | false | Remove from global skills |

```bash
nca skills
nca skills add https://github.com/user/skill-repo -s rust-patterns
nca skills remove rust-patterns --global
nca skills update
```

---

### `nca mcp`

List configured MCP (Model Context Protocol) servers.

```bash
nca mcp [--json]
```

---

### `nca memory`

Manage persistent memory notes.

```bash
nca memory [--json]              # List memory notes
nca memory list [--json]         # Same as above
nca memory add <TEXT> [--kind <KIND>]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--kind` | string | `note` | Type of memory entry |

```bash
nca memory
nca memory add "prefer async/await over thread spawning"
nca memory add "API uses bearer token auth" --kind note
```

---

### `nca models`

List available models for the current provider.

```bash
nca models [--json]
```

---

### `nca doctor`

Run diagnostic checks on your configuration.

```bash
nca doctor [--json]
```

Checks API key availability, provider connectivity, config file validity, and tool dependencies.

---

### `nca config`

Display the current runtime configuration.

```bash
nca config [--json]
```

---

### `nca completion`

Generate shell completion scripts.

```bash
nca completion <SHELL>
```

Supported shells: `bash`, `zsh`, `fish`, `power-shell`, `elvish`. Default: `bash`.

---

### `nca index`

Manage the CLI index cache (used for agent self-awareness of available commands).

```bash
nca index build [--json]
nca index show [--json]
```

---

### `nca autoresearch`

Run automated research on a program/topic.

```bash
nca autoresearch once <PROGRAM> [--workspace <PATH>]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--workspace` | path | current directory | Workspace for research output |

---

## Stream Modes

The `--stream` flag controls output format:

| Mode | Description |
|------|-------------|
| `human` | Terminal-friendly output with colors, markdown rendering, and TUI support (default) |
| `ndjson` | Newline-delimited JSON events — one event per line, for machine consumption |
| `off` | Minimal output — final result only |

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error (provider failure, config issue, etc.) |
| non-zero | Task failure or cancellation |
