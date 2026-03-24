# File Mention Preview Compaction Plan

## Goal

Keep expanded `@file` context in the model payload while showing a compact, human-friendly preview in the transcript.

## Implementation

- Add a shared preview compactor that rewrites expanded ````file:path` fences back to `@path`.
- Use that compacted preview for user `MessageReceived` events.
- Keep assistant/tool previews unchanged.

## Validation

- `cargo test -p nca-common`
- `cargo test -p nca-cli`
- `cargo build --release`
