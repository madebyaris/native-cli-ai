# Performance Baselines (Phase 1.6)

These numbers are the _initial_ baselines for the Criterion benches added in
Phase 1.6 of the Speed/DX/Feature plan. Re-run after each perf change and record
regressions or wins next to the baseline.

Hardware: Apple Silicon (user dev machine), `cargo bench` with the default
`bench` profile (release+debug-asserts off, `lto = "fat"`).

## Runtime (`cargo bench -p nca-runtime --bench session_store_load`)

### `session_store_load` — load a saved `SessionState` from disk

| messages | median time |
|----------|-------------|
| 10       | ~19.5 µs    |
| 100      | ~40.5 µs    |
| 500      | ~122 µs     |

### `event_serialize` — `serde_json::to_string` of a `TokensStreamed` event

| metric | value    |
|--------|----------|
| median | ~118 ns  |

### `context_manager`

Synthetic conversation with alternating user/assistant messages, no tool calls.
`with_default_config("MiniMax-M2")`.

| operation                   | 50 msgs | 200 msgs | 1000 msgs |
|-----------------------------|---------|----------|-----------|
| `estimate_tokens_for_slice` | ~40 ns  | ~167 ns  | ~816 ns   |
| `stats`                     | ~61 ns  | ~198 ns  | ~823 ns   |
| `get_compaction_plan`       | ~37 ns  | ~85 ns   | ~304 ns   |

Conclusion: context manager is already very cheap even at 1k messages; no
further micro-optimisation needed until profiling says otherwise.

## CLI (`cargo bench -p nca-cli --bench tui_text`)

Note: because `nca-cli` is currently a bin-only crate, the TUI hot paths cannot
be imported directly. The bench file mirrors the `wrap_text` implementation
from `crates/cli/src/tui/app.rs` verbatim and uses a simplified pure-text
analogue of `parse_md_line` (`parse_md_line_plain`). When Phase 2.1 extracts
`nca-tui`, these benches move there and call the real functions.

### `wrap_text(s, width)`

| input        | width 40 | width 80 | width 120 |
|--------------|----------|----------|-----------|
| `short`      | ~130 ns  | ~130 ns  | ~130 ns   |
| `medium`     | ~1.2 µs  | ~1.0 µs  | ~0.9 µs   |
| `long_para`  | ~9.6 µs  | ~7.7 µs  | ~6.8 µs   |
| `multi_para` | ~18.6 µs | ~14.2 µs | ~12.2 µs  |

### `parse_md_line_plain`

| input        | median   |
|--------------|----------|
| `plain`      | ~73 ns   |
| `bold`       | ~244 ns  |
| `code`       | ~41 ns   |
| `many_bolds` | ~1.63 µs |

## How to record a new baseline

```sh
cargo bench -p nca-runtime --bench session_store_load -- \
  --warm-up-time 1 --measurement-time 3 --sample-size 30
cargo bench -p nca-cli --bench tui_text -- \
  --warm-up-time 1 --measurement-time 3 --sample-size 30
```

Criterion stores raw samples under `target/criterion/`. Compare successive runs
with `cargo bench -- --baseline main` after `cargo bench -- --save-baseline main`.
