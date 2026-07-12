# Context Management & Auto-Summarize Feature

## Overview

This feature implements intelligent context management to prevent token overflow and hallucinations caused by excessive context window usage. When the conversation grows large, the system automatically summarizes older messages to maintain context continuity.

There are two complementary layers:

1. **Persisted compaction** (`ContextManager` in `nca-runtime`) — may rewrite `agent.messages` via AI summary / sliding window when the session is over budget.
2. **Provider-request smart compaction** (`context_view` in `nca-core`) — opt-in, non-destructive cloned view sent to the provider only. Canonical history and session JSON stay complete.

## Key Components

### 1. Context Manager (`crates/runtime/src/context_manager.rs`)

**Purpose:** Tracks context size, generates statistics, and handles message compaction.

**Features:**
- Token estimation using character-based approximation
- Context statistics tracking (usage %, message count)
- Sliding window for recent messages with **token-budget-aware** shrinking
- Whole tool_use / tool_result groups are never split
- System message preservation
- Summary generation prompts

**Configuration Options:**
```toml
[memory.context]
# Target context window size (approximate tokens). 0 = auto-detect.
context_window_target = 0
auto_detect_context_window = true
query_provider_models_api = true

# Maximum messages to retain after compaction
max_retained_messages = 50

# Percentage of context window that triggers auto-summarize (0-100)
auto_summarize_threshold = 75

# Enable automatic context summarization
enable_auto_summarize = true

# Opt-in provider-request view: off | dry_run | on
smart_compaction_mode = "off"
```

### 2. Smart context view (`crates/core/src/context_view.rs`)

**Purpose:** Deterministically classify conversation / tool groups and build a compact **request view**.

**Always preserved:**
- System messages
- Image-bearing (multimodal) messages
- User constraint-like prompts
- `ask_question` / `update_todos` and other non-read tools
- Failed / error tool results
- File edits/writes and recent turns

**Eligible for truncation / dedupe (older turns only):**
- `read_file`, `search_code`, `list_directory`, `git_status`, `git_diff`, `web_search`, `fetch_url`, `query_symbols` outputs
- Repeated `file:path` mention lines

**Modes:**

| Mode | Provider request | Canonical `AgentLoop.messages` | Diagnostics |
|------|------------------|----------------------------------|-------------|
| `off` | Full history | Unchanged | None |
| `dry_run` | Full history | Unchanged | `ContextCompaction` phase `dry_run` |
| `on` | Compact clone | Unchanged | `ContextCompaction` phase `completed` |

Leave the default `off` until `dry_run` data shows safe savings across MiniMax and other providers.

### 3. Auto-Summarize Integration (`crates/runtime/src/supervisor.rs`)

**Purpose:** Integrates context management into the session lifecycle.

**Flow:**
1. Before each `run_turn`: Check if context needs attention
2. Each provider request: optionally apply smart context view (`agent.rs`)
3. After each `run_turn`: Check if summarization should trigger
4. If threshold exceeded and the compact request view still cannot fit: AI summary / sliding-window fallback
5. Emit events: `ContextWarning`, `ContextCompaction`

**Events Emitted:**
- `ContextWarning`: When context reaches 80% of target
- `ContextCompaction`: During summarization / smart-view phases (`starting` / `completed` / `dry_run`)
  - Optional fields: `tokens_before`, `tokens_after`, `retained_groups`, `dropped_groups` (serde-defaulted for old logs)

TUI `/status` shows the latest compaction report; human streams print one concise line for completed/dry-run phases.

### 4. Configuration Schema (`crates/common/src/config.rs`)

```rust
pub enum SmartCompactionMode { Off, DryRun, On }

pub struct ContextConfig {
    pub context_window_target: usize,          // Default: 0 (auto-detect)
    pub auto_detect_context_window: bool,      // Default: true
    pub query_provider_models_api: bool,       // Default: true
    pub max_retained_messages: usize,          // Default: 50
    pub auto_summarize_threshold: u8,          // Default: 75
    pub enable_auto_summarize: bool,           // Default: true
    pub smart_compaction_mode: SmartCompactionMode, // Default: Off
}
```

**Nested under `memory`:**
```toml
[memory]
file_path = ".nca/memory.json"  # Default sentinel → ~/.local/share/ncacli/workspaces/<id>/memory.json
max_notes = 128
auto_compact_on_finish = false

[memory.context]
context_window_target = 0
max_retained_messages = 50
auto_summarize_threshold = 75
enable_auto_summarize = true
smart_compaction_mode = "dry_run"
```

## How It Works

### Token Estimation
```rust
// Rough approximation: tokens ≈ characters / 4
// Tool messages: more token-dense (3.5 divisor)
// System messages: standard (4.0 divisor)
// + 10 base overhead + ~50 per tool call
```

### Persisted Compaction Strategy
1. **Preserve System Messages**: Always keep at start
2. **Sliding Window**: Keep last N messages (configurable), shrink further by whole tool groups to meet token budget
3. **Summarize Middle**: Old messages get summarized by AI when still over threshold
4. **Insert Summary**: Summary inserted as system message with special header

### Smart Request View Strategy
1. Partition into atomic groups (system / user / assistant / assistant+tool results)
2. Mark must-keep groups (images, failures, writes, decisions, recent window)
3. Truncate older compactible tool outputs; dedupe repeated file mentions
4. Send the clone (or keep full history in `dry_run` / `off`)

### Summary Format
```
## Conversation Summary (Earlier Context)

[AI-generated concise summary covering:]
- Key topics and goals discussed
- Important decisions or findings
- Critical context (file paths, variable names, errors)
```

## Usage Examples

### Default Behavior
Simply start a session - persisted auto-summarize is enabled by default. Smart compaction stays `off`.

### Measure savings first
```toml
[memory.context]
smart_compaction_mode = "dry_run"
```
Then watch `/status` and the human stream for `smart context: ~before→after tokens` lines.

### Enable for requests
```toml
[memory.context]
smart_compaction_mode = "on"
```

### Custom Thresholds
For very long conversations, increase thresholds:

```toml
[memory.context]
context_window_target = 128000
max_retained_messages = 80
auto_summarize_threshold = 85
```

## Future Improvements

- Hierarchical / multi-level summaries
- Importance scoring beyond the current deterministic classifier
- Per-provider token accounting from usage APIs
- Async background summarization
