//! Agent loop, provider abstraction, harness builder, tool registry, and approval policy.
//!
//! Depends on [`nca_common`] only. All LLM calls flow through the [`provider::Provider`] trait.

#![allow(clippy::pedantic)]

/// Multi-turn conversation and tool-use loop.
pub mod agent;
/// Tool approval policy and handler hooks.
pub mod approval;
/// Fast-local code intelligence helpers.
pub mod code_intel;
/// Token usage and cost estimation.
pub mod cost;
/// Layered system prompt builder.
pub mod harness;
/// Lifecycle hook runner.
pub mod hooks;
/// Pending IPC approval state shared with the CLI.
pub mod ipc_pending;
/// LLM provider trait and adapters (MiniMax, OpenAI, Anthropic, etc.).
pub mod provider;
/// Skill installation helpers.
pub mod skill_installer;
/// Skill discovery and loading.
pub mod skills;
/// Built-in and MCP tool registry.
pub mod tools;
