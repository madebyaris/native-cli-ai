# Connection latency: how fast we connect, what blocks it (measured 2026-11-02)

Status: measured. No source changes yet; this is the plan the changes follow.

## How it was measured

- Machine: this Mac (Darwin, arm64), release build `target/release/nca` (13 MB, `lto = "thin"`, stripped).
- Workspace: `/tmp/measure-ws` (a fresh, non-git directory) so no project hooks or server config applied.
- Provider: `custom` → `kenari.id` with `deepseek-v4-1-flash`, the model set in this workspace.
- Method: `perl -MTime::HiRes=time` around `Command::spawn`, `test -S` polling for the Unix socket, the `ts` field of each NDJSON event, and `curl -w` timings for the provider alone.
- Figures are single runs unless a range is given; the provider was the noisiest component, so its numbers are ranges over 3–5 runs.

## Where the time goes: one-shot turn (`nca run --stream ndjson`)

| Phase | Measured | Controlled by |
|---|---|---|
| `exec` + clap parse + log init (floor, `nca --help`) | **10–11 ms** | nca |
| config load + workspace migration write | ~2 ms | nca |
| `SessionStarted` event (session ready) | **35 ms** from exec | nca |
| provider DNS + TCP + TLS | **75–120 ms** | nca (lazy) |
| provider first byte (TTFT) | **2.2–3.4 s** | provider |
| first byte → last byte | ~1.5 s | provider |
| total, one-shot turn | **2.9–4.7 s** | provider |

`nca`'s own share of a turn is **~50 ms**; the rest is the model's prefill/reasoning before it emits the first SSE byte.

## `nca spawn` → connectable socket

| Run | `spawn` returns | socket connectable | gap |
|---|---|---|---|
| 1 | 53 ms | 72 ms | 18 ms |
| 2 | 15 ms | 25 ms | 10 ms |
| 3 | 13 ms | 24 ms | 11 ms |
| 4 | 13 ms | 23 ms | 10 ms |
| 5 | 18 ms | 27 ms | 9 ms |

So the window in which the socket and the state file do not exist yet is **9–18 ms**.

## ACP (`nca acp`)

| Step | Warm disk cache | Cold model catalog |
|---|---|---|
| `initialize` response | 6.6–32 ms | 6.6 ms |
| `session/new` response | **22 ms** | **409 ms** |

Cold means `~/.local/share/ncacli/cache/model-catalog.json` was deleted, i.e. the first session of the day.

## Blockers found

1. **`spawn` returns before the session exists — deterministic failure, not a race.**
   `nca status <id>` immediately after `nca spawn` failed **6/6** with
   `Error: IO error: No such file or directory (os error 2)`; the same command succeeds after 50 ms.
   `nca attach` immediately after `spawn` fails the same way. The parent never waits for the child
   ([main.rs:954](../../crates/cli/src/main.rs#L954) builds and spawns, then prints).
   This breaks the "CLI as spawner" workflow: the id you get back is not usable yet.

2. **`attach` drops everything that happened before it connected.**
   Attaching 1.2 s into a run started at event id **5** — events 1–4
   (`SessionStarted`, the user `MessageReceived`, `BusyStateChanged`, `Checkpoint`) were lost.
   `IpcServer` broadcasts live only ([ipc.rs](../../crates/runtime/src/ipc.rs)) and nothing replays
   `<id>.events.jsonl`, so an observer must connect before the first token or it never sees the start.

3. **Cold model catalog adds ~400 ms to the first `session/new`**, and it is bounded by
   `CATALOG_WAIT = 2 s` ([server.rs:32](../../crates/acp/src/server.rs#L32)).
   On a slow provider the first session of the day can wait the full 2 s before the client gets a
   session id and a model picker.

4. **The provider connection is established lazily, after the user presses enter.**
   The `reqwest::Client` is built at session start but DNS/TCP/TLS happen on the first `POST`:
   the debug log shows `starting new connection` ~48 ms into the process and `connected` 70 ms later.
   That is 75–120 ms of avoidable latency sitting on the critical path of the first token.

5. **Every `NcaConfig::load_for_workspace` writes to disk.** The load calls
   `migrate_workspace_data_if_needed` → `ensure_workspace_cache`, which does `canonicalize` +
   `create_dir_all` + `std::fs::write(workspace.json)` synchronously
   ([config.rs:623](../../crates/common/src/config.rs#L623)). ACP calls `config_for` up to three times
   per request (directly and again through `store()`), so a single `session/new` performs several
   redundant writes on the runtime thread. Cheap today (~1 ms), but it is work proportional to the
   number of requests, not to the work being done.

6. **First-turn prompt is ~5.3k input tokens.** `CostUpdated` reported `input_tokens: 5329` for the
   question "count from 1 to 5": the dynamic system prompt plus ~28 tool schemas. This is prefill the
   provider must do before the first byte, on every turn.

## What is already fast (do not touch)

- Process floor is **10 ms** for a 13 MB static-ish Rust binary; no dynamic-linking win available.
- Session setup to "ready" is **35 ms**, including tool registry, PTY, IPC bind and first state save.
- Socket IPC connect is **sub-millisecond** once the socket exists; the current blocker is ordering,
  not transport cost.
- Streaming is parsed correctly (`stream: true`, SSE deltas, usage, tool calls); the late first byte is
  the provider buffering its reasoning, verified with `curl -N` on the same endpoint
  (first byte at 3.2 s, then bursts over 1.5 s).

## Plan

Ordered by user-visible effect per unit of change.

1. **Make `spawn` return a usable session.** Have the child signal readiness (inherit a pipe fd and
   write one byte after the state save and socket bind) and have `spawn` wait for it with a timeout
   (~2 s) before printing. Fall back to the current behaviour on timeout, but say so on stderr.
   Expected: removes a 100% failure of the documented spawner workflow.
2. **Replay before streaming in `attach`.** Read `<id>.events.jsonl`, emit every envelope with
   `id <= last_seen`, then subscribe to the socket and drop duplicates by envelope id.
   Expected: `attach` becomes a complete observer instead of a lossy tail.
3. **Warm the model catalog during `initialize`.** `initialize` arrives one round trip before
   `session/new`; start the catalog fetch there and let `load_model_catalog` wait on the in-flight
   future instead of a fresh request. Expected: ~400 ms off the first session of the day, and
   `CATALOG_WAIT` stops being reachable on a slow provider.
4. **Pre-connect the provider while the human is still typing.** After the provider is built, issue a
   cheap keep-alive request (a `HEAD`/`GET` on the models endpoint, or a real connection pool warm-up)
   in the background. Expected: **75–120 ms** off first-token latency on the first turn of every
   process, which is the whole nca-controlled share of TTFT.
5. **Do the workspace migration once per process.** Guard `migrate_workspace_data_if_needed` behind a
   `OnceLock`/`Once` and skip the `workspace.json` write when the bytes are unchanged.
   Expected: removes several synchronous writes per ACP request.
6. **Trim the first-turn prompt.** Measure the tool-schema share of the 5.3k tokens and consider
   moving rarely used tools behind a tier that `invoke_skill`-style discovery can surface, or
   shortening descriptions. Only worth it if the schema share is large; verify with real prefill
   timings before changing behaviour.

Items 1 and 2 are correctness of the connection contract; 3 and 4 are the measurable latency wins;
5 and 6 are hygiene and need measurement before they are worth the churn.

## Caveats

- The provider dominates every end-to-end figure. A different provider or model can move TTFT by
  seconds, so nca-side numbers (floor, session setup, socket) are the ones to optimise against.
- `kenari.id` streams in bursts rather than token by token, so "time to first token" here means
  "time to the first SSE byte"; per-token smoothness cannot be improved client-side.
- Single-machine, single-provider sample. Percentiles need the harness to run repeatedly.
