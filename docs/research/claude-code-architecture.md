# Claude Code Architecture Research

## Scope

This note studies Claude Code as a production-grade terminal agent and extracts the parts that are worth borrowing into `nca`.

The local study snapshot lives under `research/vendor/claude-code/` and is intentionally gitignored. It contains a targeted subset of the public repository at [igun997/claude-code](https://github.com/igun997/claude-code/tree/main), focused on:

- root architecture files such as `src/main.tsx`, `src/commands.ts`, `src/tools.ts`, `src/Tool.ts`, `src/QueryEngine.ts`, `src/context.ts`, and `src/cost-tracker.ts`
- command and tool implementations under `src/commands/` and `src/tools/`
- extension and runtime seams under `src/bridge/`, `src/coordinator/`, `src/plugins/`, `src/skills/`, `src/server/`, and selected `src/services/`

This is a research input, not a code dependency and not a blueprint to port literally.

## Executive Summary

Claude Code is strong because it keeps the core loop simple while investing heavily in the surfaces around that loop:

- a large but coherent slash-command UX
- a typed and capability-rich tool contract
- layered permissions plus hooks
- aggressive session persistence and resume flows
- context compaction and warning machinery
- extension seams for MCP, skills, plugins, and IDE bridges

The main lesson for `nca` is not "become TypeScript + Ink." The lesson is to keep the Rust agent loop small and make the product surfaces around it sharper, safer, and more observable.

## Core Architectural Shape

At the center is a simple model-driven tool loop rather than a heavy planner/router stack. Public architecture analysis describes Claude Code as a straight "reason -> maybe call tool -> feed result back -> repeat" loop, with the model itself deciding when to use tools and when to stop. See [How Claude Code Works](https://cc.bruniaux.com/guide/architecture/).

That high-level design lines up with what the vendored subset exposes:

- `src/QueryEngine.ts` as the central query/tool-use engine
- `src/tools.ts` as the tool surface assembler
- `src/Tool.ts` as the shared contract for schemas, permissions, progress, and execution context
- `src/commands.ts` as the user-facing command registry

This is the right overall shape for `nca` too: keep the loop boring, and put product quality into the surrounding systems.

## Command Layer

The command layer is one of Claude Code's strongest product differentiators.

From `research/vendor/claude-code/src/commands.ts`, a few characteristics stand out:

- there is a broad command catalog rather than a tiny "chat plus a few utilities" surface
- commands cover both execution and operator control: compact, context, cost, doctor, hooks, permissions, model switching, memory, skills, resume, share, review, session management
- commands are feature-gated so the same codebase can expose different surfaces in different environments
- command implementations are separated into focused folders under `src/commands/`

What matters is not the count of commands. It is that production users are given fast, explicit control over session state and agent behavior without needing to phrase everything as natural language.

## Tool System

Claude Code's tool system is more disciplined than a simple "list of functions."

The vendored `src/tools.ts` and `src/Tool.ts` show several production patterns:

- tools are registered centrally
- tools may have aliases for backwards compatibility
- each tool exposes a one-line capability phrase for discovery/search
- the tool context carries permission state, progress callbacks, dynamic tool refresh, and in-progress tracking
- tool progress is a first-class concept, not just final results
- the contract is designed to support native tools, MCP tools, and deferred/lazy-loaded tools through one shared interface

This is a bigger idea than just "typed tools." The real value is that every downstream concern, including permissions, UI progress, dynamic discovery, and context budgeting, is routed through one shared tool contract.

## Permissions And Hooks

Claude Code uses a layered permission model, which is one of the clearest production-grade ideas to borrow.

Public documentation and architecture research describe four layers:

1. interactive prompts
2. allow/deny/ask rules
3. lifecycle hooks
4. optional sandboxing

See [Claude Code hooks](https://code.claude.com/docs/en/hooks) and [Claude Code sandboxing](https://code.claude.com/docs/en/sandboxing).

The vendored architecture mirrors that:

- `src/Tool.ts` carries `PermissionMode`, rule sets, and prompt-avoidance behavior
- permission state can differ by session mode and agent type
- hook execution is treated as part of the permission/control path, not as an afterthought

Two lessons matter most:

- permission behavior should be explicit, composable, and auditable
- the hooks system should be rich enough to block, mutate, or log tool execution using structured payloads

## Context Management And Compaction

Claude Code has clearly invested in context hygiene as a product surface.

Public architecture research highlights:

- manual compaction via `/compact`
- automatic compaction once context pressure crosses a threshold
- warning states before the hard threshold
- subagents as a way to isolate exploratory work and protect the parent context

See [How Claude Code Works](https://cc.bruniaux.com/guide/architecture/).

The vendored `src/services/compact/` confirms that this is not a single summarization function. It is a small subsystem:

- `compact.ts` for compaction flow
- `autoCompact.ts` for automatic pressure handling
- `compactWarningState.ts` for warning suppression/state
- `microCompact.ts` for more granular result trimming
- hook integration around compaction boundaries

The production insight is that compaction should be observable and controllable. Users need warnings, manual escape hatches, and predictable boundaries, not just silent summarization.

## MCP And Extension Model

Claude Code treats MCP as part of the primary architecture rather than a side feature.

The vendored subset shows:

- `src/services/mcp/` for authentication, transport, connection management, and normalization
- MCP resources as a first-class concept
- shared permission handling between native tools and MCP tools
- lazy MCP tool loading/search to reduce context pollution

The biggest idea here is tool discovery at scale. Public architecture research notes that Claude Code now avoids eagerly loading large MCP catalogs and instead relies on a search/deferred-loading pattern. This reduces token overhead and improves tool selection quality. See [Advanced Tool Use](https://www.anthropic.com/engineering/advanced-tool-use).

This matters for `nca` because naive MCP loading gets expensive quickly once users enable multiple servers.

## Skills, Plugins, And Specialized Agents

Claude Code exposes multiple extension seams rather than one giant plugin API:

- `src/skills/` for workflow-specific prompt bundles and helpers
- `src/plugins/` plus `src/services/plugins/` for plugin lifecycle and operations
- `src/coordinator/` for multi-agent coordination
- specialized agent types and background agents through tools rather than only commands

The strongest product pattern is separation of concerns:

- skills package behavior and task framing
- plugins package external capabilities
- subagents package isolation and delegation

That keeps the core agent loop from turning into one giant branchy prompt.

## Bridge And Remote Execution

Claude Code is designed to run behind more than one frontend.

The vendored `src/bridge/` contains machinery for:

- session ingress/session IDs
- bridge polling and capacity control
- session spawning
- transport compatibility
- multi-session behavior

This reinforces a useful architectural rule: the runtime should own session execution, while IDE or external shells attach through a structured bridge instead of scraping terminal output.

For `nca`, this aligns with the existing CLI/runtime split and Unix-socket event bus.

## What Is Worth Copying

The highest-value transferable patterns are:

- strong command ergonomics for session and operator control
- richer tool metadata for discovery and UI progress
- more layered permission decisions with clearer audit points
- compaction as an explicit UX, not just an internal safeguard
- lazy MCP tool loading/search
- bridge/runtime separation with stable machine-readable events
- differentiated extension seams for skills, hooks, and subagents

## What Not To Copy Literally

Several Claude Code traits should be treated as ideas, not implementation targets:

- TypeScript, Bun, React, and Ink specifics
- feature-flag sprawl across one monolith
- UI-heavy subsystems that are not relevant to `nca`'s Rust-first CLI
- remote/mobile/desktop surfaces before the local CLI is stronger
- giant registries without a Rust-native crate boundary strategy

## Bottom Line

Claude Code's production quality comes less from novel agent theory and more from disciplined surfaces around a simple loop:

- commands that let the operator steer the session
- tools with rich contracts
- permissions and hooks with explicit control points
- context and session systems built for long-running real work

Those are exactly the parts `nca` should absorb, but in a Rust-native way that preserves the current `common -> core -> runtime -> cli` architecture.
