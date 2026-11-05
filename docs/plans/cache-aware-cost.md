# Cache-aware cost and per-model pricing

Status: implemented on `dev` (uncommitted). Follows §3.4 item 3 of `docs/research/harness-improvements.md`.

## Problem

- `CostTracker::estimated_cost_usd` bills every model at Claude Sonnet `$3 / $15` per million tokens. The number in the TUI says nothing about the configured provider.
- `prompt_tokens_details.cached_tokens` (OpenAI-compatible) and `cache_read_input_tokens` / `cache_creation_input_tokens` (Anthropic) are parsed nowhere, so a cache hit is invisible.
- The CLI stores cost in whole cents (`(usd * 100) as u64`), so any turn under one cent prints `$0.0000`.

## Design

1. **Provider usage carries cache counts.** `StreamChunk::Usage` gains `cached_input_tokens` and `cache_write_input_tokens`. `input_tokens` is always the whole prompt, cached parts included. Anthropic reports only the uncached part in `input_tokens`, so its adapter adds both cache fields back in.
2. **Prices are per model.** `common::pricing::ModelPricing` is the shared shape. `core::cost` holds a built-in table of flat list prices and resolves a model name to a price. Config entries under `[pricing."<model>"]` win over the table.
3. **Unknown price means unknown cost.** A model with no price contributes `0.0` to the cost, and the display shows `n/a`. There is no Sonnet fallback.
4. **Cost is priced when it arrives.** Each usage chunk is priced at the model active at that moment, so a mid-session `/model` switch stays correct.
5. **The event carries cache counts.** `AgentEvent::CostUpdated` gains `cached_input_tokens` with a serde default, so old event logs still load.
6. **Display.** Cache hit percent shows in the TUI sidebar and the narrow status bar, and in the CLI session summary. Cost is stored as `f64` in the CLI, which fixes the sub-cent bug.

## Price sources (fetched 2026-11-02)

| Vendor | Source | Used for |
|---|---|---|
| Anthropic | https://docs.anthropic.com/en/docs/about-claude/pricing | base input, cache read (hit), 5-minute cache write, output |
| OpenAI | https://developers.openai.com/api/docs/pricing | Standard tier, flat rows only |
| MiniMax | https://platform.minimax.io/docs/guides/pricing-paygo | `MiniMax-M3.1-Flash` |

Excluded from the built-in table because their published price depends on context length: GPT-5.x and GPT-6 models, `claude-haiku-5-5` (above 100k prompt tokens), and `MiniMax-M3` (above 512k). Set `[pricing]` in config for those.

## Out of scope

- An ACP `usage_update` field for cached tokens.
- Persisting cached totals in `SessionState`.
- OpenAI extended cache retention writes (30-minute and 12-hour).

## Verification

- Unit tests: price lookup and ordering, cost arithmetic with cache read and write, cache-hit percent, provider usage parsing for both wire formats, config override merge, agent `CostUpdated` wiring. `cargo test --workspace` and `cargo clippy` on the touched crates pass.
- Live, configured provider (`custom` → `kenari.id`, `deepseek-v4-1-flash`, no built-in price), same prompt in a fresh workspace:

| Run | `input_tokens` | `cached_input_tokens` | cost |
|---|---|---|---|
| 1 | 3557 | 0 | `n/a` |
| 2 | 5455 | 3557 | `n/a` |

- Live, same provider with a test override (`input 1.0`, `cached 0.1`, `output 2.0` per million; test numbers, not real prices): the reported cost matched the formula from the event counts exactly in both runs (run 2: 5454 of 5454 cached, `$0.00054940`).
