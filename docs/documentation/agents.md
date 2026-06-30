# Custom Agents & Per-Agent Provider Routing

nca supports defining named **agent profiles** that each override the provider, model,
permissions, system prompt, and available tools. This lets you route different tasks to
different LLM endpoints — for example, sending code reviews to GPT-4o while keeping
your main coding session on MiniMax.

## Overview

There are three ways to define per-agent behavior:

| Method | Where | Best for |
|--------|-------|----------|
| **Config `[agents]`** | `config.toml` / `.nca/config.local.toml` | Named profiles used across the workspace |
| **Skill frontmatter** | `SKILL.md` YAML header | One-off agent with custom instructions |
| **AGENTS.md directives** | `AGENTS.md` sections | Quick per-skill overrides without separate files |
| **`spawn_subagent` params** | Tool call JSON args | Programmatic per-child-session routing |

All four methods can specify `provider`, `model`, and `permission_mode`. The config
`[agents]` section additionally supports `system_prompt_append` and `allowed_tools`.

---

## 1. Config `[agents]` — Named Agent Profiles

Define agent profiles in your config file. Each profile is a `[agents.<name>]` section.

### Full Example

```toml
# .nca/config.local.toml

# Main session stays on MiniMax
[provider]
default = "minimax"
[provider.minimax]
api_key_env = "MINIMAX_API_KEY"

# OpenAI credentials for agent profiles
[provider.openai]
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o"

# Anthropic credentials
[provider.anthropic]
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-3-7-sonnet-latest"

# ── Agent Profiles ──────────────────────────────────────────

[agents.code-reviewer]
provider = "openai"                         # Route to OpenAI endpoint
model = "gpt-4o"                            # Use GPT-4o for reviews
permission_mode = "plan"                    # Read-only, no edits
system_prompt_append = """\
Focus on security vulnerabilities, race conditions, \
and error handling. Rate severity: critical / major / minor / suggestion."""
allowed_tools = ["read_file", "search_code", "list_directory", "web_search"]

[agents.security-audit]
provider = "anthropic"                      # Route to Anthropic
model = "claude-3-7-sonnet-latest"
permission_mode = "plan"
system_prompt_append = "Apply OWASP Top 10 checklist systematically."
allowed_tools = ["read_file", "search_code", "list_directory", "grep", "web_search"]

[agents.fast-coder]
provider = "minimax"                        # Same as session default
model = "MiniMax-M2.5"
permission_mode = "accept-edits"            # Can edit files

[agents.minimal]
# Inherits everything from session defaults.
# Useful as a marker that the agent can reference.
```

### Available Fields

| Field | Type | Description |
|-------|------|-------------|
| `provider` | string | LLM provider: `"minimax"`, `"openai"`, `"anthropic"`, `"openrouter"`, `"zhipuai"`, `"deepseek"` |
| `model` | string | Model name on that provider (e.g. `"gpt-4o"`, `"claude-3-7-sonnet-latest"`) |
| `permission_mode` | string | `"default"`, `"plan"`, `"accept-edits"`, `"dont-ask"`, `"bypass-permissions"` |
| `system_prompt_append` | string | Extra text appended to the system prompt when this agent is active |
| `allowed_tools` | list of string | If set, **only** these tools are available (all others disabled) |

All fields are optional — missing fields inherit from the global config.

### Alias-Based Provider Resolution

If you set `model` but not `provider`, nca auto-detects the provider from well-known
model name patterns:

```toml
[agents.claude-reviewer]
model = "claude-sonnet"     # provider auto-resolved to "anthropic"

[agents.gpt-reviewer]
model = "gpt4o"             # provider auto-resolved to "openai"
```

Explicit `provider` always wins over alias-based inference.

---

## 2. Skill Frontmatter — Per-Skill Provider

Skills can declare their own provider in the YAML frontmatter:

```yaml
---
name: Code Reviewer
command: review-code
description: Deep code review with security focus
provider: openai
model: gpt-4o
permission_mode: plan
---

## Instructions

Review code for security issues...
```

When this skill is invoked (via `/review-code <task>`), nca switches the session to
the specified provider and model before running the skill prompt. The provider reverts
to the session default after the skill completes.

### Supported Frontmatter Keys

| Key | Values |
|-----|--------|
| `provider` | `"minimax"`, `"openai"`, `"anthropic"`, `"openrouter"`, `"zhipuai"`, `"deepseek"` |
| `model` | Any model string |
| `permission_mode` | `"default"`, `"plan"`, `"accept-edits"`, `"dont-ask"`, `"bypass-permissions"` |
| `context` | `"inline"` (default), `"fork"` |

See [Skills](./skills.md) for the full frontmatter reference.

---

## 3. AGENTS.md — Per-Section Provider Directives

In `AGENTS.md`, each `## Heading` section can include provider directives:

```markdown
## Code Reviewer

- provider=openai model=gpt-4o permission_mode=plan

Expert code review. Focus on correctness and edge cases.

## Translation Agent

- provider=anthropic model=claude-3-7-sonnet-latest

Translate code comments and documentation.
```

The directives must appear as the first bullet list after the heading. Multiple
directives can be on the same line separated by spaces.

---

## 4. `spawn_subagent` Tool — Programmatic Routing

When the agent spawns a child session, it can specify provider and model overrides:

```json
{
  "task": "Review the auth module for security vulnerabilities",
  "provider": "openai",
  "model": "gpt-4o",
  "use_worktree": true,
  "focus_files": ["src/auth.rs"]
}
```

The child session will use OpenAI's GPT-4o regardless of the parent session's
provider. This is useful for delegating specific sub-tasks to specialized models.

### Full `spawn_subagent` Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `task` | string (required) | Clear description of what the sub-agent should do |
| `provider` | string (optional) | Override LLM provider for this child session |
| `model` | string (optional) | Override model name for this child session |
| `focus_files` | array of string | Files the sub-agent should focus on |
| `use_worktree` | boolean | Isolated git worktree (default: `true`) |

---

## Provider Reference

Valid `provider` values and their required config sections:

| Provider | Value | Config Section | Example Model |
|----------|-------|----------------|---------------|
| MiniMax | `"minimax"` | `[provider.minimax]` | `MiniMax-M2.5` |
| OpenAI | `"openai"` | `[provider.openai]` | `gpt-4o`, `gpt-4o-mini` |
| Anthropic | `"anthropic"` | `[provider.anthropic]` | `claude-3-7-sonnet-latest` |
| OpenRouter | `"openrouter"` | `[provider.openrouter]` | `openai/gpt-4o` |
| ZhipuAI | `"zhipuai"` | `[provider.zhipuai]` | `glm-5.2` |
| DeepSeek | `"deepseek"` | `[provider.deepseek]` | `deepseek-v4-flash` |

You must have the corresponding API key configured (via `[provider.<name>].api_key` or
environment variable). If the target provider has no credentials, the switch will fail
with an error message.

---

## How It Works

1. **Config merge** — `[agents]` sections merge the same way as other config:
   global → workspace → env vars. Multiple files can contribute to the same agent
   profile (fields accumulate).
2. **Provider rebuild** — When a skill/subagent triggers a provider switch, nca
   creates a new provider connection to the target endpoint. The previous session
   model/message history is preserved.
3. **Scope** — Provider switches from skill invocation or `spawn_subagent` are
   scoped to that operation. The session default provider is not permanently changed.
4. **Tool gating** — `allowed_tools` is parsed at config load time. When an agent
   profile specifies it, only the listed tool names are exposed to the LLM in the
   tool definitions. This is a declarative deny-all-then-allow approach.

---

## See Also

- [Configuration](./configuration.md) — Full config reference
- [Skills](./skills.md) — Skill authoring and discovery
- [Providers](./providers.md) — Provider setup and credentials
- [Tools](./tools.md) — Built-in tool reference including `spawn_subagent`
