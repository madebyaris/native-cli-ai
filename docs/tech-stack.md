# Tech Stack

This document records every dependency choice, the rationale behind it, and the alternatives that were evaluated and rejected.

---

## Runtime and Async

| Crate | Version | Role |
|-------|---------|------|
| `tokio` | 1.x | Async runtime for all I/O: network, filesystem, PTY, IPC |

**Why tokio**: De-facto standard, best ecosystem support (reqwest, tonic, tower all assume tokio). No realistic alternative for a project this broad.

**Rejected**: `async-std` (smaller ecosystem, fewer integrations), `smol` (too minimal for our needs).

---

## CLI and Argument Parsing

| Crate | Version | Role |
|-------|---------|------|
| `clap` | 4.x | CLI argument parsing with derive macros |

**Why clap**: Mature, well-documented, supports subcommands, env vars, config layering, and shell completions out of the box.

**Rejected**: `argh` (simpler but less flexible), `structopt` (merged into clap 3+).

---

## Terminal UI

| Crate | Version | Role |
|-------|---------|------|
| `ratatui` | 0.30.x | Widget-based terminal UI framework |
| `crossterm` | 0.29.x | Cross-platform terminal backend (events, raw mode, colors) |
| `reedline` | 0.44.x | Line editor with history, completions, and hints |

**Why ratatui + crossterm**: ratatui is the most actively maintained TUI framework in Rust (19M+ downloads). crossterm is its default backend and works on Windows, macOS, and Linux without external dependencies.

**Why reedline**: Provides a nushell-quality line editor with multi-line input, syntax highlighting hooks, history search, and tab completion. Better UX than raw crossterm input handling.

**Rejected**: `tui-rs` (unmaintained, ratatui is its successor), `termion` (Linux-only), `cursive` (higher-level but less flexible for custom layouts).

---

## Terminal Rendering

| Crate | Version | Role |
|-------|---------|------|
| `syntect` | 5.x | Syntax highlighting for code blocks |
| `pulldown-cmark` | 0.12.x | Markdown parsing |
| `colored` | 3.x | Simple ANSI color helpers for non-TUI output |

---

## LLM Provider Abstraction

| Crate | Version | Role |
|-------|---------|------|
| `genai` | 0.5.x | Multi-provider abstraction (Anthropic, OpenAI, Gemini, Ollama, etc.) |
| `anthropic-async` | 0.4.x | Direct Anthropic client for advanced features (caching, thinking, beta APIs) |
| `async-openai` | 0.33.x | Direct OpenAI client for Codex/GPT features |

**Strategy**: MiniMax is the first-class provider and is implemented directly over `reqwest` so we control auth, base URL, model naming, and tool-call normalization without waiting on a generic wrapper. `genai` remains useful later for broad multi-provider support, while `anthropic-async` and `async-openai` stay as direct adapters for providers that need special handling.

**Current default**: `MiniMax-M2.5` via `provider.minimax` config and `MINIMAX_API_KEY`.

**Rejected**: `llm` crate (focused on local models, not API providers), rolling our own HTTP client (unnecessary duplication).

---

## MCP (Model Context Protocol)

| Crate | Version | Role |
|-------|---------|------|
| `mcpr` | 0.2.x | MCP client and server, stdio/SSE/WebSocket transports |

**Why mcpr**: Only serious Rust MCP implementation with complete schema definitions and multiple transports. Aligns with Anthropic's protocol spec.

**Deferred**: MCP is Phase 3. The crate is included in the workspace but not wired into the agent loop until after MVP.

---

## HTTP

| Crate | Version | Role |
|-------|---------|------|
| `reqwest` | 0.13.x | HTTP client for direct API calls and fallback |

**Why reqwest**: Tokio-native, supports streaming responses, TLS, and connection pooling. Used by both `anthropic-async` and `async-openai` internally.

---

## PTY and Process Management

| Crate | Version | Role |
|-------|---------|------|
| `portable-pty` | 0.9.x | Cross-platform pseudo-terminal interface |
| `tokio-process` | (via tokio) | Async child process spawning |

**Why portable-pty**: Part of the WezTerm project, battle-tested across platforms. Provides the PTY abstraction needed for sandboxed bash execution and terminal capture.

**Rejected**: Raw `std::process::Command` (no PTY support, can't capture terminal output properly), `pty` crate (unmaintained).

---

## Tmux Integration

| Crate | Version | Role |
|-------|---------|------|
| `tmux_interface` | 0.3.x | Programmatic tmux control via CLI |

**Why tmux_interface**: Most mature Rust tmux library (62K downloads), supports tmux 0.8 through 3.4.

**Deferred**: Phase 3. The adapter will wrap this crate behind a trait so we can support other multiplexers later.

**Rejected**: `tmux-lib` (newer but less proven), shelling out directly (fragile, hard to test).

---

## Serialization and Config

| Crate | Version | Role |
|-------|---------|------|
| `serde` | 1.x | Serialization framework |
| `serde_json` | 1.x | JSON for API payloads, session files, IPC messages |
| `toml` | 0.9.x | TOML for config files |

---

## Local Persistence

| Crate | Version | Role |
|-------|---------|------|
| `rusqlite` | 0.38.x | SQLite-backed orchestration store for companies, projects, todos, agents, and run links |

**Why SQLite + rusqlite**: The desktop now needs relational local data for company/project/todo/agent orchestration without introducing a server or a hosted database. SQLite keeps setup zero-admin on macOS/Linux, while `rusqlite` is the simplest direct Rust integration for a local embedded control-plane store.

**Hybrid model**: SQLite is the source of truth for orchestration entities and relationships. Session transcripts and event logs stay as JSON/JSONL artifacts in workspace-local `.nca/sessions/` folders.

**Rejected for now**: Postgres (too much infrastructure for a local-first desktop), monitor-only JSON files (poor relational querying), fully remote control-plane storage (premature for the native desktop path).

---

## Filesystem and Search

| Crate | Version | Role |
|-------|---------|------|
| `ignore` | 0.4.x | Gitignore-aware directory walking |
| `globset` | 0.4.x | Glob pattern matching for permission rules |
| `walkdir` | 2.x | Recursive directory traversal |

**Code search**: MVP shells out to `rg` (ripgrep) as an external tool, same as Claude Code. A future phase may embed `grep` crate or `tree-sitter` for structural search.

---

## Diff and Patching

| Crate | Version | Role |
|-------|---------|------|
| `similar` | 2.x | Unified diff generation |

**Rejected**: `diffy` (less actively maintained), `diff` (older API).

---

## Git

| Crate | Version | Role |
|-------|---------|------|
| `gix` | latest | Pure-Rust Git implementation |

**Deferred**: Phase 3. Used for branch detection, status, and commit operations.

**Rejected**: `git2` (libgit2 C binding -- works but adds a C dependency; `gix` keeps us pure Rust).

---

## Error Handling

| Crate | Version | Role |
|-------|---------|------|
| `thiserror` | 2.x | Derive macros for error types |
| `anyhow` | 1.x | Ergonomic error propagation in application code |

**Convention**: Library crates (`core`, `common`, `runtime`) use `thiserror` for typed errors. Application crates (`cli`, `monitor`) use `anyhow` for convenience.

---

## Logging and Tracing

| Crate | Version | Role |
|-------|---------|------|
| `tracing` | 0.1.x | Structured, async-aware instrumentation |
| `tracing-subscriber` | 0.3.x | Log formatting and filtering |

---

## Desktop Monitor (Phase 2)

| Crate | Version | Role |
|-------|---------|------|
| `egui` | 0.30.x | Immediate-mode GUI framework |
| `eframe` | 0.30.x | egui integration for native windowing |

**Why egui**: Pure Rust, no webview, no JS. GPU-accelerated via wgpu or glow. Excellent for dashboards, log viewers, and control panels. Fast iteration with hot-reload support.

**Rejected alternatives**:

| Framework | Reason for rejection |
|-----------|---------------------|
| **Tauri** | Requires a webview and JS/HTML frontend. Violates the native-first constraint. |
| **Dioxus Desktop** | Currently uses system webview under the hood (built on Wry/Tao). Same webview concern as Tauri for our purposes. |
| **GPUI** | Promising (powers Zed), but pre-1.0, macOS-focused, limited docs, and smaller community. Higher risk for a secondary deliverable. |
| **iced** | Pure Rust and native, but slower to build complex UIs with. Less widget variety than egui. |

---

## Testing

| Crate | Version | Role |
|-------|---------|------|
| `tempfile` | 3.x | Temporary directories for sandboxed test fixtures |
| `assert_cmd` | 2.x | CLI integration testing |
| `insta` | 1.x | Snapshot testing for rendered output |

---

## Summary: Dependency Tree by Crate

```
crates/common   -> serde, serde_json, toml, thiserror, tracing
crates/core     -> common, genai, anthropic-async, async-openai, reqwest,
                   serde_json, thiserror, tracing, tokio, similar, globset
crates/cli      -> common, core, runtime, clap, ratatui, crossterm, reedline,
                   syntect, pulldown-cmark, colored, anyhow, tracing, tokio
crates/runtime  -> common, core, portable-pty, tokio, serde_json, tracing,
                   thiserror, ignore, walkdir
crates/monitor  -> common, egui, eframe, tokio, serde_json, tracing, anyhow
```
