# What the harness literature says about improving nca

Status: survey + codebase audit, 2026-11-02. Findings are ordered by (evidence strength × impact per unit of change). No source files changed by this document.

## 1. Method

- **Literature**: `web_search` was unavailable (the backend rejected every request), so I queried the arXiv API directly (`export.arxiv.org/api/query`) and read abstracts from the returned Atom feeds. Every citation below is an arXiv abstract I fetched in this session, with its real ID and submission date. Quotes are from those abstracts, not from memory.
- **Codebase**: read `crates/core/src/{harness,agent,context_view}.rs`, `crates/runtime/src/{supervisor,context_manager}.rs`, `crates/core/src/tools/mod.rs`, the provider layer, and the runtime session path.
- **Empirical check**: I probed the configured provider (`kenari.id`, `deepseek-v4-1-flash`) to confirm prefix caching exists and what breaks it (§3.2). Timings elsewhere in this doc come from `docs/plans/connection-latency.md`.

A note on scope: many of the most quantitative papers in this space are about the **serving layer** (vLLM, SGLang, schedulers, GPU tokenizers). nca is a client. I separate what nca can act on today from what only informs its provider strategy, because conflating the two would produce advice nca cannot implement.

## 2. The literature, by lever

### 2.1 Prefix cache is the dominant economic fact

**TurnCost: Turns, Not Observations: Cache-Aware Loop Engineering for Coding Agents** (arXiv:2610.15093, Oct 2026)

- Analyzed 600 public coding-agent trajectories, 2 scaffolds × 2 models.
- Median tool observation is only **118–286 tokens**; large build logs sit at the **99th percentile**.
- **98.6% of input tokens are served from a prefix cache**, so flat input pricing overestimates loop cost by **≈7×** on average.
- **Turn count explains 84% of the variance in trajectory cost; mean observation size explains 4%.**

This inverts the common optimisation instinct. Compressing tool output targets a secondary cost; unnecessary turns are the primary one.

**The Physical Cost of Context: Quality-Constrained Resource Accounting for Coding Agents** (arXiv:2610.34763, NeurIPS 2026 RAAAI workshop)

- "A shorter prompt can therefore cost more if it rewrites cached history."
- With mini-SWE-agent on 100 SWE-bench Verified instances, keeping only the last five observations (**Mask-5**) cuts prompt volume **33.3%** but loses **135.9k reusable tokens per task**: uncached prefill rises **608.5%**, the energy proxy rises **125.6%**, and the policy fails its quality gate.
- Preloading retrieved blocks cut model calls **35.7%** and energy **33.2%**.
- Enabling prefix caching cut that policy's uncached prefill **67.7%**.

**Understanding KV Residency in Production-Scale Agentic LLM Workloads** (arXiv:2610.22863, Microsoft, >121M model calls)

- Coding-agent median context grows from ~10K to ~68K tokens by the 30th call; **~80% of context tokens are served from the prefix cache**.

**Cache Engineering for Agents** (arXiv:2610.28577)

- "Modifying the trajectory prefix invalidates subsequent cached states and triggers costly re-prefilling of retained context."
- A tool-response **rolling-drop** cache policy beat threshold-triggered summarization by **+1.7 points** and **−27.7% cost**.

**HiState** (arXiv:2610.41508) and **Vesta** (arXiv:2610.14686) are serving-side: agent workloads "modify and revisit earlier contexts", exposing reusable prefixes that conventional hybrid caches never checkpoint. Vesta reports mean TTFT −57.8%.

### 2.2 Compaction is lossy in ways benchmarks hide

**Benchmarks Are Surprisingly Compaction-Friendly** (arXiv:2610.31778, Oct 2026)

- Five compaction strategies in OpenCode, 3 frontier models, 3 long-horizon benchmarks.
- **No compaction method outperformed default self-summarization**; methods that discard more content cost some models up to **20 points**.
- On ProgramBench the **first** compaction summary already drops about **one third** of the task's anticheating rules, rising monotonically with each compaction.
- On real SWE-chat sessions, instructions about **how** to work and what not to do are dropped **~6× more often** than specifications of what to build.
- The authors' explanation is important: current benchmarks are compaction-friendly because "the objective is fixed and the agent's progress stays on disk" — so silent loss doesn't show up as failure.

**Compact-Bench** (arXiv:2610.31710)

- 35 SWE-bench repairs, 146 sessions, 106 production chains, 14 summarizer backbones.
- **LLM summaries retain at most 16.0% of recurring details, versus 79.1% for mechanical extraction.** Production summaries retain 15.7% at the first boundary.
- Damage is stable and reproducible across backbones: "reliable compaction requires preserving the progress needed to complete a task, and retaining relevant information alone does not ensure successful continuation."

**Maximising Transparency in Context Compaction** (arXiv:2610.20116)

- `transparent-compact`: deterministic, guarantees user and assistant prose is **unchanged** by any model or algorithm, and explicitly flags where information was stripped. Matched or beat model-written summaries, with the largest advantage on lightweight models.

**Same Context, Different Agents** (arXiv:2610.20959) — a caution against blanket compaction:

- Across 7 models, 1,514 tasks: CETs (semantic layers, orchestration, compaction) benefit only the behaviour group they match. For *investigator*-type agents, **compaction makes them repeat steps**, and orchestration costs them 5 points.
- Combining CETs yields smaller gains **and losses** than their sum.

### 2.3 Verification is the best-measured accuracy lever

**Rethinking Verification in Agent Harnesses** (arXiv:2610.34577, Oct 2026)

- 17 designs × 7 frontier coding models on Terminal-Bench and SWE-bench Verified.
- **Independent review by a critic: +5.9–9.5 points on Terminal-Bench and +3.9–6.6 on SWE-bench at 1.13–1.25× and 1.00–1.04× baseline cost.**
- Completion gates can yield larger gains but "may substantially increase cost". Initial instructions and mid-turn reminders give only "modest, model-dependent" gains.
- Additional review rounds or multiple critics add up to +3.5 points on Terminal-Bench.

This is the clearest accuracy-per-dollar result in the survey: near-zero extra token cost (1.00–1.04× on SWE-bench) for a multi-point gain.

### 2.4 Harness structure, delegation, and its limits

**Spine: Compiling Subtask Trees into Structured Context for Long-Horizon Coding Agents** (arXiv:2610.37224)

- Replaces completed subtasks' details with their results *while preserving prefix-cache reuse*.
- In Codex with GPT-5.6 Sol: SWE-Milestone Score **+11.8%**, total cost **−32.4%**; ProgramBench test-pass **+17.3%**.
- Versus **Recursive Subagent**: comparable score at **73.7% lower total cost**.
- Also notes the failure mode of naive trimming: compressing history "without explicit subtask boundaries can discard information needed by unfinished subtasks", while delegation-for-everything "conflates subtask granularity with executing agent choice".

**StateHarness: Maintaining Evolving Project State for Long-Horizon Coding Agents** (arXiv:2610.17616)

- Externalises evolving project state: current problem and hypothesis, findings and decisions, discovered repository relationships, unfinished work, plus a compact working brief.
- RefactorBench full-chain completion **29.4% → 35.3%**, cleanup-check accuracy **87.0% → 95.4%**; LHTB mean reward **0.427 → 0.576**; ChainSWE bug-fix rate 35.1% → 39.5%.
- Honest about costs: increases recorded execution cost and pass-to-pass regressions on ChainSWE.

**Humanize MLE Agents** (arXiv:2610.13494) — two named failure modes worth designing against:

- **Long-horizon deceleration**: "as single-session execution lengthens, the agent's reasoning intensity declines alongside slowing progress."
- **Harness staleness**: "the benefits of these custom harnesses vanish as the underlying model improves."
- Their fix is session alternation with context reset: **78.2% any-medal on MLE-bench**, +8.0 points over the strongest baseline at **a quarter of the time budget**.

**Towards Portable Specification of the Agent Harness** (arXiv:2610.40917)

- Decomposes the harness into **eight functional components** and proposes a portable spec.
- Justification: "different harness configurations can yield several-fold differences in reported scores for the same model", and the harness is a large part of the attack surface. It should be "explicitly described and specified as a versioned part of the system".

**How Do Coding-Agent Harnesses Self-Evolve?** (arXiv:2610.34938)

- Ran a "retro" skill on Claude Code over 30 SWE-bench Verified issues; implemented 24 of 125 proposals, evaluated 12.
- Of the 7 evaluations where a control session hit the targeted problem: **2 eliminated** it, **1 reduced** it, **4 showed no reduction**, and some changes **added work on unrelated issues**.

So harness changes must be A/B'd, not assumed — which is also why the papers above report deltas rather than anecdotes.

**Build or Adopt? Custom-Built vs. General-Purpose LLM Agent Harnesses** (arXiv:2610.33636): general-purpose harnesses cost **2.6–10.7× more inference per task** but saved enough engineering to lower total monthly cost 3–66%; "external advances can turn an in-house harness into technical debt even without deterioration".

### 2.5 Security: compaction as a trust boundary

**Compaction Laundering: Provenance Destruction in Context-Compacting Coding Agents** (arXiv:2610.29869, Oct 2026)

- Harnesses that flatten typed conversation into one plain string with text labels like `[User]:` destroy message provenance **in code**, before the summarizer runs.
- Across five flattening harnesses (pi, opencode, Cline, Aider, OpenHands) a forged approval launders into memory **87–100%** of the time; across four role-typed harnesses (Codex, Gemini CLI, Qwen Code, Continue) the identical attack succeeds **0%** of the time.
- "Because the provenance is destroyed deterministically in harness code, no prompt-level defense can restore it." A code-level provenance check catches 77% of laundering while demoting 2% of legitimate summary content.

This is a 0%-vs-100% design distinction, decided entirely by whether the harness keeps roles typed at the point of compaction.

### 2.6 Tool-output truncation done badly

**Context Is a Pipeline, Not a Window: Information Attrition in LLM Agent Harnesses** (arXiv:2610.36114)

- In a real fault-localisation agent, the largest prompt used only **5.75%** of the window, yet **3,827 of 11,189 tool results (a third)** were cut before reaching the model, and **every cut result became broken JSON** — the harness sliced JSON text at a fixed length.
- The cut is not even: one field reached the model in **4.7%** of calls, another in the very same results in **99.3%**.
- Cost surfaced first as **work**, not wrong answers: **+43% tool calls, 2× computation, yield 0.61 → 0.31**.
- A survey of seven agent frameworks found **two that cut tool output the same unsafe way**.

### 2.7 Failure memory

**What Does a Repair Agent Gain from Remembering Its Failure?** (arXiv:2610.41433)

- On 75 BugsInPy bugs, 181 failed attempts, 5 memory conditions: every condition gained over a plain retry in point estimate (+5.5 to +9.9 points over 29.8%), **none significantly**.
- A **model-written lesson showed no advantage over the transcript it summarises** (−1.7 points, 95% CI [−8.7, +4.9]) while costing an extra model call.
- A "notice that you failed" alone produced no significant change; the transcript is what carries the information.

### 2.8 Approval

**"Shall We Approve?" Consequence-Aware Approval for Autonomous Coding Agents** (arXiv:2610.40753)

- Existing approvers judge the request text and command. **Caper** pre-executes each command in a snapshotted sandbox, gives an independent approver a bounded report of observed effects, commits allowed commands and rolls back blocked ones with an actionable reason.
- Lowest harmful-outcome rate on SABER: **6.7% vs ≥14.8%** for six existing systems; **7.8% vs ≥35.3%** on YoloFS-Ext, while blocking 1.0% of safe twins.

## 3. What this means for nca

### 3.1 P0 — the compaction summarizer never receives the conversation

`ContextManager::summary_prompt` (`crates/runtime/src/context_manager.rs:369-395`) builds a prompt ending in:

```
---

Conversation to summarize:

```

and then returns. `perform_auto_summarize` (`crates/runtime/src/supervisor.rs:588`) passes that string straight to `summarize_with_ai` (`:592`), which sends `vec![Message::user(prompt)]` — **the prompt only**. The messages collected by `get_messages_to_summarize` are used to compute a token estimate and a count, then discarded.

`apply_summary` (`context_manager.rs:229`) then removes every message in `summarize_range` and replaces them with whatever the model wrote in response to an empty conversation. So the first compaction silently destroys the history it was supposed to condense, and the replacement text is not derived from it.

No test covers this: `summary_prompt` has no test, and `apply_summary_preserves_tool_use_result_pairs` (`:608`) only checks that tool call/result pairs stay atomic.

This is a bug, not a design choice, and it is exactly the class of silent compaction damage §2.2 measures. Fix: serialise the messages into the prompt — and do it **role-typed**, not flattened, per §3.5.

### 3.2 P0 — volatile content sits first in the system prompt, so every turn re-prefills

`build_system_prompt` (`crates/core/src/harness.rs`) emits sections in this order: identity → permission mode → **Environment (cwd, git branch, model, permission mode)** → **Session Todos** → **Memory Notes** → AGENTS.md → project instructions → local instructions → skills → orchestration → tool playbook.

`refresh_system_prompt` runs **before every turn** (`supervisor.rs:454`; also `:306`, `:403`, `:824`, `:903`). The system prompt is message 0, so it precedes the entire conversation and the tool schemas.

Consequence: any todo update, memory note, branch change, or model switch rewrites the first bytes of the prompt and invalidates the prefix for everything after it. With ~5.3k input tokens reported for a trivial turn (measured in `docs/plans/connection-latency.md`), that is the whole prefix re-prefilled.

I verified the mechanism against the configured provider:

| Request | prompt_tokens | cached_tokens | TTFB |
|---|---|---|---|
| Same ~2.5k-token prefix, 3rd call | 2555 | 165 | 3.90 s |
| Prefix changed at line 1 | 2555 | **0** | 3.56 s |
| Same changed prefix again | 2555 | **2555** | 2.85 s |

The provider reports `prompt_tokens_details.cached_tokens` and honours prefix reuse; changing one early line drops it to zero. (The provider was also visibly degraded during the probe — 15 s and 43 s TTFB spikes on the first two calls — so treat the latency column as indicative and the cached_tokens column as the actual result.)

Fix: order the system prompt **static → volatile**, i.e. identity, permission mode, AGENTS.md, project/local instructions, skills, tool playbook first; environment, todos, memory, orchestration last. Same information, stable prefix, and `update_todos` stops invalidating the cache.

### 3.3 P0 — mid-history truncation and eviction rewrite the prefix too

`plan_context_view_fitted` (`crates/core/src/context_view.rs`) keeps the newest `RECENT_GROUPS_KEEP_FULL = 8` groups intact and truncates older tool results to `TOOL_RESULT_KEEP_CHARS = 400`, then evicts oldest non-system groups to fit the budget.

Every turn, groups fall out of the protected window and get truncated, and eviction removes prefix content. That is precisely the Mask-5 pattern measured in arXiv:2610.34763: prompt volume down 33%, **uncached prefill up 608.5%**, quality gate failed. The cache-aware literature (arXiv:2610.28577) says the same in one line: don't edit the trajectory prefix.

Fix direction: prefer **append-only growth with a stable prefix**, and when trimming is unavoidable, do it at a small number of *anchored* boundaries rather than continuously re-truncating the window (arXiv:2610.34763's "anchored history compaction" passed its quality gate). Cache Engineering's `drop`/`repos` idea is the serving-side version; the client-side equivalent is to stop mutating messages that the model has already seen.

### 3.4 P1 — cost accounting is wrong twice over

Two independent problems, both in the same display:

**(a) Prices are hardcoded to one vendor.** `CostTracker::estimated_cost_usd` (`crates/core/src/cost.rs`) bills every model at `$3 / $15` per million tokens — Claude Sonnet pricing — with the comment "Real implementation should look up per-model pricing." Every `CostUpdated` event (`crates/core/src/agent.rs:585`, `:798`) therefore reports a number that is unrelated to the configured provider: `deepseek-v4-1-flash` through `kenari.id` is not priced like Claude Sonnet.

**(b) Caching is invisible.** `prompt_tokens_details.cached_tokens` is parsed nowhere (`openai_compat.rs:124` reads only `prompt_tokens` and `completion_tokens`; a repo-wide grep for `cached_tokens`/`cache_control` finds nothing), and neither is Anthropic's `cache_read_input_tokens`.

arXiv:2610.15093 measured 98.6% of input tokens served from cache typically, making flat pricing ≈7× too pessimistic. Combined with (a), the number in the TUI is close to meaningless, and nca has no way to tell whether a harness change helped.

Fix: per-model pricing, parse `cached_tokens`, bill cached reads at their real (discounted) rate, and surface `cached %` in the status line. Then §3.2's fix becomes measurable, and "did compaction save money or cost more work?" becomes answerable.

### 3.5 P1 — keep compaction role-typed (do not flatten)

nca does **not** currently flatten messages for summarisation — `prepare_for_summary` preserves `role`, `tool_call_id` and `tool_calls`. That is the safe side of arXiv:2610.29869's 0%-vs-100% split, and it should be an explicit invariant with a test, because the obvious "fix" for §3.1 (dump the conversation into the prompt string) is exactly the vulnerable pattern.

Correct §3.1 fix: keep messages typed in the summariser request (e.g. replay the range as real user/assistant/tool messages with an instruction prefix), or if a single string is unavoidable, escape role labels per the paper. Add a regression test that a forged `[User]: ...` inside a tool result cannot round-trip as an instruction.

### 3.6 P1 — add independent critic review

nca's system prompt tells the agent to verify with `run_validation`, but verification is self-review. arXiv:2610.34577 measured independent critic review at **+3.9–6.6 points on SWE-bench at 1.00–1.04× cost**.

Concrete shape for nca: after a turn that changed files and passed `run_validation`, run a **separate review call with a fresh, minimal context** (diff + task statement + validation output, no tool schemas) that returns pass/fail plus objections, and feed failures back as one revision turn. This reuses the existing provider `chat` path and costs one cheap call.

Pair it with a toggle, because the same paper shows completion gates "may substantially increase cost", and arXiv:2610.20959 shows blanket additions can hurt a specific agent behaviour class.

### 3.7 P1 — instrument turns, not tokens

If arXiv:2610.15093's numbers hold for nca (84% of cost variance from turn count, 4% from observation size), then nca's efficiency work should be steered by a **turns-per-task** metric, not by tool-output compression. nca already persists every event to `<id>.events.jsonl`, so turns, tool calls and retries are already on disk and merely unaggregated. A `nca sessions --stats` summary (turns, tool calls, cached %, per-session cost) would make every later change measurable, and would make the §3.9 warning about harness self-evolution actionable.

### 3.8 P2 — structured project state instead of a growing transcript

nca has `update_todos` and model-written memory notes, but no place to record the current hypothesis, decisions, discovered repository relationships and remaining work — the StateHarness brief (arXiv:2610.17616) that produced 29.4% → 35.3% full-chain completion and 87.0% → 95.4% cleanup accuracy.

nca is close: `SessionMeta` already carries `session_summary`, `todos` and lineage. Extending the existing `update_todos` tool with a short "brief" (problem, hypothesis, decisions, open questions) that the runtime renders near the **end** of the system prompt (per §3.2) is a small change.

### 3.9 P2 — delegation granularity

nca's `spawn_subagent` creates a separate session, optionally in a git worktree, with its own event log and parent lineage — the "recursive subagent" pattern. Spine measured comparable quality to that pattern at **73.7% lower cost** by keeping sequential subtasks inline within one session and replacing *completed* subtask detail with its result, while reserving delegation for genuinely parallel work.

That reframes an existing user preference (isolated worktrees for sub-agents) into a sharper rule: **worktrees and sub-sessions for parallel isolation, not for sequential decomposition**. The latter is where the cost lives.

### 3.10 P2 — approval by consequence, not by command text

nca's approval decision today is the request text (`ApprovalPolicy` + `IpcApprovalHandler`). Caper's result (6.7% vs ≥14.8% harmful outcomes) comes from pre-executing in a snapshot and showing the approver what actually happened. That is a larger change than the rest and depends on a sandbox; worth prototyping behind a flag for `execute_bash` only.

### 3.11 P3 — treat the harness as versioned and re-measurable

Three separate papers argue the harness is a first-class, versioned artefact (arXiv:2610.40917), that its benefits fade as models improve (arXiv:2610.13494, "harness staleness"), and that most self-proposed harness changes do nothing (arXiv:2610.34938, 4 of 7). Practical consequence for nca: record the harness configuration (prompt version, compaction strategy, tool set) in session metadata next to the model, so a score change can be attributed rather than guessed.

## 4. What nca already gets right

Worth stating, because it bounds the recommendation set:

- **Tool output capping is safe.** `cap_text` (`crates/core/src/tools/mod.rs:207`) keeps head **and** tail around an explicit omission marker with the omitted count, and is char-boundary safe (tests at `:363-427`). That is the opposite of the fixed-slice anti-pattern in arXiv:2610.36114, where every truncated result became broken JSON.
- **Messages stay role-typed** through compaction (§3.5), which is the provenance-safe side of arXiv:2610.29869.
- **Group atomicity.** `plan_context_view_fitted` keeps tool call/result groups intact and protects the system prompt, first user task and newest turn — the prerequisite Spine (arXiv:2610.37224) says naive trimming lacks.
- **No speculative prompt bloat.** The system prompt is layered and capped (`MAX_MEMORY_NOTES = 12`, `MEMORY_CHARS = 400`, `TODOS_IN_PROMPT = 20`), so the prefix is bounded rather than growing without limit.
- **Deterministic context view.** Building the provider view as a clone while leaving canonical history untouched is the right architecture for retries and for cache reasoning.

## 5. Sequencing

| # | Change | Evidence | Expected effect | Effort |
|---|---|---|---|---|
| 1 | Serialise messages into the summariser prompt, role-typed | §3.1, arXiv:2610.31710/29869 | Stops silent total history loss on first compaction | S |
| 2 | Reorder system prompt: static first, volatile last | §3.2, arXiv:2610.15093/22863 + probe | Prefix survives todo/memory churn; large cut in uncached prefill | S |
| 3 | Per-model pricing + parse `cached_tokens`, show cached % | §3.4, arXiv:2610.15093 | Makes 1–2 and future changes measurable; honest cost | S |
| 4 | Turn/tool-call/cached% stats per session | §3.7 | Steering metric for everything after | S–M |
| 5 | Independent critic review after validation | §3.6, arXiv:2610.34577 | +3.9–6.6 pts SWE at ~1.0× cost | M |
| 6 | Anchored (not per-turn) history trimming | §3.3, arXiv:2610.34763 | Removes the 608%-class prefill amplification | M |
| 7 | Mechanical + verbatim compaction instead of summary-only | §2.2, arXiv:2610.31710/20116 | Detail retention ~16% → ~79% | M |
| 8 | Project-state brief alongside todos | §3.8, arXiv:2610.17616 | 29.4% → 35.3% chain completion (their setup) | M |
| 9 | Delegation rule: parallel only, inline for sequential | §3.9, arXiv:2610.37224 | Up to −32% cost vs delegation-heavy runs | M |
| 10 | Sandbox pre-execution approval for shell | §3.10, arXiv:2610.40753 | Harmful-outcome rate ~½ | L |
| 11 | Harness version stamp in session metadata | §3.11 | Attributable measurements | S |

Items 1–3 are small, independent and each removes a defect rather than adding a feature. I would land them first and let item 4 prove the effect before touching compaction strategy.

## 6. Caveats

- **arXiv-only sourcing.** With `web_search` down I could not cross-check the vendor engineering blogs (Anthropic/OpenAI/DeepSeek caching docs) that define the pricing details in §3.4. The mechanism is verified against a live provider; the exact cache-read discount per vendor is not.
- **Single-provider, single-machine measurements.** The prompt-cache probe in §3.2 ran against `kenari.id` during a visibly degraded window; only the `cached_tokens` column is load-bearing.
- **Papers' numbers are their harnesses.** Reports like "+11.8% SWE-Milestone" or "29.4% → 35.3%" come from other scaffolds (Codex, OpenCode, Pi, mini-SWE-agent), other models and other benchmarks. They establish direction and order of magnitude, not nca's specific gain. Every item in §5 needs an in-repo A/B before it is claimed as an improvement — which is exactly what arXiv:2610.34938 found most self-proposed harness changes lack.
- **Provider-side results are context, not tasks.** HiState, Vesta, WAKMan, GraphTok and the residency studies describe what the serving layer can do (KV residency, routing, GPU tokenisation, 5.73× p99 TTFT). nca cannot implement them; they are relevant only when choosing or negotiating with a provider, or when running a self-hosted endpoint.

## 7. References

| ID | Title | Date |
|---|---|---|
| 2610.15093 | TurnCost: Turns, Not Observations: Cache-Aware Loop Engineering for Coding Agents | 2026-10-10 |
| 2610.34763 | The Physical Cost of Context: Quality-Constrained Resource Accounting for Coding Agents | 2026-10-26 |
| 2610.28577 | Cache Engineering for Agents | 2026-10-20 |
| 2610.22863 | Understanding KV Residency in Production-Scale Agentic LLM Workloads | 2026-10-16 |
| 2610.41508 | HiState: Hierarchical Prefix Cache Management for Hybrid LLM Serving in Agentic Workloads | 2026-10-30 |
| 2610.14686 | Vesta: Queueing-Aware LLM Scheduling for Real-World Workloads | 2026-10-10 |
| 2610.36355 | WAKMan: Workload Aware KVCache Manager | 2026-10-26 |
| 2610.13325 | Taking Tokenization off the Host for Agentic LLM Serving | 2026-10-09 |
| 2610.31778 | Benchmarks Are Surprisingly Compaction-Friendly | 2026-10-22 |
| 2610.31710 | Compact-Bench: Measuring What Context Compaction Costs Long-Horizon Coding Agents | 2026-10-22 |
| 2610.20116 | Maximising Transparency in Context Compaction Improves Retention of Facts and Commitments | 2026-10-14 |
| 2610.29869 | Compaction Laundering: Provenance Destruction in Context-Compacting Coding Agents | 2026-10-21 |
| 2610.20959 | Same Context, Different Agents: Semantic Layers, Multi-Agent Orchestration and Memory Compaction | 2026-10-14 |
| 2610.36114 | Context Is a Pipeline, Not a Window: Information Attrition in LLM Agent Harnesses | 2026-10-26 |
| 2610.34577 | Rethinking Verification in Agent Harnesses | 2026-10-25 |
| 2610.37224 | Spine: Compiling Subtask Trees into Structured Context for Long-Horizon Coding Agents | 2026-10-27 |
| 2610.17616 | StateHarness: Maintaining Evolving Project State for Long-Horizon Coding Agents | 2026-10-13 |
| 2610.13494 | Humanize MLE Agents: Multi-Agent Session Alternation Beyond Single-Agent Limits | 2026-10-09 |
| 2610.40917 | Towards Portable Specification of the Agent Harness | 2026-10-29 |
| 2610.34938 | How Do Coding-Agent Harnesses Self-Evolve? An Empirical Study of Retrospective Adaptation | 2026-10-26 |
| 2610.33636 | Build or Adopt? Custom-Built vs. General-Purpose LLM Agent Harnesses for Automotive GUI Testing | 2026-10-24 |
| 2610.41433 | What Does a Repair Agent Gain from Remembering Its Failure? | 2026-10-30 |
| 2610.40753 | "Shall We Approve?" Consequence-Aware Approval for Autonomous Coding Agents | 2026-10-29 |
| 2610.40721 | MetaVul: Agentic Harness Discovery for Vulnerability Detection | 2026-10-29 |
| 2405.15793 | SWE-agent: Agent-Computer Interfaces Enable Automated Software Engineering | 2024-05-06 |