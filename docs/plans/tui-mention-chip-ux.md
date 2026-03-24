# TUI Mention Chip UX Plan

## Goal

Make accepted `@` file mentions read like visible chips in the composer and remove them with a single Backspace.

## Implementation

- Render completed `@path` mentions in the composer with a dedicated background color.
- Add a helper that detects a completed mention immediately before the cursor, including the trailing spacer inserted on selection.
- Update Backspace to remove the entire mention token in one step before falling back to normal character deletion.

## Validation

- `cargo test -p nca-cli`
- `cargo build --release`
- lints on edited files
