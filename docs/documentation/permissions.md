# Permissions

nca uses a tiered permission system to control which actions the agent can take. This ensures safety while allowing flexibility for different workflows.

## Permission Modes

Set the permission mode via CLI flag, config file, or interactive command.

| Mode | Description |
|------|-------------|
| `default` | Same as `accept-edits`: reads, file edits, and safe shell commands run; dangerous actions prompt |
| `plan` | Read-only tools only; all writes and execution denied |
| `accept-edits` | Reads, file edits, and safe shell commands run without asking; dangerous actions prompt |
| `dont-ask` | Read-only tools only; no prompts (deny anything that would need asking) |
| `bypass-permissions` | All tools auto-allowed without prompts |

### Setting the Mode

```bash
# CLI flag
nca --permission-mode accept-edits

# Environment (via config)
# In config.toml:
# [permissions]
# mode = "accept-edits"

# Interactive slash command
/permissions accept-edits
```

## Tool Categories

Tools are classified into categories that determine how each permission mode handles them:

### Read-Only Tools (always allowed in most modes)

- `read_file`
- `list_directory`
- `search_code`
- `git_status`
- `git_diff`
- `query_symbols`
- `web_search`
- `fetch_url`
- `ask_question`
- `show_image`

### File-Edit Tools (allowed in `accept-edits` and above)

- `write_file`
- `create_directory`
- `apply_patch`
- `edit_file`
- `replace_match`
- `rename_path`
- `move_path`
- `copy_path`
- `spawn_subagent`

### Destructive Tools (always prompt in `accept-edits`)

- `delete_path` — always requires explicit approval unless in `bypass-permissions`

### Execution Tools (prompt only when dangerous in `default` and `accept-edits`)

- `execute_bash` — runs without asking unless the command looks dangerous. nca asks when a command:
  - deletes (`rm`, `rmdir`, `unlink`, `shred`, `find -delete`, `find -exec`)
  - escalates privileges (`sudo`, `su`, `doas`)
  - rewrites or publishes git history (`git push`, `reset --hard`, `clean`, `rebase`, `branch -D`, `stash drop`, `checkout --`, `restore --`)
  - pipes a download into an interpreter (`curl … | sh`)
  - kills processes or changes the system (`kill`, `pkill`, `shutdown`, `launchctl`, `dd`, `mkfs`, `diskutil`, `chmod -R`, `chown`, `defaults write`)
  - publishes or installs globally (`npm publish`, `npm -g`, `cargo publish`/`install`, `pip install`, `brew install`, `docker push`, `gh pr merge`, …)
  - writes into system or home dotfile paths (`> /etc/…`, `>> ~/.zshrc`, `tee /usr/…`)
  - runs SQL `DROP`/`TRUNCATE`/`DELETE FROM`

  The prompt names the reason, e.g. `Dangerous command (runs `rm`): rm old.txt`.
- `run_validation` — always allowed; it only runs allowlisted check commands.

### GUI and Capture Tools (macOS)

- `app_input` — `list` (reads an app's UI) is allowed; `press`, `type_text`, and `open_url` prompt.
- `screen_record` — prompts (recordings and screenshots can show private content).

## Permission Mode Behavior Matrix

| Tool Category | `default` | `plan` | `accept-edits` | `dont-ask` | `bypass-permissions` |
|---------------|-----------|--------|-----------------|------------|----------------------|
| Read-only | Allowed | Allowed | Allowed | Allowed | Allowed |
| File-edit | Allowed | Denied | Allowed | Denied | Allowed |
| Destructive (`delete_path`) | **Ask** | Denied | **Ask** | Denied | Allowed |
| Safe shell command | Allowed | Denied | Allowed | Denied | Allowed |
| Dangerous shell command | **Ask** | Denied | **Ask** | Denied | Allowed |
| GUI control / capture | **Ask** | Denied | **Ask** | Denied | Allowed |
| MCP tools | **Ask** | Denied | **Ask** | Denied | Allowed |

## Allow/Deny Lists

Fine-tune permissions with pattern-based lists in config:

```toml
[permissions]
mode = "default"

# Always allow these tool+pattern combinations
allow = [
    "execute_bash:cargo *",
    "execute_bash:git status",
    "write_file:src/*",
]

# Always deny these
deny = [
    "execute_bash:rm -rf *",
    "execute_bash:sudo *",
    "delete_path:*",
]

# Force ask for these (even if mode would auto-allow)
ask = [
    "write_file:Cargo.toml",
]
```

### Pattern Format

Patterns use the format `tool_name:description_pattern` with simple wildcard matching:

- `*` matches any substring
- Matching is case-insensitive
- The description is derived from the tool's input (file path, command, URL, etc.)

### Pattern Matching Examples

```toml
# Allow all cargo commands
allow = ["execute_bash:cargo *"]

# Allow writes only to src/ directory
allow = ["write_file:src/*"]

# Deny any rm commands
deny = ["execute_bash:rm *"]

# Always ask before modifying config files
ask = ["write_file:*.toml", "write_file:*.json"]
```

## Session-Level Approvals

When you approve a tool call interactively, nca offers to remember the approval pattern for the rest of the session. This is stored as a session-level allow pattern and does not persist across sessions.

For example, approving `execute_bash: cargo test` might add `execute_bash:cargo test` to the session allow list, so future `cargo test` calls proceed without prompting.

## Approval Flow

When a tool requires approval:

1. The agent emits an `ApprovalRequested` event
2. In the TUI, an approval modal appears showing the tool name and parameters
3. You can:
   - **Allow** — execute this tool call
   - **Allow pattern** — allow this and similar future calls
   - **Deny** — block this tool call
4. The agent receives the decision and continues

### Approval Timeout

In interactive mode, approval requests time out after **300 seconds** (5 minutes) and auto-deny.

## Safe Mode

Safe mode (`--safe` or `-s`) is a special restricted mode:

```bash
nca --safe
nca -s
```

In safe mode:
- Only read-only tools are registered
- `execute_bash` is explicitly added to the deny list
- `spawn_subagent` is not registered
- MCP tools are only available if `mcp.expose_in_safe_mode = true`

Safe mode is ideal for code exploration, review, and analysis without any risk of modification.

## Headless/CI Mode

For non-interactive usage (CI pipelines, scripts, automation):

```bash
nca run --prompt "..." --permission-mode bypass-permissions --json
```

In headless mode:
- If a tool requires approval and no interactive handler is available, it **fails loudly** instead of stalling
- Use `bypass-permissions` to skip all approval prompts
- Use `accept-edits` for a middle ground (file edits allowed, shell still denied)
- The `--json` flag outputs structured results for machine consumption

## Interactive Permission Changes

Change the permission mode during a session:

```
/permissions                    # Show current mode
/permissions accept-edits       # Switch mode
/permission-bypass              # Toggle bypass mode
```
