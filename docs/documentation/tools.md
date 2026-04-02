# Tools

nca's agent has access to a set of built-in tools for interacting with your codebase, running commands, and searching the web. Tools are the actions the agent can take on your behalf.

## Tool Categories

### Read-Only Tools

These tools are always available, including in [safe mode](./permissions.md).

| Tool | Description |
|------|-------------|
| `read_file` | Read the contents of a file |
| `search_code` | Search code using ripgrep with structured JSON output |
| `list_directory` | List files and directories at a path |
| `git_status` | Show `git status` for the workspace |
| `git_diff` | Show `git diff` (staged or unstaged) |
| `web_search` | Search the web via DuckDuckGo |
| `fetch_url` | Fetch and extract text content from a URL |

### Write Tools

Available in standard mode. Requires appropriate [permissions](./permissions.md).

| Tool | Description |
|------|-------------|
| `write_file` | Create or overwrite a file |
| `create_directory` | Create a directory (including parents) |
| `apply_patch` | Apply one or more exact string replacements to a file |
| `edit_file` | Replace a specific string in an existing file |
| `replace_match` | Precision replace using line and column coordinates |
| `rename_path` | Rename a file or directory |
| `move_path` | Move a file or directory |
| `copy_path` | Copy a file |
| `delete_path` | Delete a file or directory |

### Execution Tools

| Tool | Description |
|------|-------------|
| `execute_bash` | Execute a shell command in the workspace (PTY-backed) |
| `run_validation` | Run an allowlisted build/test/lint command |

### Intelligence Tools

| Tool | Description |
|------|-------------|
| `query_symbols` | Search for symbol definitions in the codebase |
| `ask_question` | Ask the user a structured question with options |
| `invoke_skill` | Load and follow a skill's instructions |
| `spawn_subagent` | Spawn a child agent session for parallel work |

### MCP Tools

Tools from configured [MCP servers](./advanced.md#mcp-servers) appear as `mcp__<server>__<tool>`.

---

## Tool Reference

### `read_file`

Read the contents of a file in the workspace.

**Parameters:**
- `path` (string, required) — File path relative to workspace root

**Behavior:** Reads the file asynchronously. The path must resolve within the workspace boundary.

---

### `search_code`

Search code using ripgrep and return structured match results.

**Parameters:**
- `pattern` (string, required) — Search pattern (regex by default)
- `path` (string, optional) — Directory to search in (default: workspace root)
- `glob` (string, optional) — File glob filter (e.g., `"*.rs"`)
- `fixed_strings` (bool, optional) — Treat pattern as literal text
- `case_sensitive` (bool, optional) — Case-sensitive matching
- `word` (bool, optional) — Match whole words only
- `context_before` (int, optional) — Lines of context before match
- `context_after` (int, optional) — Lines of context after match
- `max_results` (int, optional) — Maximum number of results

**Behavior:** Invokes `rg` with JSON output. Search root is validated to stay within the workspace.

---

### `list_directory`

List files and directories at a given path.

**Parameters:**
- `path` (string, optional) — Directory path (default: `.`, workspace root)

**Behavior:** Lists entries under the given path. Directories are suffixed with `/`.

---

### `git_status`

Show the current git status for the workspace.

**Parameters:** None (empty object `{}`)

**Behavior:** Runs `git status --short --branch` in the workspace.

---

### `git_diff`

Show git diff for the workspace.

**Parameters:**
- `staged` (bool, optional) — If true, show staged changes (`--cached`)

---

### `web_search`

Search the public web and return titles, URLs, and snippets.

**Parameters:**
- `query` (string, required) — Search query
- `limit` (int, optional) — Number of results (1–10, default from config)

**Behavior:** HTTP GET to DuckDuckGo HTML search. Results are parsed and returned as structured text.

---

### `fetch_url`

Fetch and normalize the text content of a URL.

**Parameters:**
- `url` (string, required) — The URL to fetch

**Behavior:** Makes an HTTP GET request, strips HTML to text content, and truncates to `max_fetch_chars` (default 25,000 characters).

---

### `write_file`

Create or overwrite a file inside the workspace.

**Parameters:**
- `path` (string, required) — File path relative to workspace
- `content` (string, required) — File contents

**Behavior:** Creates parent directories if needed. Path must resolve within workspace.

---

### `create_directory`

Create a directory inside the workspace.

**Parameters:**
- `path` (string, required) — Directory path

**Behavior:** Creates the directory and all parent directories (`mkdir -p` equivalent).

---

### `apply_patch`

Apply one or more exact string replacements to a file.

**Parameters:**
- `path` (string, required) — File to patch
- `edits` (array, required) — List of edits, each containing:
  - `old_text` (string, required) — Text to find (must not be empty)
  - `new_text` (string, required) — Replacement text
  - `replace_all` (bool, optional) — Replace all occurrences (default: false)

**Behavior:** For each edit, finds the exact `old_text` string. If `replace_all` is false and multiple matches exist, the edit fails with an error.

---

### `edit_file`

Replace a specific string in an existing file.

**Parameters:**
- `path` (string, required) — File to edit
- `old_text` (string, required) — Text to find
- `new_text` (string, required) — Replacement text
- `replace_all` (bool, optional) — Replace all occurrences

Similar to `apply_patch` but for a single edit.

---

### `replace_match`

Precision replacement using exact file path, line number, and column.

**Parameters:**
- `path` (string, required) — File path
- `line` (int, required) — Line number (1-based)
- `column` (int, required) — Column number (1-based)
- `old_text` (string, required) — Text to replace at the specified position
- `new_text` (string, required) — Replacement text

**Behavior:** Anchors the replacement at a specific line and column for maximum precision.

---

### `rename_path`

Rename a file or directory within the workspace.

**Parameters:**
- `from` (string, required) — Current path
- `to` (string, required) — New path

---

### `move_path`

Move a file or directory within the workspace.

**Parameters:**
- `from` (string, required) — Source path
- `to` (string, required) — Destination path

---

### `copy_path`

Copy a file within the workspace.

**Parameters:**
- `from` (string, required) — Source file
- `to` (string, required) — Destination file

---

### `delete_path`

Delete a file or directory.

**Parameters:**
- `path` (string, required) — Path to delete
- `recursive` (bool, optional) — Required for directory deletion

**Behavior:** Always requires explicit approval under most permission modes (classified as destructive).

---

### `execute_bash`

Execute a shell command in the workspace.

**Parameters:**
- `command` (string, required) — The shell command to run
- `timeout_secs` (int, optional, default: 30) — Command timeout in seconds

**Behavior:** Runs in a PTY (pseudo-terminal) for full interactive command support. Returns stdout content or status message. Exit code 0 = success.

In safe mode, `execute_bash` is added to the deny list automatically.

---

### `run_validation`

Run a safe build, test, or lint command.

**Parameters:**
- `command` (string, required) — The command to run
- `cwd` (string, optional, default: `.`) — Working directory
- `timeout_secs` (int, optional, default: 120) — Command timeout

**Behavior:** Only executes commands that start with an allowlisted prefix:

- `cargo build`, `cargo test`, `cargo check`, `cargo clippy`, `cargo fmt`
- `npm run`, `npm test`, `npx`
- `pnpm run`, `pnpm test`
- `pytest`, `python -m pytest`
- `go test`, `go build`, `go vet`
- `make`

Other commands are rejected with an error.

---

### `query_symbols`

Search for symbol definitions (functions, structs, traits, etc.) in the codebase.

**Parameters:**
- `query` (string, required) — Symbol name or pattern to search
- `glob` (string, optional) — File filter

**Behavior:** Uses fast local code intelligence to find symbol definitions. Returns `path:line:text` formatted results.

---

### `ask_question`

Ask the user a structured question with predefined options.

**Parameters:**
- `prompt` (string, required) — The question text
- `options` (array, required) — List of options with `id` and `label`
- `suggested_answer` (string, required) — Default/recommended answer
- `allow_custom` (bool, optional, default: true) — Allow freeform custom answer

**Behavior:** Opens a modal in the TUI (or prompts in REPL) and waits for the user's selection. Blocks up to 3600 seconds.

---

### `invoke_skill`

Load a skill's full instructions by name.

**Parameters:**
- `skill_name` (string, required) — Name of the skill to invoke

**Behavior:** Discovers the skill's `SKILL.md` file from configured skill directories and returns its expanded content. If no match, lists available skills.

---

### `spawn_subagent`

Spawn a child agent session for parallel task delegation.

**Parameters:**
- `task` (string, required) — Clear description of what the sub-agent should do
- `focus_files` (string[], optional) — File paths the sub-agent should focus on
- `use_worktree` (bool, optional, default: true) — Run in an isolated git worktree

**Behavior:** Creates a new child session that inherits conversation context. The child runs with `bypass-permissions` mode and no interactive approvals. Returns a JSON response with `child_session_id`, `status`, `output`, `workspace`, `branch`, and `worktree_path`. Times out after 600 seconds.

See [Sub-Agents](./advanced.md#sub-agents) for details.

---

## Tool Execution Flow

1. The LLM decides to call a tool and provides parameters
2. nca checks [permissions](./permissions.md) for the tool
3. If permission is `Ask`, the user is prompted for approval
4. The tool executes asynchronously
5. Results are fed back to the LLM for the next turn
6. Multiple approved tools can execute concurrently within a single turn

## Workspace Sandbox

All file tools enforce a workspace boundary:

- File reads, writes, and edits must resolve to paths within the workspace root
- Shell commands (`execute_bash`) run with the workspace as the working directory
- Attempts to access paths outside the workspace are rejected with an error

This sandbox protects against accidental or malicious file access outside your project.
