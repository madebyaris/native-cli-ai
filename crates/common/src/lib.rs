//! Shared types for the nca workspace: configuration, events, messages, sessions,
//! tool schemas, and orchestration metadata.
//!
//! This is the leaf crate — every other crate may depend on it.

/// Configuration loading, merging, and persistence.
pub mod config;
/// Agent event bus types and NDJSON envelopes.
pub mod event;
/// Conversation message and attachment types.
pub mod message;
/// Provider/model capability helpers (e.g. vision support).
pub mod model_caps;
/// Session persistence schema and lineage metadata.
pub mod session;
/// Session-scoped todo list model.
pub mod todo;
/// Tool definitions, calls, and results.
pub mod tool;
