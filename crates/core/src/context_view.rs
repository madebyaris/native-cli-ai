//! Deterministic provider-request context view (smart compaction).
//!
//! Builds a **cloned** message list for the provider while leaving the
//! canonical `AgentLoop.messages` history untouched. Tool call/result groups
//! stay atomic; only older compactible read/search outputs are truncated.

use nca_common::config::SmartCompactionMode;
use nca_common::message::{ContentPart, Message, MessageContent, Role};
use std::collections::HashSet;
use std::num::NonZeroUsize;

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
    /// Groups removed entirely to fit the token budget (oldest first).
    pub evicted_groups: usize,
    /// Token budget the view was fitted to, when one was set.
    pub token_budget: Option<usize>,
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
        let mut line = format!(
            "smart context: ~{}→{} tokens (−{}%), retained {} groups, compacted {}",
            self.tokens_before,
            self.tokens_after,
            self.savings_percent(),
            self.retained_groups,
            self.dropped_groups
        );
        if self.evicted_groups > 0 {
            line.push_str(&format!(", evicted {} groups", self.evicted_groups));
        }
        if let Some(budget) = self.token_budget {
            line.push_str(&format!(", budget {budget}"));
        }
        line
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
    let argument_tokens = message.tool_call_argument_chars() as f64 / 4.0;
    (content_tokens + tool_call_overhead + argument_tokens + 10.0) as usize
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
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Image { .. } => None,
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
    plan_context_view_fitted(messages, mode, None)
}

/// Like [`plan_context_view`], but hard-fits the result under `token_budget`.
///
/// Truncation alone cannot bound a long session: a conversation with hundreds
/// of already-small groups still overflows. After compaction this evicts the
/// oldest non-system groups (never splitting a tool call/result group) until the
/// view fits, then shrinks any remaining oversized message.
pub fn plan_context_view_fitted(
    messages: &[Message],
    mode: SmartCompactionMode,
    token_budget: Option<NonZeroUsize>,
) -> ContextViewPlan {
    let mut plan = plan_context_view_compacted(messages, mode);
    let Some(budget) = token_budget else {
        return plan;
    };
    if plan.messages.is_empty() {
        return plan;
    }
    plan.report.token_budget = Some(budget.get());
    fit_to_budget(&mut plan, budget.get());
    plan
}

/// Header that auto-summarization writes into its summary system message.
pub const CONTEXT_SUMMARY_HEADER: &str = "## Conversation Summary (Earlier Context)";

/// True for the system message that holds an auto-summarization summary.
pub fn is_context_summary(message: &Message) -> bool {
    message.role == Role::System
        && message
            .content
            .to_summary_text()
            .starts_with(CONTEXT_SUMMARY_HEADER)
}

/// Evict oldest groups, then shrink single oversized messages, until the view fits.
///
/// Protected from eviction: the leading system prompt, summary messages, the
/// first user task (the anchor), and the whole current turn (the newest user
/// message and everything after it). Within the protected set, older tool
/// groups of the current turn may still be evicted as a last resort; the
/// newest group and the request itself never are.
fn fit_to_budget(plan: &mut ContextViewPlan, budget: usize) {
    let mut total = estimate_tokens_for_slice(&plan.messages);
    if total <= budget {
        return;
    }

    let mut groups = partition_groups(&plan.messages);
    let last_index = groups.len().saturating_sub(1);
    let is_user_group = |group: &MessageGroup| {
        group
            .messages
            .iter()
            .any(|message| message.role == Role::User)
    };
    // The first user group carries the task and its constraints. Keep it as an
    // anchor even when it predates the recent-window cutoff.
    let anchor_index = groups.iter().position(is_user_group);
    // The newest user group is the current request; the turn runs from there.
    let request_index = groups.iter().rposition(is_user_group);
    let turn_start = request_index.unwrap_or(last_index);
    let pinned = |index: usize, group: &MessageGroup| {
        // Only the leading system prompt and summaries are pinned. Sessions
        // saved before the prompt was refreshed in place carry one stale
        // harness prompt per turn; those are evictable like any old group.
        let pinned_system = group.kind == GroupKind::System
            && (index == 0 || group.messages.iter().any(is_context_summary));
        pinned_system || anchor_index == Some(index)
    };
    let mut evicted = 0usize;

    // Pass 1: drop older groups, cheap ones (not `must_keep`) before the ones
    // that carry constraints, failures, or edits. The last sweep trims older
    // tool groups inside the current turn, keeping the request and newest group.
    let sweeps = [
        (true, 0..turn_start),
        (false, 0..turn_start),
        (false, turn_start..last_index),
    ];
    for (skip_must_keep, range) in sweeps {
        for index in range {
            if total <= budget {
                break;
            }
            let group = &groups[index];
            if group.messages.is_empty()
                || pinned(index, group)
                || request_index == Some(index)
                || (skip_must_keep && group.must_keep)
            {
                continue;
            }
            total -= tokens_of(std::slice::from_ref(group));
            groups[index].messages.clear();
            evicted += 1;
        }
    }

    // Pass 2: surviving groups are individually oversized; shrink their text.
    // The current request is never truncated: a truncated prompt is worse than
    // a provider-side over-budget error the user can see and act on.
    let request = request_index.unwrap_or(last_index);
    if total > budget {
        shrink_oversized(&mut groups, budget, request);
    }

    if evicted > 0 {
        prepend_eviction_note(&mut groups, evicted);
        // The note itself costs tokens; make sure it did not push the view
        // back over the budget.
        if tokens_of(&groups) > budget {
            shrink_oversized(&mut groups, budget, request);
        }
    }

    let mut flattened: Vec<Message> = groups
        .into_iter()
        .filter(|g| !g.messages.is_empty())
        .flat_map(|g| g.messages)
        .collect();
    flattened = drop_orphaned_tool_results(flattened);

    plan.report.retained_groups = count_live_groups(&flattened);
    plan.report.evicted_groups = evicted;
    plan.messages = flattened;
    plan.report.tokens_after = estimate_tokens_for_slice(&plan.messages);
}

fn tokens_of(groups: &[MessageGroup]) -> usize {
    groups
        .iter()
        .flat_map(|g| g.messages.iter())
        .map(estimate_tokens)
        .sum()
}

fn count_live_groups(messages: &[Message]) -> usize {
    partition_groups(messages)
        .into_iter()
        .filter(|g| !g.messages.is_empty())
        .count()
}

/// Replace a message's text while keeping any image parts it carries.
fn replace_text(message: &mut Message, text: String) {
    message.content = match &message.content {
        MessageContent::Parts(parts) if message.content.has_image_parts() => {
            let mut rebuilt = vec![ContentPart::Text { text }];
            rebuilt.extend(
                parts
                    .iter()
                    .filter(|part| matches!(part, ContentPart::Image { .. }))
                    .cloned(),
            );
            MessageContent::Parts(rebuilt)
        }
        _ => MessageContent::Text(text),
    };
}

/// Tell the model an eviction happened, so it re-reads instead of assuming.
fn prepend_eviction_note(groups: &mut [MessageGroup], evicted: usize) {
    let Some(target) = groups
        .iter_mut()
        .find(|g| g.kind != GroupKind::System && g.messages.iter().any(|m| m.role == Role::User))
    else {
        return;
    };
    let Some(message) = target.messages.iter_mut().find(|m| m.role == Role::User) else {
        return;
    };
    let existing = message_text(message);
    replace_text(
        message,
        format!(
            "[nca context note: {evicted} older message group(s) were dropped to fit the model context window. Re-read files if you need their contents.]\n\n{existing}"
        ),
    );
}

/// Shrink the largest retained messages until the total fits, skipping the
/// current request group at `protected_index`.
///
/// Every iteration either strictly shrinks a message or retires it, so the
/// loop always ends. A message can stop shrinking before the floor: tool-call
/// overhead is not text, and a cut message keeps its marker.
fn shrink_oversized(groups: &mut [MessageGroup], budget: usize, protected_index: usize) {
    let mut total = tokens_of(groups);
    let mut exhausted: HashSet<(usize, usize)> = HashSet::new();
    while total > budget {
        let mut largest: Option<(usize, usize, usize)> = None;
        for (gi, group) in groups.iter().enumerate() {
            if gi == protected_index {
                continue;
            }
            for (mi, message) in group.messages.iter().enumerate() {
                let tokens = estimate_tokens(message);
                if tokens > 64
                    && !exhausted.contains(&(gi, mi))
                    && largest.is_none_or(|(_, _, t)| tokens > t)
                {
                    largest = Some((gi, mi, tokens));
                }
            }
        }
        let Some((gi, mi, tokens)) = largest else {
            return;
        };

        let message = &mut groups[gi].messages[mi];
        let text = message_text(message);
        let keep = (text.chars().count() * budget).div_ceil(total).max(200);
        replace_text(message, truncate_text(&text, keep));
        // Truncation must actually shrink; otherwise fall back to a hard cut.
        if estimate_tokens(message) >= tokens {
            replace_text(message, truncate_text(&text, 200));
        }
        let after = estimate_tokens(message);
        if after >= tokens {
            // Nothing left to cut here; restore it and stop considering it.
            replace_text(message, text);
            exhausted.insert((gi, mi));
            continue;
        }
        total -= tokens - after;
    }
}

/// Remove tool results whose originating call is no longer present.
fn drop_orphaned_tool_results(messages: Vec<Message>) -> Vec<Message> {
    let mut call_ids = HashSet::new();
    for message in &messages {
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                call_ids.insert(call.id.clone());
            }
        }
    }
    messages
        .into_iter()
        .filter(|m| {
            m.role != Role::Tool
                || m.tool_call_id
                    .as_ref()
                    .is_some_and(|id| call_ids.contains(id))
        })
        .collect()
}

fn plan_context_view_compacted(messages: &[Message], mode: SmartCompactionMode) -> ContextViewPlan {
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
                evicted_groups: 0,
                token_budget: None,
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
            evicted_groups: 0,
            token_budget: None,
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
    fn fitted_view_evicts_old_groups_and_fits_budget() {
        let mut messages = vec![Message::system("sys")];
        for i in 0..40 {
            messages.push(Message::user(format!("turn {i}")));
            messages.push(Message::assistant("a".repeat(2_000)));
        }
        messages.push(Message::user("final request"));

        let budget = NonZeroUsize::new(2_000).unwrap();
        let plan = plan_context_view_fitted(&messages, SmartCompactionMode::On, Some(budget));

        assert!(plan.report.evicted_groups > 0, "expected evictions");
        assert!(
            plan.report.tokens_after <= budget.get(),
            "tokens_after={} budget={}",
            plan.report.tokens_after,
            budget.get()
        );
        assert_eq!(plan.report.token_budget, Some(budget.get()));
        assert_eq!(orphaned_tool_results(&plan.messages), 0);
        // The newest request must survive verbatim.
        assert!(
            plan.messages
                .iter()
                .any(|m| message_text(m) == "final request")
        );
        // The model is told that history was dropped.
        assert!(
            plan.messages
                .iter()
                .any(|m| message_text(m).contains("nca context note"))
        );
        // Canonical history is untouched.
        assert_eq!(messages.len(), 82);
    }

    #[test]
    fn fitted_view_keeps_tool_groups_atomic() {
        let mut messages = vec![Message::system("sys")];
        for i in 0..30 {
            messages.push(Message::user(format!("inspect {i}")));
            messages.extend(tool_group(
                &format!("r{i}"),
                "read_file",
                &"line
"
                .repeat(400),
            ));
            messages.push(Message::assistant(format!("done {i}")));
        }
        messages.push(Message::user("final"));

        let plan =
            plan_context_view_fitted(&messages, SmartCompactionMode::On, NonZeroUsize::new(3_000));
        assert!(plan.report.evicted_groups > 0);
        assert_eq!(orphaned_tool_results(&plan.messages), 0);
        // Every surviving tool result still has its assistant call.
        for msg in &plan.messages {
            if msg.role == Role::Tool {
                let id = msg.tool_call_id.as_deref().unwrap();
                assert!(plan.messages.iter().any(|m| {
                    m.tool_calls
                        .as_ref()
                        .is_some_and(|calls| calls.iter().any(|c| c.id == id))
                }));
            }
        }
    }

    #[test]
    fn fitted_view_keeps_the_first_task_anchor() {
        let mut messages = vec![Message::system("sys")];
        messages.push(Message::user(
            "Build the importer and keep the public API stable.",
        ));
        for i in 0..30 {
            messages.push(Message::user(format!("follow-up {i}")));
            messages.push(Message::assistant("a".repeat(1_000)));
        }
        messages.push(Message::user("final request"));

        let plan =
            plan_context_view_fitted(&messages, SmartCompactionMode::On, NonZeroUsize::new(2_000));

        assert!(plan.report.evicted_groups > 0);
        assert!(
            plan.messages.iter().any(|message| {
                message.role == Role::User
                    && message_text(message)
                        .contains("Build the importer and keep the public API stable.")
            }),
            "the first user task must survive eviction"
        );
    }

    #[test]
    fn fitted_view_never_truncates_the_current_request() {
        let mut messages = vec![Message::system("sys")];
        for i in 0..10 {
            messages.push(Message::user(format!("old {i}")));
            messages.push(Message::assistant("a".repeat(2_000)));
        }
        let request = format!("please review this exact text: {}", "z".repeat(5_000));
        messages.push(Message::user(request.clone()));

        let plan =
            plan_context_view_fitted(&messages, SmartCompactionMode::On, NonZeroUsize::new(500));

        assert_eq!(
            plan.messages
                .last()
                .map(message_text)
                .expect("current request"),
            request,
            "the current request is never truncated to fit the budget"
        );
    }

    #[test]
    fn fitted_view_shrinks_one_oversized_message() {
        let messages = vec![
            Message::system("x".repeat(120_000)),
            Message::user("final request"),
        ];
        let budget = NonZeroUsize::new(1_500).unwrap();

        let plan = plan_context_view_fitted(&messages, SmartCompactionMode::On, Some(budget));
        assert!(
            plan.report.tokens_after <= budget.get(),
            "tokens_after={}",
            plan.report.tokens_after
        );
        assert!(plan.messages.len() >= 2, "system + newest group survive");
        assert_eq!(
            plan.messages.last().map(message_text).unwrap(),
            "final request"
        );
    }

    #[test]
    fn fitted_view_without_budget_matches_compaction() {
        let messages = vec![Message::system("sys"), Message::user("hello")];
        let plain = plan_context_view(&messages, SmartCompactionMode::On);
        let fitted = plan_context_view_fitted(&messages, SmartCompactionMode::On, None);
        assert_eq!(plain.messages, fitted.messages);
        assert_eq!(plain.report, fitted.report);
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

    #[test]
    fn fitted_view_terminates_when_nothing_can_shrink_further() {
        // A pinned system prompt that is still over the shrink floor after a
        // cut, plus tool-call overhead that no text cut can remove, used to
        // make `shrink_oversized` spin forever.
        let calls = (0..3)
            .map(|i| MessageToolCall {
                id: format!("c{i}"),
                name: "execute_bash".into(),
                arguments: json!({"command": "ls"}),
            })
            .collect();
        let mut messages = vec![
            Message::system("s".repeat(239)),
            Message::user("task"),
            Message::assistant_with_tool_calls("", calls),
        ];
        for i in 0..3 {
            messages.push(Message::tool(format!("c{i}"), "ok"));
        }
        messages.push(Message::user("x".repeat(3_000)));

        let plan =
            plan_context_view_fitted(&messages, SmartCompactionMode::On, NonZeroUsize::new(100));

        assert!(plan.messages.iter().any(|m| m.role == Role::System));
    }

    #[test]
    fn fitted_view_keeps_the_current_request_during_a_tool_loop() {
        let mut messages = vec![Message::system("sys"), Message::user("task #1")];
        for i in 0..20 {
            messages.push(Message::assistant(format!(
                "old reply {i} {}",
                "a".repeat(1_000)
            )));
        }
        messages.push(Message::user("now fix the tests"));
        for i in 0..12 {
            messages.extend(tool_group(
                &format!("t{i}"),
                "read_file",
                &"r".repeat(1_500),
            ));
        }

        let plan =
            plan_context_view_fitted(&messages, SmartCompactionMode::On, NonZeroUsize::new(1_500));

        assert!(
            plan.messages
                .iter()
                .any(|m| m.role == Role::User && message_text(m) == "now fix the tests"),
            "the user's current request must survive tool-loop eviction"
        );
        let last_call = plan
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Tool)
            .and_then(|m| m.tool_call_id.clone());
        assert_eq!(last_call.as_deref(), Some("t11"));
        assert_eq!(orphaned_tool_results(&plan.messages), 0);
    }

    #[test]
    fn fitted_view_keeps_images_when_it_adds_the_eviction_note() {
        let mut messages = vec![
            Message::system("sys"),
            Message::user_with_parts(vec![
                ContentPart::Text {
                    text: "fix the layout in this screenshot".into(),
                },
                ContentPart::Image {
                    media_type: "image/png".into(),
                    path: ".nca/images/shot.png".into(),
                },
            ]),
        ];
        for i in 0..10 {
            messages.push(Message::user(format!("old {i}")));
            messages.push(Message::assistant("a".repeat(2_000)));
        }
        messages.push(Message::user("and now?"));

        let plan =
            plan_context_view_fitted(&messages, SmartCompactionMode::On, NonZeroUsize::new(800));

        assert!(plan.report.evicted_groups > 0);
        let anchor = plan
            .messages
            .iter()
            .find(|m| m.role == Role::User)
            .expect("anchor");
        assert!(message_text(anchor).starts_with("[nca context note:"));
        assert!(anchor.content.has_image_parts(), "image parts survive");
    }

    #[test]
    fn fitted_view_evicts_stale_system_prompts_from_old_sessions() {
        // Sessions saved before the prompt was refreshed in place carry one
        // harness prompt per turn.
        let mut messages = vec![Message::system("current harness"), Message::user("task")];
        for i in 0..30 {
            messages.push(Message::system(format!(
                "stale harness {i} {}",
                "h".repeat(800)
            )));
            messages.push(Message::user(format!("q{i}")));
            messages.push(Message::assistant(format!("a{i}")));
        }
        messages.push(Message::system(format!(
            "{CONTEXT_SUMMARY_HEADER}\n\nwe fixed the parser"
        )));
        messages.push(Message::user("next"));

        let plan =
            plan_context_view_fitted(&messages, SmartCompactionMode::On, NonZeroUsize::new(1_000));

        assert!(plan.report.tokens_after <= 1_000);
        assert_eq!(message_text(&plan.messages[0]), "current harness");
        assert!(plan.messages.iter().any(is_context_summary));
    }

    #[test]
    fn token_estimate_counts_tool_call_arguments() {
        let small = Message::assistant_with_tool_calls(
            "",
            vec![MessageToolCall {
                id: "w".into(),
                name: "write_file".into(),
                arguments: json!({"path": "a.html", "content": "x"}),
            }],
        );
        let large = Message::assistant_with_tool_calls(
            "",
            vec![MessageToolCall {
                id: "w".into(),
                name: "write_file".into(),
                arguments: json!({"path": "a.html", "content": "x".repeat(40_000)}),
            }],
        );
        assert!(estimate_tokens(&large) >= estimate_tokens(&small) + 9_000);
    }
}
