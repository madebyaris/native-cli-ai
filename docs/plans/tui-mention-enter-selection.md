# TUI Mention Enter Selection Plan

## Goal

Make `Enter` accept the highlighted `@` file mention into the composer without immediately submitting the prompt.

## Implementation

- Add a small helper for applying the selected `@` completion with optional trailing whitespace.
- Update the composer `Enter` handler to treat an active `@` mention menu as a selection action, not a submit action.
- Keep `Tab` behavior unchanged aside from reusing the same helper.

## Validation

- `cargo test -p nca-cli`
- `cargo build --release`
- lints on edited files
