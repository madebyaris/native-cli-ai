# Interactive Mode

nca provides a rich interactive experience with a full-screen TUI (Terminal User Interface) and a fallback line-oriented REPL.

## TUI vs REPL

| Mode | When Used | Features |
|------|-----------|----------|
| **TUI** (default) | Terminal is a TTY, `--stream human`, no `--no-tui` | Full-screen, scrollable output, command palette, modals, mouse support |
| **Line REPL** | `--no-tui` flag, or non-TTY stdin/stdout | Simple line-by-line input/output, still supports slash commands |

Force line REPL mode:

```bash
nca --no-tui
```

## Input Modes

### Regular Text

Type your message and press **Enter** to send it to the agent.

### Shell Commands (`!`)

Prefix with `!` to run a shell command directly. The output is captured and fed into the conversation context.

```
! cargo test
! git log --oneline -5
! ls -la src/
```

### File Mentions (`@`)

Type `@` followed by a path to inline-reference a file. nca performs fuzzy file search and auto-completion.

```
Can you review @src/main.rs and @src/lib.rs?
```

In the TUI, pressing `@` opens a file picker with fuzzy search. Use `Tab` to navigate matches and `Enter` to select.

### Multiline Input (`\`)

End a line with `\` to continue input on the next line:

```
Write a function that \
takes a vector of strings \
and returns the longest one.
```

### Slash Commands (`/`)

Type `/` to access slash commands. In the TUI, this opens an inline autocomplete menu.

---

## Slash Commands

### General

| Command | Description |
|---------|-------------|
| `/help` | Show help with all commands and keyboard shortcuts |
| `/status` | Display session status (ID, model, agent profile, permission mode) |
| `/clear` | Clear the screen |
| `/exit`, `/quit`, `/q` | Exit the session |
| `/new` | Start a new session |
| `/export` | Export the current session to markdown |
| `/stop` | Cancel the current agent turn |

### Agent Profiles

| Command | Description |
|---------|-------------|
| `/agent [profile]` | Show or switch agent profile |
| `/plan <task>` | Run a planning-oriented turn (read-only analysis) |
| `/review <task>` | Run a code review turn |
| `/fix <task>` | Run a bug-fix turn |
| `/test <task>` | Run a validation/testing turn |

Available agent profiles:

| Profile | Description |
|---------|-------------|
| `@build` | Default full-access agent for development work |
| `@plan` | Read-only agent for analysis and planning |
| `@review` | Focused code review agent |
| `@fix` | Bug diagnosis and fix agent |
| `@test` | Testing and validation agent |

### Model and Provider

| Command | Description |
|---------|-------------|
| `/models` | Browse and select models (opens picker in TUI) |
| `/model [name]` | Set the active model for the session |
| `/connect` | Open the provider connection picker |
| `/provider [name]` | Show or set the default LLM provider |
| `/apikey <provider> <key>` | Store an API key for a provider |

### Session and Context

| Command | Description |
|---------|-------------|
| `/compact` | Compact session context (summarize and trim history) |
| `/thinking` | Toggle thinking/reasoning visibility |
| `/sessions` | List and switch between sessions |
| `/agents` | List child sub-agent sessions |
| `/logs` | View session event log |
| `/attach` | Attach to a session |
| `/diff` | Show recent file changes |
| `/cost` | Show token usage and costs |
| `/stats` | Show session statistics |

### Tools and Configuration

| Command | Description |
|---------|-------------|
| `/skills` | List discovered skills |
| `/memory [text]` | Show memory notes, or add a note |
| `/mcp` | List MCP servers |
| `/permissions [mode]` | Show or set permission mode |
| `/permission-bypass` | Toggle permission bypass |
| `/config` | Show runtime configuration |
| `/doctor` | Run configuration diagnostics |
| `/settings` | Show settings |

### Editor

| Command | Description |
|---------|-------------|
| `/editor [seed]` | Open an external editor to compose a message |
| `/set-editor <cmd>` | Persist the editor command (e.g., `vim`, `code --wait`) |

### Images

| Command | Description |
|---------|-------------|
| `/image` | Manage staged image attachments |

### Other

| Command | Description |
|---------|-------------|
| `/undo` | Undo last file change |
| `/redo` | Redo last undone change |
| `/auto-answer` | Auto-answer agent questions with suggested answer |

---

## Keyboard Shortcuts

### General Navigation

| Shortcut | Action |
|----------|--------|
| `Enter` | Send message |
| `Esc` | Cancel current agent turn / close modal |
| `Ctrl+C` | Cancel request |
| `Ctrl+L` | Clear screen |
| `Ctrl+Q` | Exit |
| `Mouse wheel` | Scroll output |
| `End` | Jump to bottom of transcript (on empty input) |

### Agent and Model

| Shortcut | Action |
|----------|--------|
| `Tab` | Cycle agent profile (build → plan → review → fix → test) |
| `F2` | Cycle through recent models (forward) |
| `Shift+F2` | Cycle through recent models (backward) |

### Command Palette and Pickers

| Shortcut | Action |
|----------|--------|
| `Ctrl+P` | Open command palette |
| `Ctrl+V` | Paste image from clipboard (TUI only) |
| `Ctrl+X M` | Switch model (model picker) |
| `Ctrl+X E` | Open external editor |
| `Ctrl+X L` | Switch session |
| `Ctrl+X N` | New session |
| `Ctrl+X C` | Compact context |
| `Ctrl+X S` | View status |
| `Ctrl+X A` | Agent profile picker |
| `Ctrl+X H` | Show help |
| `Ctrl+X Q` | Exit |

### Within Modals and Pickers

| Shortcut | Action |
|----------|--------|
| `↑` / `↓` | Navigate options |
| `Enter` | Select / confirm |
| `Esc` | Close modal |
| `j` / `k` | Navigate (in info modals) |

### Agent Question Modals

When the agent asks a structured question:

| Shortcut | Action |
|----------|--------|
| `↑` / `↓` | Select an option |
| `Enter` | Confirm selection (or accept suggested answer on empty input) |
| `0` | Accept suggested answer |
| `1`–`n` | Select option by number |
| `c` | Type a custom answer |

---

## Command Palette

Press `Ctrl+P` to open the command palette — a searchable list of all available commands. Type to filter, use `↑`/`↓` to navigate, and `Enter` to execute.

## Status Bar

The TUI displays a status bar at the bottom showing:

- Current agent profile
- Active model
- Available shortcuts hint

```
Tab  agent   Ctrl+V  image   Ctrl+P  commands   !cmd  shell   @path  search   /  inline   wheel  scroll
```

## Image Attachments

In the TUI, paste images from your clipboard with `Ctrl+V` or use the `/image` command. Images are processed through MiniMax native vision and the text description is injected into the conversation.

## Vi Mode

If your `NCA_EDITOR_MODE` environment variable is set to `vi` or `vim`, the REPL uses vi keybindings for line editing.

```bash
export NCA_EDITOR_MODE=vi
```

## External Editor

For composing long messages, use `/editor` to open your configured external editor. The content is sent as your message when you save and close.

Editor resolution order:
1. `NCA_EDITOR` environment variable
2. `[ui].editor` in config
3. `EDITOR` environment variable
4. `vim` (fallback)
