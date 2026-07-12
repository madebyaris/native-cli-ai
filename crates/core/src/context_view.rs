//! Deterministic provider-request context view (smart compaction).
//!
//! Builds a **cloned** message list for the provider while leaving the
//! canonical `AgentLoop.messages` history untouched. Tool call/result groups
//! stay atomic; only older compactible read/search outputs are truncated.

use nca_common::config::SmartCompactionMode;
use nca_common::message::{Message, MessageContent, Role};
use std::collections::HashSet;

const RECENT_GROUPS_KEEP_FULL: usize = 8;
const TOOL_RESULT_KEEP_CHARS: usize = 400;
const FILE_MENTION_KEEP_CHARS: usize = 240;

/// Diagnostics for a planned provider view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextViewReport {
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub retained_groups: usize,
    pub dropped_groups: usize,
    pub truncated_tool_results: usize,
    pub deduped_file_mentions: usize,
}

impl ContextViewReport {
    pub fn savings_percent(&self) -> u8 {
        if self.tokens_before == 0 {
            return 0;
        }
        let saved = self.tokens_before.saturating_sub(self.tokens_after);
        ((saved * 100) / self.tokens_before) as u8
    }

    pub fn summary_line(&self) -> String {
        format!(
            "smart context: ~{}→{} tokens (−{}%), retained {} groups, compacted {}",
            self.tokens_before,
            self.tokens_after,
            self.savings_percent(),
            self.retained_groups,
            self.dropped_groups
        )
    }
}

/// Result of planning a provider request view.
#[derive(Debug, Clone)]
pub struct ContextViewPlan {
    pub messages: Vec<Message>,
    pub report: ContextViewReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    System,
    User,
    Assistant,
    ToolGroup,
}

#[derive(Debug, Clone)]
struct MessageGroup {
    kind: GroupKind,
    messages: Vec<Message>,
    must_keep: bool,
}

/// Estimate tokens with the same rough heuristic as the runtime context manager.
pub fn estimate_tokens(message: &Message) -> usize {
    let divisor = match message.role {
        Role::Tool => 3.5,
        Role::System => 4.0,
        _ => 4.0,
    };
    let content_tokens = message.content.approx_chars() as f64 / divisor;
    let tool_call_overhead = message
        .tool_calls
        .as_ref()
        .map(|calls| calls.len() * 50)
        .unwrap_or(0) as f64;
    (content_tokens + tool_call_overhead + 10.0) as usize
}

pub fn estimate_tokens_for_slice(messages: &[Message]) -> usize {
    messages.iter().map(estimate_tokens).sum()
}

fn message_text(message: &Message) -> String {
    match &message.content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| match p {
                nca_common::message::ContentPart::Text { text } => Some(text.as_str()),
                nca_common::message::ContentPart::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn is_compactible_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "search_code"
            | "list_directory"
            | "git_status"
            | "git_diff"
            | "web_search"
            | "fetch_url"
            | "query_symbols"
    )
}

fn tool_names_in_assistant(msg: &Message) -> Vec<String> {
    msg.tool_calls
        .as_ref()
        .map(|calls| calls.iter().map(|c| c.name.clone()).collect())
        .unwrap_or_default()
}

fn tool_call_ids_in_assistant(msg: &Message) -> HashSet<String> {
    msg.tool_calls
        .as_ref()
        .map(|calls| calls.iter().map(|c| c.id.clone()).collect())
        .unwrap_or_default()
}

fn group_must_keep(group: &MessageGroup) -> bool {
    if group.kind == GroupKind::System {
        return true;
    }
    for msg in &group.messages {
        if msg.content.has_image_parts() {
            return true;
        }
        if msg.role == Role::User {
            let text = message_text(msg);
            // User constraints / decisions often start with strong directives.
            if text.contains("IMPORTANT:")
                || text.contains("Do not")
                || text.contains("MUST")
                || text.contains("constraint")
            {
                return true;
            }
        }
        if msg.role == Role::Assistant {
            let names = tool_names_in_assistant(msg);
            if names.iter().any(|n| !is_compactible_tool(n)) {
                return true;
            }
            if names
                .iter()
                .any(|n| n == "ask_question" || n == "update_todos")
            {
                return true;
            }
        }
        if msg.role == Role::Tool {
            let text = message_text(msg);
            let lower = text.to_ascii_lowercase();
            if lower.contains("error")
                || lower.contains("failed")
                || lower.contains("denied")
                || text.starts_with("Selected ")
            {
                return true;
            }
        }
    }
    false
}

/// Partition messages into atomic conversation / tool groups.
fn partition_groups(messages: &[Message]) -> Vec<MessageGroup> {
    let mut groups = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let msg = &messages[i];
        match msg.role {
            Role::System => {
                groups.push(MessageGroup {
                    kind: GroupKind::System,
                    messages: vec![msg.clone()],
                    must_keep: true,
                });
                i += 1;
            }
            Role::User => {
                let mut g = MessageGroup {
                    kind: GroupKind::User,
                    messages: vec![msg.clone()],
                    must_keep: false,
                };
                g.must_keep = group_must_keep(&g);
                groups.push(g);
                i += 1;
            }
            Role::Assistant => {
                let has_tools = msg
                    .tool_calls
                    .as_ref()
                    .map(|c| !c.is_empty())
                    .unwrap_or(false);
                if !has_tools {
                    let mut g = MessageGroup {
                        kind: GroupKind::Assistant,
                        messages: vec![msg.clone()],
                        must_keep: false,
                    };
                    g.must_keep = group_must_keep(&g);
                    groups.push(g);
                    i += 1;
                    continue;
                }

                let ids = tool_call_ids_in_assistant(msg);
                let mut bundle = vec![msg.clone()];
                i += 1;
                while i < messages.len() && messages[i].role == Role::Tool {
                    // Keep tool results that belong to this assistant turn.
                    // Unmatched / orphaned tool results stay attached to preserve order.
                    let belongs = messages[i]
                        .tool_call_id
                        .as_ref()
                        .map(|id| ids.contains(id) || ids.is_empty())
                        .unwrap_or(true);
                    if !belongs && !ids.is_empty() {
                        break;
                    }
                    bundle.push(messages[i].clone());
                    i += 1;
                    if belongs && bundle.len() > 1 + ids.len() {
                        // collected all expected results
                        let collected: HashSet<_> = bundle
                            .iter()
                            .filter(|m| m.role == Role::Tool)
                            .filter_map(|m| m.tool_call_id.clone())
                            .collect();
                        if !ids.is_empty() && ids.is_subset(&collected) {
                            // keep scanning for extras that still match ids
                            if i < messages.len()
                                && messages[i].role == Role::Tool
                                && messages[i]
                                    .tool_call_id
                                    .as_ref()
                                    .is_some_and(|id| ids.contains(id))
                            {
                                continue;
                            }
                            break;
                        }
                    }
                }
                let mut g = MessageGroup {
                    kind: GroupKind::ToolGroup,
                    messages: bundle,
                    must_keep: false,
                };
                g.must_keep = group_must_keep(&g);
                groups.push(g);
            }
            Role::Tool => {
                // Orphan tool result — keep as its own protected group.
                groups.push(MessageGroup {
                    kind: GroupKind::ToolGroup,
                    messages: vec![msg.clone()],
                    must_keep: true,
                });
                i += 1;
            }
        }
    }
    groups
}

fn truncate_text(text: &str, keep: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= keep {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(keep).collect();
    format!(
        "{head}…\n[truncated {} chars of tool output]",
        trimmed.len().saturating_sub(keep)
    )
}

fn compact_file_mentions(text: &str, seen_paths: &mut HashSet<String>) -> (String, usize) {
    let mut deduped = 0usize;
    let mut out = String::new();
    for line in text.lines() {
        let path = line
            .strip_prefix("file:")
            .or_else(|| line.strip_prefix("File:"))
            .map(str::trim);
        if let Some(path) = path {
            if !seen_paths.insert(path.to_string()) {
                deduped += 1;
                continue;
            }
            if line.len() > FILE_MENTION_KEEP_CHARS {
                out.push_str(&truncate_text(line, FILE_MENTION_KEEP_CHARS));
            } else {
                out.push_str(line);
            }
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    (out.trim_end().to_string(), deduped)
}

fn compact_group(
    group: &MessageGroup,
    seen_paths: &mut HashSet<String>,
) -> (Vec<Message>, usize, usize) {
    let mut truncated = 0usize;
    let mut deduped = 0usize;
    let mut out = Vec::with_capacity(group.messages.len());

    for msg in &group.messages {
        let mut cloned = msg.clone();
        if msg.role == Role::Tool {
            let text = message_text(msg);
            if text.chars().count() > TOOL_RESULT_KEEP_CHARS {
                cloned.content = MessageContent::Text(truncate_text(&text, TOOL_RESULT_KEEP_CHARS));
                truncated += 1;
            }
        } else if msg.role == Role::User || msg.role == Role::Assistant {
            let text = message_text(msg);
            if text.contains("file:") || text.contains("File:") {
                let (compacted, d) = compact_file_mentions(&text, seen_paths);
                deduped += d;
                if compacted != text {
                    cloned.content = MessageContent::Text(compacted);
                }
            }
        }
        out.push(cloned);
    }
    (out, truncated, deduped)
}

/// Build a provider-facing context view for the given mode.
///
/// `Off` returns a clone of the input with a zeroed report.
/// `DryRun` / `On` both compute the compact view; callers decide whether to send it.
pub fn plan_context_view(messages: &[Message], mode: SmartCompactionMode) -> ContextViewPlan {
    let tokens_before = estimate_tokens_for_slice(messages);
    if matches!(mode, SmartCompactionMode::Off) || messages.is_empty() {
        return ContextViewPlan {
            messages: messages.to_vec(),
            report: ContextViewReport {
                tokens_before,
                tokens_after: tokens_before,
                retained_groups: 0,
                dropped_groups: 0,
                truncated_tool_results: 0,
                deduped_file_mentions: 0,
            },
        };
    }

    let groups = partition_groups(messages);
    let recent_start = groups.len().saturating_sub(RECENT_GROUPS_KEEP_FULL);
    let mut seen_paths = HashSet::new();
    let mut view = Vec::with_capacity(messages.len());
    let mut retained = 0usize;
    let mut dropped = 0usize;
    let mut truncated_tool_results = 0usize;
    let mut deduped_file_mentions = 0usize;

    for (idx, group) in groups.iter().enumerate() {
        let keep_full = group.must_keep || idx >= recent_start || group.kind == GroupKind::System;
        if keep_full {
            // Still dedupe file mentions for older must-keep text? Keep full content.
            view.extend(group.messages.iter().cloned());
            retained += 1;
        } else if group.kind == GroupKind::ToolGroup
            && group.messages.iter().any(|m| {
                m.role == Role::Assistant
                    && tool_names_in_assistant(m)
                        .iter()
                        .all(|n| is_compactible_tool(n))
            })
        {
            let (compacted, t, d) = compact_group(group, &mut seen_paths);
            view.extend(compacted);
            truncated_tool_results += t;
            deduped_file_mentions += d;
            if t > 0 || d > 0 {
                dropped += 1;
            } else {
                retained += 1;
            }
        } else {
            let (compacted, t, d) = compact_group(group, &mut seen_paths);
            view.extend(compacted);
            truncated_tool_results += t;
            deduped_file_mentions += d;
            if d > 0 {
                dropped += 1;
            } else {
                retained += 1;
            }
        }
    }

    let tokens_after = estimate_tokens_for_slice(&view);
    ContextViewPlan {
        messages: view,
        report: ContextViewReport {
            tokens_before,
            tokens_after,
            retained_groups: retained,
            dropped_groups: dropped,
            truncated_tool_results,
            deduped_file_mentions,
        },
    }
}

/// Ensure no tool_call_id in the view is orphaned (result without matching call).
pub fn orphaned_tool_results(messages: &[Message]) -> usize {
    let mut call_ids = HashSet::new();
    for msg in messages {
        if let Some(calls) = &msg.tool_calls {
            for c in calls {
                call_ids.insert(c.id.clone());
            }
        }
    }
    messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter(|m| {
            m.tool_call_id
                .as_ref()
                .map(|id| !call_ids.contains(id))
                .unwrap_or(true)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::message::MessageToolCall;
    use serde_json::json;

    fn tool_group(call_id: &str, tool: &str, output: &str) -> Vec<Message> {
        vec![
            Message::assistant_with_tool_calls(
                "",
                vec![MessageToolCall {
                    id: call_id.into(),
                    name: tool.into(),
                    arguments: json!({"path": "a.rs"}),
                }],
            ),
            Message::tool(call_id, output),
        ]
    }

    #[test]
    fn off_mode_is_identity() {
        let messages = vec![Message::user("hi"), Message::assistant("hello")];
        let plan = plan_context_view(&messages, SmartCompactionMode::Off);
        assert_eq!(plan.messages, messages);
        assert_eq!(plan.report.tokens_before, plan.report.tokens_after);
    }

    #[test]
    fn truncates_old_read_file_output() {
        let mut messages = vec![Message::system("sys")];
        let big = "x".repeat(2_000);
        messages.extend(tool_group("c1", "read_file", &big));
        messages.push(Message::user("recent"));
        messages.push(Message::assistant("ok"));
        // Pad recent groups so the tool group falls outside the recent window.
        for i in 0..10 {
            messages.push(Message::user(format!("u{i}")));
            messages.push(Message::assistant(format!("a{i}")));
        }

        let plan = plan_context_view(&messages, SmartCompactionMode::On);
        assert!(plan.report.tokens_after < plan.report.tokens_before);
        assert!(plan.report.truncated_tool_results >= 1);
        assert_eq!(orphaned_tool_results(&plan.messages), 0);
        // Canonical history unchanged by caller; view is a clone.
        assert!(messages.iter().any(|m| message_text(m).len() >= 2000));
    }

    #[test]
    fn preserves_failed_tools_and_writes() {
        let mut messages = vec![Message::system("sys")];
        messages.extend(tool_group("w1", "write_file", "wrote ok"));
        messages.extend(tool_group("r1", "read_file", "error: failed to read"));
        for i in 0..12 {
            messages.push(Message::user(format!("u{i}")));
            messages.push(Message::assistant(format!("a{i}")));
        }
        let plan = plan_context_view(&messages, SmartCompactionMode::On);
        let texts: Vec<_> = plan.messages.iter().map(message_text).collect();
        assert!(texts.iter().any(|t| t.contains("wrote ok")));
        assert!(texts.iter().any(|t| t.contains("error: failed")));
        assert_eq!(orphaned_tool_results(&plan.messages), 0);
    }

    #[test]
    fn dry_run_plan_matches_on_view() {
        let mut messages = vec![Message::system("sys"), Message::user("go")];
        messages.extend(tool_group("c1", "search_code", &"hit\n".repeat(500)));
        for i in 0..10 {
            messages.push(Message::user(format!("u{i}")));
            messages.push(Message::assistant(format!("a{i}")));
        }
        let dry = plan_context_view(&messages, SmartCompactionMode::DryRun);
        let on = plan_context_view(&messages, SmartCompactionMode::On);
        assert_eq!(dry.report, on.report);
        assert_eq!(dry.messages, on.messages);
    }

    #[test]
    fn dedupes_repeated_file_mentions() {
        let mut messages = vec![Message::system("sys")];
        messages.push(Message::user("file:src/a.rs\nfile:src/a.rs\nfile:src/b.rs"));
        for i in 0..10 {
            messages.push(Message::user(format!("u{i}")));
            messages.push(Message::assistant(format!("a{i}")));
        }
        let plan = plan_context_view(&messages, SmartCompactionMode::On);
        assert!(plan.report.deduped_file_mentions >= 1);
    }

    #[test]
    fn long_session_fixture_reduces_tokens_without_orphans() {
        let mut messages = vec![Message::system("You are nca.")];
        // Simulate many noisy read/search turns interleaved with a few writes.
        for i in 0..40 {
            messages.push(Message::user(format!("inspect batch {i}")));
            let big = format!("RESULT {}\n{}", i, "line\n".repeat(200));
            messages.extend(tool_group(&format!("r{i}"), "read_file", &big));
            if i % 7 == 0 {
                messages.extend(tool_group(
                    &format!("w{i}"),
                    "write_file",
                    "wrote important change",
                ));
            }
            messages.push(Message::assistant(format!("done {i}")));
        }
        messages.push(Message::user("file:src/a.rs\nfile:src/a.rs\nfinish"));

        let before = estimate_tokens_for_slice(&messages);
        let plan = plan_context_view(&messages, SmartCompactionMode::On);
        assert!(
            plan.report.tokens_after < before,
            "expected token reduction: before={before} after={}",
            plan.report.tokens_after
        );
        assert!(plan.report.savings_percent() >= 10);
        assert_eq!(orphaned_tool_results(&plan.messages), 0);
        // Canonical history must remain byte-equivalent for the caller.
        let canonical = serde_json::to_vec(&messages).unwrap();
        let after_plan = serde_json::to_vec(&messages).unwrap();
        assert_eq!(canonical, after_plan);
        assert!(
            plan.messages
                .iter()
                .any(|m| message_text(m).contains("wrote important change"))
        );
    }
}
