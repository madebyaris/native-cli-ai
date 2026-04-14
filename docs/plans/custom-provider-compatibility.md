# Custom Provider Compatibility Plan

## Goal

Add a configurable `Custom` provider so users can route `nca` to arbitrary endpoints and choose whether the endpoint speaks an OpenAI-compatible or Anthropic-compatible API.

## Implementation

- Add a `Custom` provider config block in `common::config` with `base_url`, `model`, `api_key`, `api_key_env`, `temperature`, and a compatibility enum.
- Build a `CustomProvider` in `core` that reuses the existing OpenAI-compatible or Anthropic-compatible request/streaming paths based on the selected compatibility.
- Extend provider validation and model-catalog lookup so custom providers follow the selected compatibility behavior.
- Add `Custom` to provider selection surfaces and update the TUI/onboarding flow to collect compatibility, base URL, and API key before activating the provider.
- Update provider docs so custom endpoint setup is documented alongside the built-in providers.

## Validation

- `cargo test -p nca-common`
- `cargo test -p nca-core`
- `cargo test -p nca-runtime`
- `cargo test -p nca-cli`
- `cargo build --release`
