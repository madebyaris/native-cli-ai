# Keenable as the primary web search backend

## Goal

Make Keenable the first backend used by `web_search`, while retaining the existing
providers as ordered fallbacks. Keenable's public endpoint works without a key; an
optional configured key or `KEENABLE_API_KEY` selects its keyed endpoint.

## Implementation

1. Use Keenable's keyless endpoint as the first default backend.
2. Keep configured keyed APIs and existing keyless engines as fallbacks.
3. Bound response snippets at the provider and parser, and document the user-facing order.
4. Preserve explicit user `web.search_backends` overrides.

## Verification

- Run formatting checks and Keenable unit tests.
- Run `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo build --release -p nca-cli`.
- Confirm the public endpoint accepts the keyless request and snippet limit when network
  access is available.

## Scope

This changes source defaults only. It does not modify existing user configs that explicitly
set `web.search_backends` or publish a release.
