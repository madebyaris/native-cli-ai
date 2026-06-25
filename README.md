# nca

<p align="center">
  <img src="docs/images/nca-readme-hero.png" alt="nca — Native CLI AI" width="720" />
</p>

<p align="center">
  <strong>Rust-native coding agent. Single binary. Terminal-first.</strong>
</p>

<p align="center">
  <a href="#installation">Install</a> &middot;
  <a href="#quick-start">Quick Start</a> &middot;
  <a href="#core-commands">Commands</a> &middot;
  <a href="#interactive-ux">Interactive UX</a> &middot;
  <a href="#providers">Providers</a> &middot;
  <a href="#funding">Funding</a>
</p>

---

`nca` is a Rust-native coding CLI that ships as a single binary. It is built for local-first, terminal-first workflows: interactive TUI, line REPL, one-shot runs, detached sessions, attach/status/logs, JSON and NDJSON output, Unix-socket IPC, worktree-isolated subagents, and autonomous research helpers.

It is meant for people who like their AI tooling close to the terminal: fast to start, easy to script, and capable of running real session workflows without dragging in a browser shell.

The product surface is the CLI. No desktop wrapper, no Electron, no browser in the default path.

## What It Does

- Runs coding tasks in an interactive TUI or a line-oriented REPL.
- Supports one-shot runs and detached background sessions.
- Persists session state and event logs under the current workspace.
- Exposes machine-readable JSON and NDJSON for automation.
- Spawns child agents with explicit parent/child lineage and optional git worktrees.
- Uses MiniMax by default, with DeepSeek, OpenAI, Anthropic, OpenRouter, and Zhipuai support.
- Loads built-in tools plus optional MCP tools from config.
- Sends **native multimodal** (text + image) messages to MiniMax and other vision-capable models.
- Auto-summarizes long conversations to prevent token overflow.
- Discovers skills from `AGENTS.md` sections, filesystem directories, and user-level skill directories.
- Builds a cached CLI index for workspace-aware agent context.

## Why People Reach For It

- You want a coding CLI that feels quick and stays out of the way.
- You want sessions, event logs, and resumable work instead of a throwaway prompt box.
- You want child agents that can branch off cleanly with lineage and optional git worktrees.
- You want a CLI that still works well when another system is driving it through JSON, NDJSON, and IPC.
- You want intelligent context management that auto-summarizes without losing important context.

## Common Use Cases

| Use case | Why `nca` fits |
|---|---|
| Solo coding in the terminal | Start with `nca`, use the TUI, switch agent profiles, review diffs, and keep everything in one terminal-native flow. |
| Quick one-shot work | `nca run --prompt ...` gives you a focused foreground task without opening a longer session than you need. |
| Background analysis | `nca spawn --prompt ...` lets you kick off work, keep coding, then come back with `status`, `logs`, or `attach`. |
| Multi-agent exploration | Parent and child sessions keep lineage, and child runs can use separate git worktrees for isolation. |
| Automation and orchestration | `--json`, `--stream ndjson`, Unix-socket IPC, and `NCA_ORCH_*` metadata make it usable as a worker process. |
| Long-running research | `nca autoresearch once` runs metric-driven experiments with parsed output for CI/profiling. |

## Installation

### One-line install (macOS and Linux)

```bash
curl -fsSL https://nca-cli.com/install | bash
```

This detects your platform, downloads the latest release from GitHub, and installs `nca` to `/usr/local/bin`. Set `NCA_INSTALL_DIR` to change the install path.

### GitHub Releases

Pre-built binaries for every release are available on the [Releases](https://github.com/madebyaris/native-cli-ai/releases) page:

| Platform | Target |
|---|---|
| macOS (Apple Silicon) | `aarch64-apple-darwin` |
| macOS (Intel) | `x86_64-apple-darwin` |
| Linux (x86_64) | `x86_64-unknown-linux-gnu` |

### Build from source

Requires Rust edition 2024 (use a recent toolchain).

```bash
git clone https://github.com/madebyaris/native-cli-ai.git
cd native-cli-ai
cargo build --release
cp target/release/nca /usr/local/bin/
```

## Quick Start

```bash
# Configure a provider
export MINIMAX_API_KEY="your-api-key"

# Start the interactive CLI
nca

# Line REPL instead of the full-screen TUI
nca --no-tui

# Run one task and exit
nca run --prompt "Explain this repository"

# Spawn a detached session
nca spawn --prompt "Inspect the repo and draft a plan"

# Inspect and attach
nca sessions
nca status <session_id>
nca attach <session_id>
```

The full-screen UI appears when `stdin` and `stdout` are TTYs and `--stream human` is active. Otherwise `nca` falls back to the line-oriented REPL or one-shot execution path.

### Images (full-screen TUI)

In the default TUI you can attach images for the **next** user message:

- **Ctrl+V** — paste a bitmap from the system clipboard (saved as PNG under the session).
- **Ctrl+Shift+C** or **Ctrl+Shift+Insert** — copy selected transcript text to the system clipboard.
- **`/image paste`** — same as clipboard paste if Ctrl+V is not available.
- **`/image path/to/screenshot.png`** — copy a file into the session attachment dir.
- **`/image clear`** — remove staged images before you press Enter.

For **MiniMax**, pasted images are analyzed with the same HTTP API as the MCP's `understand_image` tool (`POST /v1/coding_plan/vlm` on `https://api.minimax.io` or your region host); nca does this in Rust (no Python MCP). The description is merged into the user message before `/v1/messages`. Other providers use their own multimodal chat formats where supported. If the **selected provider/model is not treated as vision-capable**, `nca` **errors** instead of silently dropping images. Session attachment copies are removed automatically after a successful send/process; your original source file is not deleted.

## A Quick Look

The main interface is designed to feel like a serious terminal tool, not a toy overlay.

![nca interactive view](docs/images/nca-show.png)

## Core Commands

| Command | Purpose |
|---|---|
| `nca` | Start the default interactive experience. Auto-resumes the last session unless `--no-resume` is used. |
| `nca run --prompt ...` | Run one task in the foreground. |
| `nca spawn --prompt ...` | Start a detached session and return immediately. |
| `nca resume <session_id>` | Resume a saved session. |
| `nca attach <session_id>` | Attach to a running session over IPC. |
| `nca logs <session_id>` | Read or follow the event log. |
| `nca status <session_id>` | Show session status and metadata. |
| `nca cancel <session_id>` | Mark a detached session as cancelled. |
| `nca sessions` | List saved sessions, with filters like `--status`, `--since-hours`, and `--search`. |
| `nca models` | Show configured models and provider-facing defaults. |
| `nca doctor` | Check provider readiness, skills, and memory/config paths. |
| `nca config` | Print effective config and resolved paths. |
| `nca memory list\|add` | Inspect or append workspace memory notes. |
| `nca skills` | List discovered skills with their source (`AGENTS.md`, filesystem, or user directory). Subcommands: `list`, `add`, `remove`, `update`. |
| `nca mcp` | List configured MCP servers. |
| `nca completion <shell>` | Generate shell completions (bash, zsh, fish, PowerShell, elvish). |
| `nca index build\|show` | Build or inspect a cached CLI index under `~/.nca/workspaces/<workspace-id>/`. |
| `nca autoresearch once <program.md>` | Run a metric-driven research program and print parsed output. |

There is also a hidden `serve` subcommand used for IPC-oriented service sessions.

## Interactive UX

The interactive surface has two modes:

- Full-screen TUI with transcript, composer, approvals, structured questions, slash-command palette, session sidebar, and branch picker.
- Line-oriented REPL built on `reedline` for scripts, terminals where TUI is not desired, or cases where `--no-tui` is easier.

Useful interactive behaviors:

- `! <cmd>` runs a shell command.
- `@ <query>` searches files (fuzzy file mention completions).
- `/...` runs slash commands.
- `Tab` cycles agent profiles (`build` → `plan` → `review` → `fix` → `test`) when the input is empty, or completes the selected slash command without running it.
- `Ctrl+C` or `/stop` cancels the current running turn.
- `/auto-answer` accepts the suggested answer for a pending `ask_question`.

Small touches in the TUI matter too: branch switching, structured options, session sidebars, model picker, provider configuration, and direct control over long-running turns.

![branch picker](docs/images/git-branch.png)

![interactive options](docs/images/option.png)

## Output and Automation

`nca` is designed to work well in two very different moods: terminal-first for humans, and machine-friendly for orchestrators.

- `--stream off` returns only the final output.
- `--stream human` renders the normal terminal experience.
- `--stream ndjson` emits newline-delimited event envelopes.
- `--json` is available on lifecycle-oriented commands such as `spawn`, `sessions`, `status`, `cancel`, `skills`, `models`, `doctor`, `config`, `index show`, and `mcp`.
- `NCA_ORCH_*` and `NCA_ORCH_META_*` environment variables attach orchestration metadata to sessions and harness context.

See [Orchestration Contract](docs/orchestration.md) for the subprocess-facing surface.

## Storage and Paths

`nca` is workspace-first. The current workspace keeps its own session history and local state.

| Path | Purpose |
|---|---|
| `~/.nca/config.toml` | Global config file. |
| `<workspace>/.nca/config.local.toml` | Workspace-local config overrides. |
| `<workspace>/.nca/sessions/<id>.json` | Saved session state. |
| `<workspace>/.nca/sessions/<id>.events.jsonl` | Event log for the session. |
| `<workspace>/.nca/memory.json` | Default memory store. |
| `<workspace>/AGENTS.md` | Repo-local instruction layer; each `## Heading` is also a discoverable skill. |
| `<workspace>/.nca/skills/` | Default workspace skill directory. |
| `~/.nca/skills/` | User-level skill directory. |
| `~/.claude/skills/` | Imported Claude-style skill directory, if present. |
| `<repo>/.nca/worktrees/<session-id>` | Worktree path for isolated child sessions. |
| `$XDG_RUNTIME_DIR/nca/<session_id>.sock` | IPC socket path when `XDG_RUNTIME_DIR` is set. |
| `/tmp/nca/<session_id>.sock` | IPC socket fallback when `XDG_RUNTIME_DIR` is not set. |
| `~/.nca/workspaces/<workspace-id>/cli-index.json` | Cached CLI index for agents and tooling. |
| `.ncarc` | Project instructions file committed with the repo, if present. |
| `.nca/instructions.md` | Local instructions file. |

## Skills System

`nca` discovers skills from multiple sources with visible provenance:

1. **`AGENTS.md`** — Each root-level `## Heading` becomes a slash-invokable skill. Optional directive bullets can set `model=...`, `permission_mode=...`, and `context=...`.
2. **Filesystem directories** — Skills from `.nca/skills/`, `~/.nca/skills/`, and `~/.claude/skills/`.
3. **Built-in skills** — Core skills baked into the binary.

Use `nca skills --json` to see all discovered skills with their sources.

## Context Management

`nca` automatically manages conversation context to prevent token overflow:

- **Token estimation** uses character-based approximation (chars / 4, with tool-message adjustments).
- **Auto-summarize** kicks in when context reaches a configurable threshold (default 75%).
- **Summary format** preserves key topics, decisions, and critical context.
- System messages are always preserved; recent messages use a sliding window.

### Size Guardrails

Several guards prevent any single turn from blowing the context window:

- Tool output truncated at 32 KB (head+tail strategy) before entering the model context.
- `list_directory` capped at 1,000 entries.
- Skill descriptions clipped to 120 chars in the system prompt index.
- Skills index capped at 4,000 chars total in `harness.rs`.

### Cache Token Accounting

Cost tracking includes cache-aware token pricing. `cache_read` tokens (e.g. from DeepSeek prefix caching) are billed at 1/50 of the normal input rate.

Configuration in `~/.nca/config.toml`:

```toml
[memory.context]
context_window_target = 0          # 0 = auto-detect from model; set explicitly to override
max_retained_messages = 50
auto_summarize_threshold = 75     # percentage of context window
enable_auto_summarize = true
```

## Providers

MiniMax is the default provider path. The codebase also supports DeepSeek, OpenAI, Anthropic, OpenRouter, and Zhipuai, so the project can stay MiniMax-first without boxing itself into one provider forever.

Typical environment variables:

- `MINIMAX_API_KEY`
- `DEEPSEEK_API_KEY`
- `OPENAI_API_KEY`
- `ANTHROPIC_API_KEY`
- `OPENROUTER_API_KEY`
- `ZHIPUAI_API_KEY`

DeepSeek sessions benefit from provider-level token optimizations: `reasoning_content` (thinking chain) is stripped before re-uploading to save ~500 prompt-input tokens per turn and keep the prefix cache byte-stable.

Provider config is loaded from defaults, then `~/.nca/config.toml`, then `<workspace>/.nca/config.local.toml`, then environment overrides.

Use `nca doctor` to verify provider readiness and `nca models` to inspect model selection.

## Harness and Tooling

The system prompt is layered in this order:

1. Built-in harness prompt
2. Permission-mode guidance
3. Global Instructions (`~/.nca/AGENTS.md`, user-level)
4. `AGENTS.md` (workspace repo-level instructions)
5. `.ncarc` (committed project instructions, if present)
6. `.nca/instructions.md` (local instructions)
7. Discovered skills summary
8. Orchestration context (`NCA_ORCH_*` env vars)

The built-in tool surface includes filesystem editing, search, diffing, patching, shell execution, web access, `ask_question`, and `spawn_subagent`. MCP tools are loaded dynamically via the `rmcp` crate when configured, so the available tool set can grow with your environment.

### Search And Edit Tools

Recent search/edit improvements are aimed at making agent file work less brittle:

- `search_code` now returns structured JSON match objects instead of raw `rg` text.
- `search_code` treats ripgrep exit code `1` as a successful empty result, not a failure.
- `search_code` supports `path`, `glob`, `fixed_strings`, `case_sensitive`, `word`, `context_before`, `context_after`, and `max_results`.
- `query_symbols` is a literal Rust symbol lookup, not an implicit regex expansion of user input.
- `edit_file` and `apply_patch` now fail loudly on ambiguous single-match edits instead of silently changing the first occurrence.
- `replace_match` can edit a specific search result by exact `path`, `line`, and `column`, which makes search -> edit flows much safer.

## Crate Layout

| Crate | Responsibility |
|---|---|
| `crates/common` | Shared config, events, sessions, messages, tool schemas, and orchestration metadata. |
| `crates/core` | Agent loop, provider abstraction, harness builder, skills, approvals, and tool registry. |
| `crates/runtime` | Session supervision, IPC, persistence, worktrees, memory store, context management, and subagent execution. |
| `crates/cli` | `nca` entrypoint, command parsing, stream rendering, REPL, and TUI. |
| `crates/autoresearch` | Metric-driven autonomous research helpers and experiment runner. |

## Session Model

- Sessions are persisted as JSON snapshots plus JSONL event logs.
- The runtime uses a `Supervisor` to own lifecycle, IPC, approvals, questions, event fanout, and persistence.
- Child sessions can inherit parent context, record lineage in session metadata, and run inside separate git worktrees.
- IPC uses newline-delimited JSON over Unix sockets so `attach`, approvals, status, and other controls share one runtime transport.
- `ContextManager` tracks token usage and auto-summarizes long conversations.

In practice, that means you can start small, branch out when a task gets bigger, and still keep a clean trail of what happened.

## Documentation

Full user-facing documentation lives in [`docs/documentation/`](docs/documentation/index.md):

| Page | Description |
|---|---|
| [Getting Started](docs/documentation/getting-started.md) | Installation, first run, and initial configuration |
| [Commands](docs/documentation/commands.md) | Complete CLI command and flag reference |
| [Interactive Mode](docs/documentation/interactive-mode.md) | TUI, REPL, slash commands, keyboard shortcuts |
| [Configuration](docs/documentation/configuration.md) | Config files, TOML format, and environment variables |
| [Providers](docs/documentation/providers.md) | LLM provider setup — MiniMax, DeepSeek, Anthropic, OpenAI, OpenRouter, Zhipuai |
| [Tools](docs/documentation/tools.md) | All agent tools — file ops, search, shell, web, and more |
| [Sessions](docs/documentation/sessions.md) | Session lifecycle, persistence, resume, and management |
| [Permissions](docs/documentation/permissions.md) | Approval system, permission modes, and safe mode |
| [Skills](docs/documentation/skills.md) | Skill discovery, installation, and authoring |
| [Advanced](docs/documentation/advanced.md) | Sub-agents, MCP servers, hooks, orchestration, and IPC |

Internal design docs:

- [Product Requirements](docs/prd.md)
- [Tech Stack](docs/tech-stack.md)
- [Architecture](docs/architecture.md)
- [Orchestration Contract](docs/orchestration.md)
- [Context Management](docs/context-management.md)
- [Performance Optimization Research](docs/research/rust-ratatui-optimization.md)

## Funding

`nca` is an independent project built by [Aris Setia](https://github.com/madebyaris). It is not backed by a company and is not powered by any single provider — the multi-provider architecture is intentional so users can choose what works for them.

MiniMax has supported this work through content collaboration and developer events in Indonesia, which helped make the early development possible. Sustaining full-time work on the Rust-native AI ecosystem requires ongoing support.

If you find `nca` useful and want to help keep it going:

| Channel | Link |
|---|---|
| GitHub Sponsors | [github.com/madebyaris](https://github.com/madebyaris) |
| PayPal | [paypal.me/airs](https://paypal.me/airs) (arissetia.m@gmail.com) |

Your support directly funds full-time development on `nca` and the broader Native CLI ecosystem — better provider support, performance work, new tools, and documentation.

## License

MIT
