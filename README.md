# nca

<p align="center">
  <img src="docs/images/nca-readme-run.svg" alt="nca: a terminal runner jumps five monsters, one per issue on the next-release list" width="720" />
</p>

<p align="center">
  <strong>Rust-native coding agent. Single binary. Terminal-first.</strong><br />
  <sub>v0.5.5 — desktop clients over ACP, context that fits, fewer prompts</sub>
</p>

<p align="center">
  <a href="#installation">Install</a> &middot;
  <a href="#whats-new-in-050">What's New</a> &middot;
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

## What's New in 0.5.5

- **Desktop clients over ACP** — `nca acp` serves the [Agent Client Protocol](https://agentclientprotocol.com) on stdin/stdout, so T3 Code, Termul, Zed, and other ACP clients can run nca as a local agent. Streamed replies, tool cards, approvals, plans, modes, a model picker, slash commands, and session list/load/delete with history replay. See [Desktop Clients](docs/documentation/acp.md).
- **Context that always fits** — every provider request is fitted to the model window. Old groups are evicted oldest first (plain ones before ones that hold constraints, failures, or edits), tool groups stay whole, and the system prompt, the first task, and the whole current turn are kept. Tool-call arguments now count toward the estimate, so sessions that write large files no longer undercount.
- **Prompts only for dangerous actions** — in the default and accept-edits modes, ordinary shell commands run without asking. nca still asks before deleting, `sudo`, `git push`/`reset --hard`/`clean`, `curl | sh`, killing processes, publishing or global installs, writes to system or dotfile paths, and SQL `DROP`. The prompt names the reason. See [Permissions](docs/documentation/permissions.md).
- **`show_image`** — the agent can show a picture in the chat. ACP clients get an image block; T3 Code gets a Markdown image.
- **Provider model picker** — ACP clients list the active provider's own chat models (from its `/v1/models` catalog, cached for a day) instead of every alias.
- **Clearer output-limit errors** — a tool call cut off by `max_tokens` now says so and tells the model to write in smaller pieces.
- **Fixes** — a context-shrink loop that could hang, a `0` context window clamping every request to 1,024 tokens, image parts lost when shrinking, "Reject for this session" in ACP asking again, and screenshot `stop` reporting the wrong length.

## What's New in 0.5.0

- **Read before edit** — `edit_file`, `apply_patch`, and overwriting `write_file` need a `read_file` of an existing file earlier in the session, and the file must be unchanged since. New files are not affected.
- **Paged reads, capped output** — `read_file` takes `start_line` and `end_line` and returns 2,000 lines per call by default. Tool output is capped at 30,000 characters.
- **Resilient provider calls** — rate limits, 5xx errors, and dropped connections retry with backoff, honoring `Retry-After`. Stream errors are reported instead of shown as answers, and stalled connections time out.
- **Loop control** — a repeated identical tool call gets a nudge, then stops the turn. A turn that runs out of budget ends with a summary. Edits get one reminder to run `run_validation`.
- **Esc interrupts a pending request** — cancel works while the provider has not answered yet.
- **Windows input fixed** — key release events no longer double every keystroke.
- **`/rename`** — name a session. `nca sessions` shows the name.
- **`[ui] mouse_capture`** — set it to `false` to select text with the terminal. PageUp and PageDown scroll the transcript.
- **Leaner dependencies** — the unused `genai` dependency is gone, so the build has one `reqwest` version.
- **Desktop tools on macOS (preview)** — `app_input` drives a background app (keys, text, links) and `screen_record` records or screenshots one window without bringing it to the front. See [Desktop Tools (macOS)](#desktop-tools-macos).
- **History and recordings in the data folder** — command history and screen recordings live under `~/.local/share/ncacli/workspaces/<id>/`, not in the project. Set `[recording] dir` to store recordings somewhere else.
- **Faster startup** — context-window lookups are cached for 24 hours, so after the first lookup a run no longer waits on the provider's model list.
- **Remote MCP servers over HTTP** — set `url` on a `[[mcp.servers]]` entry to use a server that speaks streamable HTTP, and keep its token in the environment variable named by `bearer_token_env`. See [Remote MCP servers](docs/documentation/advanced.md#remote-mcp-servers).

## What It Does

- Runs coding tasks in an interactive TUI or a line-oriented REPL.
- Keeps slash commands, the command palette, and `/help` in sync via one registry (palette Enter runs the command).
- Supports one-shot runs and detached background sessions.
- Persists session state and event logs in the product home, one folder per workspace.
- Exposes machine-readable JSON and NDJSON for automation.
- Spawns child agents with explicit parent/child lineage and optional git worktrees.
- Uses MiniMax by default, with OpenAI, Anthropic, OpenRouter, and custom endpoint support.
- Loads built-in tools plus optional MCP tools from config, over stdio or HTTP.
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

Windows: download the matching `.zip` from [Releases](https://github.com/madebyaris/native-cli-ai/releases) and put `nca.exe` on your `PATH`.

### GitHub Releases

Pre-built binaries for every release are available on the [Releases](https://github.com/madebyaris/native-cli-ai/releases) page:

| Platform | Target | Archive |
|---|---|---|
| macOS (Apple Silicon) | `aarch64-apple-darwin` | `.tar.gz` |
| macOS (Intel) | `x86_64-apple-darwin` | `.tar.gz` |
| Linux (x86_64) | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Linux (ARM64) | `aarch64-unknown-linux-gnu` | `.tar.gz` |
| Windows (x86_64) | `x86_64-pc-windows-msvc` | `.zip` |

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
| `nca skills` | List discovered skills with their source (`AGENTS.md`, filesystem, or user directory). |
| `nca mcp` | List configured MCP servers. |
| `nca completion <shell>` | Generate shell completions. |
| `nca acp` | Serve the Agent Client Protocol on stdin/stdout for T3 Code, Termul, Zed, and other desktop clients. |
| `nca index build\|show` | Build or inspect a cached CLI index under `~/.local/share/ncacli/workspaces/<workspace-id>/`. |
| `nca autoresearch once <program.md>` | Run a metric-driven research program and print parsed output. |

There is also a hidden `serve` subcommand used for IPC-oriented service sessions.

## Interactive UX

The interactive surface has two modes:

- **Full-screen TUI** — transcript (markdown + syntax highlighting), composer, overlays (palette, pickers, connect wizard), approvals, structured questions, session sidebar, and branch chip.
- **Line-oriented REPL** (`reedline`) — for scripts, non-TTY environments, or `--no-tui`.

Slash commands, the Ctrl+P command palette, autocomplete, and `/help` all come from one registry, so labels stay in sync. Palette **Enter executes** the command (it does not only pre-fill the composer).

### Slash commands

| Command | Purpose |
|---|---|
| `/help` | Show help (generated from the registry). |
| `/agent` | Choose agent profile (`build`, `plan`, `review`, `fix`, `test`). |
| `/plan` `/review` `/fix` `/test` | Run a preset turn for that profile. |
| `/skills` | Browse discovered skills; `/{skill}` runs one. |
| `/memory` | Show or append memory notes. |
| `/compact` | Compact session context. |
| `/copy` | Copy the latest assistant response (TUI; also `Ctrl+Shift+C`). |
| `/todos` | Show the session todo list. |
| `/model` | Open the model picker (`/models` is an alias). |
| `/connect` | Connect / switch provider, API key, or custom endpoint (`/provider`, `/apikey`, `/custom` are aliases). |
| `/status` | Session status and health (`/stats`, `/cost`, `/doctor` are aliases). |
| `/config` | Config and editor settings (`/settings`, `/set-editor` are aliases). |
| `/permissions` | Permission mode picker (`/permission-bypass` is an alias). |
| `/sessions` `/new` `/export` `/attach` `/logs` | Session lifecycle and inspection. |
| `/image` | Stage clipboard or file images (TUI). |
| `/editor` | Open the external editor. |
| `/mcp` `/agents` `/diff` `/thinking` `/stop` `/clear` `/exit` | System and turn controls. |
| `/auto-answer` | Accept the suggested answer for a pending `ask_question`. |

### Keyboard shortcuts (TUI)

| Binding | Action |
|---|---|
| `Ctrl+P` | Open the command palette (fuzzy; Enter runs). |
| `Ctrl+V` | Paste image from clipboard (TUI). |
| `Ctrl+Shift+C` | Copy last assistant response (TUI). |
| `Ctrl+X` then `m` / `e` / `l` / `n` / `c` / `s` / `a` / `h` / `q` | Leader shortcuts: model, editor, sessions, new, compact, status, agent, help, exit. |
| `Ctrl+Y` / `Ctrl+N` / `Ctrl+U` | Approve / deny / always-allow a pending tool (wins over an open question modal). |
| `Tab` | Complete `@` path or `/` command; otherwise cycle agent profile. |
| `F2` / `Shift+F2` | Cycle recent models. |
| `Ctrl+V` | Paste clipboard image (off the UI thread). |
| `Ctrl+L` | Clear the transcript (same idea as `/clear`). |
| `Esc` / `Ctrl+C` | Cancel the current turn when busy. |
| `! <cmd>` | Run a shell command. |
| `@ <path>` | File mention with fuzzy completion (indexes in the background on open). |

Click the branch chip in the status line to open the branch picker (list/switch/create via background git).

While the agent is working, the status bar shows an animated busy state with elapsed seconds (`thinking 12s`, `tool 3s`) and a short detail (model wait, stream size, or file path). The transcript footer mirrors the same live activity so long provider waits do not look frozen.

![branch picker](docs/images/git-branch.png)

![interactive options](docs/images/option.png)

## Desktop Clients (ACP)

`nca acp` turns the same binary into an [Agent Client Protocol](https://agentclientprotocol.com) server, so desktop agents can drive nca as a local subprocess. No Node.js, no bridge, no second binary:

```
Command: nca
Args:    acp
```

Works with T3 Code, Termul, Zed, and any other ACP v1 client. The client gets streamed replies, tool cards with file locations, approval prompts, a plan from `update_todos`, a model picker, permission modes, session list/load/delete with history replay, slash commands (`/compact`, `/rename`, `/model`, `/mode`), compaction updates, and a context-usage meter. Sessions keep nca's large context window (MiniMax M2.5 resolves to 204,800 tokens), shrink themselves when they approach the limit, and idle sessions are evicted so the process stays small (about 5 MB footprint while streaming). See [Desktop Clients](docs/documentation/acp.md).

## Output and Automation

`nca` is designed to work well in two very different moods: terminal-first for humans, and machine-friendly for orchestrators.

- `--stream off` returns only the final output.
- `--stream human` renders the normal terminal experience.
- `--stream ndjson` emits newline-delimited event envelopes.
- `--json` is available on lifecycle-oriented commands such as `spawn`, `sessions`, `status`, `cancel`, `skills`, `models`, `doctor`, `config`, `index show`, and `mcp`.
- `NCA_ORCH_*` and `NCA_ORCH_META_*` environment variables attach orchestration metadata to sessions and harness context.

See [Orchestration Contract](docs/orchestration.md) for the subprocess-facing surface.

## Storage and Paths

`nca` keeps project instructions and git worktrees in the workspace, and stores session/memory/cache data under a unified product home (`$NCA_HOME`, `$XDG_DATA_HOME/ncacli`, or `~/.local/share/ncacli/`).

| Path | Purpose |
|---|---|
| `~/.local/share/ncacli/config.toml` | Global config file (legacy `~/.nca/config.toml` is still read if present). |
| `<workspace>/.nca/config.local.toml` | Workspace-local config overrides. |
| `~/.local/share/ncacli/workspaces/<id>/sessions/<sid>.json` | Saved session state. |
| `~/.local/share/ncacli/workspaces/<id>/sessions/<sid>.events.jsonl` | Event log for the session. |
| `~/.local/share/ncacli/workspaces/<id>/memory.json` | Default memory store. |
| `~/.local/share/ncacli/workspaces/<id>/last_session` | Auto-resume pointer. |
| `<workspace>/AGENTS.md` | Repo-local instruction layer; each `## Heading` is also a discoverable skill. |
| `<workspace>/.nca/skills/` | Default workspace skill directory. |
| `~/.local/share/ncacli/skills/` | User-level skill directory (legacy `~/.nca/skills/` still discovered). |
| `~/.claude/skills/` | Imported Claude-style skill directory, if present. |
| `<repo>/.nca/worktrees/<session-id>` | Worktree path for isolated child sessions. |
| `$XDG_RUNTIME_DIR/nca/<session_id>.sock` | IPC socket path when `XDG_RUNTIME_DIR` is set. |
| `/tmp/nca/<session_id>.sock` | IPC socket fallback when `XDG_RUNTIME_DIR` is not set. |
| `~/.local/share/ncacli/workspaces/<id>/cli-index.json` | Cached CLI index for agents and tooling. |
| `~/.local/share/ncacli/workspaces/<id>/history` | Command history for the REPL and TUI. An older `<workspace>/.nca/.history` is copied here once and left in place. |
| `~/.local/share/ncacli/workspaces/<id>/recordings/` | Screen recordings and screenshots from `screen_record` (macOS). Set `[recording] dir` to store them elsewhere. |
| `~/.local/share/ncacli/cache/model-limits.json` | Context-window sizes from the provider's model list, cached for 24 hours. |
| `.ncarc` | Project instructions file committed with the repo. |
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
- **Smart compaction** (opt-in) builds a deterministic provider-request view that truncates older read/search noise while keeping tool groups atomic. Canonical session history is never rewritten by this path.

Configuration in `~/.local/share/ncacli/config.toml`:

```toml
[memory.context]
context_window_target = 0          # 0 = auto-detect
max_retained_messages = 50
auto_summarize_threshold = 75
enable_auto_summarize = true
smart_compaction_mode = "on"       # off | dry_run | on
```

The default is `on`: every provider request is fitted to the model window, so a long session shrinks instead of overflowing. Use `dry_run` to inspect savings under `/status`, or `off` to always send the full history.

## Providers

MiniMax is the default provider path. The codebase also supports OpenAI, Anthropic, and OpenRouter, so the project can stay MiniMax-first without boxing itself into one provider forever.

Typical environment variables:

- `MINIMAX_API_KEY`
- `OPENAI_API_KEY`
- `ANTHROPIC_API_KEY`
- `OPENROUTER_API_KEY`
- `CUSTOM_PROVIDER_API_KEY` (for custom endpoints)

### Custom Endpoints

Use `/connect` in the TUI (or the command palette) to pick a provider, enter an API key, and optionally add a custom OpenAI-compatible or Anthropic-compatible endpoint. Legacy aliases `/provider`, `/apikey`, and `/custom` still resolve to the same flow.

```
/connect
```

See [Providers](docs/documentation/providers.md) for full details.

Provider config is loaded from defaults, then `~/.local/share/ncacli/config.toml`, then `<workspace>/.nca/config.local.toml`, then environment overrides.

Use `nca doctor` to verify provider readiness and `nca models` to inspect model selection.

## Harness and Tooling

The system prompt is layered in this order:

1. Built-in harness prompt
2. Permission-mode guidance
3. Project guidance from `AGENTS.md` (full file as instructions)
4. Project instructions from `.ncarc`
5. Local instructions from `.nca/instructions.md`
6. Discovered skills summary
7. Orchestration context

The built-in tool surface includes filesystem editing, search, diffing, patching, shell execution, web access, `ask_question`, and `spawn_subagent`. On macOS it also includes `app_input` and `screen_record`. MCP tools are loaded dynamically when configured, so the available tool set can grow with your environment.

### Search And Edit Tools

Recent search/edit improvements are aimed at making agent file work less brittle:

- `search_code` now returns structured JSON match objects instead of raw `rg` text.
- `search_code` treats ripgrep exit code `1` as a successful empty result, not a failure.
- `search_code` supports `path`, `glob`, `fixed_strings`, `case_sensitive`, `word`, `context_before`, `context_after`, and `max_results`.
- `query_symbols` is a literal Rust symbol lookup, not an implicit regex expansion of user input.
- `edit_file` and `apply_patch` now fail loudly on ambiguous single-match edits instead of silently changing the first occurrence.
- `replace_match` can edit a specific search result by exact `path`, `line`, and `column`, which makes search -> edit flows much safer.

### Desktop Tools (macOS)

Two tools let the agent work with Mac apps in the background. Both are macOS-only; there is no Windows or Linux equivalent yet.

- **`app_input`** — `list` shows an app's controls through Accessibility, `press` presses a control by its title, `type_text` sends keystrokes to a background app, and `open_url` opens an `http(s)` link in an app without taking focus.
- **`screen_record`** — `start` records one window for up to `seconds` (1–600, default 60), `stop` ends a recording early, and `screenshot` saves a PNG of one window after an optional delay (0–30 s, default 3).

Both ask for approval by default (except `app_input` `list`, which only reads an app's UI) and are denied in Plan and DontAsk modes. `app_input` needs Accessibility permission and `screen_record` needs Screen Recording permission, both granted to the terminal app. Calculator and Dia are the apps tested so far; other apps are unverified. Parameters are in [Tools](docs/documentation/tools.md).

## Crate Layout

| Crate | Responsibility |
|---|---|
| `crates/common` | Shared config, events, sessions, messages, tool schemas, and orchestration metadata. |
| `crates/core` | Agent loop, provider abstraction, harness builder, skills, approvals, and tool registry. |
| `crates/runtime` | Session supervision, IPC, persistence, worktrees, memory store, context management, and subagent execution. |
| `crates/tui` | Full-screen TUI, line REPL, slash-command registry, overlays, transcript rendering, and session UI state. |
| `crates/acp` | Agent Client Protocol server: JSON-RPC transport, session registry, event translation, and approvals for desktop clients. |
| `crates/cli` | `nca` binary entrypoint, clap commands, stream rendering, and glue onto `nca-tui` / runtime. |
| `crates/autoresearch` | Metric-driven autonomous research helpers and experiment runner. |

The shipped app is still a **single binary** (`nca`). The TUI crate owns interactive presentation; the CLI crate owns argument parsing and lifecycle commands.

## Session Model

- Sessions are persisted as JSON snapshots plus JSONL event logs.
- The runtime uses a `Supervisor` to own lifecycle, IPC, approvals, questions, event fanout, and persistence.
- Child sessions can inherit parent context, record lineage in session metadata, and run inside separate git worktrees.
- IPC uses newline-delimited JSON over Unix sockets so `attach`, approvals, status, and other controls share one runtime transport.
- `ContextManager` tracks token usage and auto-summarizes long conversations.

In practice, that means you can start small, branch out when a task gets bigger, and still keep a clean trail of what happened.

## Speed and Memory

Measured on one Mac (Darwin 27). Each CLI answered the same short prompt, "Reply with the word ok." nca used kenari.id with `minimax-m3-1-flash`; the other CLIs used their own default models. These numbers come from the build before 0.5.0 landed and have not been re-measured on 0.5.0 yet.

<p align="center">
  <img src="docs/images/nca-readme-power.svg" alt="RAM footprint per CLI: nca 6.4 MB, Grok CLI 59 MB, Claude Code 162 MB, Cursor CLI not sampled" width="720" />
</p>

| CLI | Wall | CPU | RAM (macOS footprint) | Notes |
|---|---|---|---|---|
| **nca** (release build, cached) | 3.6 s | 0.033 s | 6.4 MB | First event 14–20 ms after launch |
| Claude Code (`claude -p`) | 3.1 s | 0.80 s | 162 MB | |
| Grok CLI (`grok -p`) | 6.6 s | 1.77 s | 59 MB | |
| Cursor CLI (`cursor-agent -p`) | exits in 0.36 s | 0.39 s | not sampled | Not logged in, so startup only |

Wall and CPU are the median of three runs. RAM is one footprint sample per CLI, taken 1.5–3 s into the run. Codex CLI is not included because it is not installed on the test machine.

- Wall time mostly reflects each provider's model, so it is not a fair comparison. CPU and RAM are the fair client-side numbers.
- The footprint is an estimate from one sample, not a peak.
- Before the startup cache, each nca run waited 2.5–4.3 s on the provider's model list.

Full method and numbers: [CLI speed and footprint](docs/plans/cli-speed-and-footprint.md).

## Documentation

Full user-facing documentation lives in [`docs/documentation/`](docs/documentation/index.md):

| Page | Description |
|---|---|
| [Getting Started](docs/documentation/getting-started.md) | Installation, first run, and initial configuration |
| [Commands](docs/documentation/commands.md) | Complete CLI command and flag reference |
| [Interactive Mode](docs/documentation/interactive-mode.md) | TUI, REPL, slash commands, keyboard shortcuts |
| [Configuration](docs/documentation/configuration.md) | Config files, TOML format, and environment variables |
| [Providers](docs/documentation/providers.md) | LLM provider setup — MiniMax, Anthropic, OpenAI, OpenRouter, Custom |
| [Tools](docs/documentation/tools.md) | All agent tools — file ops, search, shell, web, and more |
| [Sessions](docs/documentation/sessions.md) | Session lifecycle, persistence, resume, and management |
| [Permissions](docs/documentation/permissions.md) | Approval system, permission modes, and safe mode |
| [Skills](docs/documentation/skills.md) | Skill discovery, installation, and authoring |
| [Advanced](docs/documentation/advanced.md) | Sub-agents, MCP servers, hooks, orchestration, and IPC |
| [Desktop Clients (ACP)](docs/documentation/acp.md) | Run nca from T3 Code, Termul, Zed, and other ACP clients |

Internal design docs:

- [Product Requirements](docs/prd.md)
- [Tech Stack](docs/tech-stack.md)
- [Architecture](docs/architecture.md)
- [Orchestration Contract](docs/orchestration.md)
- [Context Management](docs/context-management.md)
- [Performance Optimization Research](docs/research/rust-ratatui-optimization.md)
- [CLI speed and footprint](docs/plans/cli-speed-and-footprint.md)
- [macOS background input](docs/plans/macos-background-input.md)
- [macOS window recording](docs/plans/macos-window-recording.md)

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
