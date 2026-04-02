# Getting Started

## Prerequisites

- **Rust toolchain** — install via [rustup](https://rustup.rs/) (edition 2024, stable channel)
- **Git** — required for session worktrees and version control integration
- **ripgrep (`rg`)** — used by the `search_code` tool for fast code search
- **An LLM API key** — MiniMax (default), Anthropic, OpenAI, or OpenRouter

## Installation

### Build from Source

```bash
git clone https://github.com/madebyaris/native-cli-ai.git
cd native-cli-ai

# Build optimized release binary
cargo build --release

# Install to your PATH
cp target/release/nca /usr/local/bin/
```

The release profile is optimized for size and speed (`opt-level = 3`, `lto = "thin"`, `strip = true`).

### Verify Installation

```bash
nca doctor
```

This runs configuration checks and reports any issues with your setup.

## Initial Setup

### 1. Set an API Key

The fastest way to get started is to export your API key:

```bash
# MiniMax (default provider)
export MINIMAX_API_KEY="your-key-here"

# Or use another provider
export ANTHROPIC_API_KEY="your-key-here"
export OPENAI_API_KEY="your-key-here"
export OPENROUTER_API_KEY="your-key-here"
```

To persist the key, add it to your shell profile (`~/.zshrc`, `~/.bashrc`) or store it in the config file:

```toml
# ~/.nca/config.toml
[provider.minimax]
api_key = "your-key-here"
```

### 2. First Run

Navigate to any project directory and launch nca:

```bash
cd ~/my-project
nca
```

On first launch, nca runs an **onboarding flow** that helps you:
- Select your default LLM provider
- Enter your API key
- Confirm basic settings

After onboarding, you enter the interactive TUI where you can start chatting with the agent.

### 3. One-Shot Mode

For quick tasks without entering the interactive session:

```bash
nca -p "explain the architecture of this project"
nca -p "add a health check endpoint to the API"
```

## Directory Structure

nca creates a `.nca/` directory in your workspace for local state:

```
my-project/
├── .nca/
│   ├── config.local.toml    # Workspace-specific config overrides
│   ├── instructions.md      # Local instructions for the agent
│   ├── .last_session         # Pointer to the most recent session
│   ├── memory.json           # Persistent memory notes
│   ├── sessions/             # Session state and event logs
│   │   ├── <id>.json         # Session state snapshot
│   │   └── <id>.events.jsonl # Event log (NDJSON)
│   ├── worktrees/            # Git worktrees for sub-agents
│   └── skills/               # Local skill definitions
├── .ncarc                    # Project-level instructions
├── AGENTS.md                 # Agent instructions (also used by other AI tools)
└── ...
```

Global config lives at `~/.nca/config.toml`.

## Custom Instructions

You can guide the agent's behavior with instruction files, loaded in this order:

1. **Built-in system prompt** — nca's default behavior rules
2. **`AGENTS.md`** — project-level instructions (compatible with other AI tools)
3. **`.ncarc`** — project-level nca-specific instructions
4. **`.nca/instructions.md`** — personal local instructions (gitignored)

Example `.ncarc`:

```markdown
## Project Context

This is a REST API built with Axum. Use PostgreSQL for data storage.
Always run `cargo test` after making changes.
Prefer the `anyhow` crate for error handling.
```

## Shell Completions

Generate shell completions for your shell:

```bash
# Bash
nca completion bash > /etc/bash_completion.d/nca

# Zsh
nca completion zsh > ~/.zsh/completions/_nca

# Fish
nca completion fish > ~/.config/fish/completions/nca.fish
```

## Next Steps

- [Commands](./commands.md) — full CLI reference
- [Interactive Mode](./interactive-mode.md) — TUI features, slash commands, shortcuts
- [Configuration](./configuration.md) — detailed config options
- [Providers](./providers.md) — set up LLM providers
