# Keep the cached prefix stable across turns

Status: measured, plan only. Not implemented; waiting for a go-ahead because it changes the harness contract in `AGENTS.md`.

## What was measured (2026-11-02)

Provider: `custom` → `kenari.id`, `deepseek-v4-1-flash`. Build: `dev` with the cache accounting from `docs/plans/cache-aware-cost.md`.

| Case | Result |
|---|---|
| Typical turns (3 sessions × 5 turns, `nca resume`) | 98–100% of prompt tokens cached |
| Turn after `update_todos` (benchmark turn 4, 3 sessions) | 3%, 53%, 100% cached |
| Controlled repeat (turn 3 → turn 4) | turn 4 sent 5639 tokens, 1792 cached; 3847 re-prefilled |

The stored system prompt, turn 3 vs turn 4, differs in one place: a `Session Todos` section appended at the end, after `Environment`. Nothing earlier changed. The cached 1792 tokens equal the length of the whole turn-3 system message, so the provider matched the system message and re-prefilled the conversation after it.

The benchmark's 100% turn-4 session also called `update_todos`, and I did not explain it. Treat the three-session spread as the variance to expect.

## Cause

- `Supervisor::refresh_system_prompt` runs before every turn and calls `AgentLoop::set_system_prompt`, which rewrites message 0 in the stored history (`crates/core/src/agent.rs`).
- The reorder in `543c02e` moved volatile sections after the static ones. That limits how much breaks, but the conversation after the system message still follows a changed prefix, so it is re-prefilled.

## Plan

1. **Freeze message 0.** It holds only static sections: identity, permission mode, `AGENTS.md`, project and local instructions, skills, tool playbook. Set it at session start and on explicit reload.
2. **Build a per-request block** from the volatile sections: environment, orchestration, session todos, memory notes.
3. **Attach the block to the newest user message in the request view only** (`AgentLoop::request_messages`). It is not stored in history, so history stays append-only and each request matches the previous one byte for byte up to the new user message.
   - Not a mid-history system message: the Anthropic adapter lifts leading system messages into the top-level `system` field (`crates/core/src/provider/anthropic_compat.rs`), so a mid-history system note would move and break the prefix again.
4. **Tests.** Two consecutive requests share their prefix up to the new user message. A todo change shows up in the next request. The volatile block never appears in persisted history.

## Expected effect

- The turn after a todo or memory change re-prefills only the new tail. In the controlled case that is roughly 3847 fewer uncached tokens on that turn.
- The model sees the same content on that turn; only its position changes.

## Risks

- The model sees todos next to the user message instead of in the system prompt. Check that it still follows the todo list.
- `AGENTS.md` says the system prompt is refreshed each turn. Update that line when this lands.
- Context compaction (`docs/research/harness-improvements.md` §3.3) can still rewrite older messages and break the prefix. That is a separate change.

## Latency: what the cache is worth on this provider

Same binary, same workspace, 8 pairs (hot = prefix identical, cold = `AGENTS.md` line changed near the top so the prefix misses from there on):

| | median TTFT | min | max | cached (cold case) |
|---|---|---|---|---|
| hot (prefix cached) | 6230 ms | 2308 ms | 15163 ms | 100% |
| cold (prefix missed) | 6595 ms | 3949 ms | 9834 ms | about 1703 of 3557 tokens |

The median difference is about 0.4 s, inside the spread. Provider time dominates: identical requests ranged from 2.3 s to 15 s. So on this provider the cache mostly saves cost, and a small amount of time per affected turn.

## Build overhead (current build vs `HEAD`)

- `config --json`, 30 interleaved runs: 4.6 ms vs 4.7 ms median.
- One turn, 8 alternating pairs: 4663 ms vs 4704 ms median TTFT. The earlier fixed-order run showed 7.3 s vs 4.6 s; reversing the order removed the gap, so it was ordering and provider noise.

## Not measured

- The cost effect depends on each provider's cached-read price, which `[pricing]` now sets.
