//! Context management for preventing context window overflow.
//!
//! This module provides:
//! - Token counting and context size tracking
//! - Message windowing (sliding window of recent messages)
//! - Automatic summarization when context approaches limits
//! - Preservation of critical messages (system prompt, memory, etc.)

use nca_common::message::{Message, MessageContent, Role};
use serde::{Deserialize, Serialize};

/// Configuration for context management behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextManagerConfig {
    /// Target context size in tokens (rough approximation).
    /// When context exceeds this, we'll start compacting.
    pub context_window_target: usize,
    /// Maximum messages to retain after compaction.
    /// This is the "sliding window" size for recent conversation.
    pub max_retained_messages: usize,
    /// Percentage (0-100) of context window that triggers auto-summarize.
    /// E.g., 75 means start summarizing when at 75% of context_window_target.
    pub auto_summarize_threshold: u8,
    /// Enable automatic context summarization.
    pub enable_auto_summarize: bool,
    /// Maximum tokens per message before truncation during summary.
    pub max_message_chars_for_summary: usize,
}

impl Default for ContextManagerConfig {
    fn default() -> Self {
        Self {
            // Default to ~32k tokens target (rough: chars / 4 ≈ tokens)
            context_window_target: 32_000,
            // Keep last 50 messages + system + memory
            max_retained_messages: 50,
            // Start summarizing at 75% of target
            auto_summarize_threshold: 75,
            enable_auto_summarize: true,
            // Truncate very long messages when preparing for summary
            max_message_chars_for_summary: 10_000,
        }
    }
}

/// Context statistics exposed via the CLI/API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextStats {
    /// Model name
    pub model: String,
    /// Model context window in tokens
    pub context_window: usize,
    /// Estimated total tokens in context
    pub estimated_tokens: usize,
    /// Number of messages in context
    pub message_count: usize,
    /// Percentage of context window used (0-100)
    pub usage_percent: u8,
    /// Whether context is approaching limit
    pub needs_attention: bool,
    /// Whether auto-summarize should trigger
    pub should_summarize: bool,
}

/// Manages conversation context to prevent token overflow.
///
/// The context manager maintains:
/// - A sliding window of recent user/assistant messages
/// - Preservation of system messages (always at start)
/// - Track tool messages that follow the relevant turns
pub struct ContextManager {
    config: ContextManagerConfig,
    model: String,
}

impl ContextManager {
    pub fn new(config: ContextManagerConfig, model: String) -> Self {
        Self { config, model }
    }

    pub fn with_default_config(model: String) -> Self {
        Self::new(ContextManagerConfig::default(), model)
    }

    /// Calculate estimated token count for a message.
    /// Uses a rough approximation: tokens ≈ characters / 4
    pub fn estimate_tokens(message: &Message) -> usize {
        // Tool messages tend to be more token-dense
        let divisor = match message.role {
            Role::Tool => 3.5,
            Role::System => 4.0,
            _ => 4.0,
        };

        let content_tokens = message.content.approx_chars() as f64 / divisor;

        // Add overhead for tool calls
        let tool_call_overhead = message
            .tool_calls
            .as_ref()
            .map(|calls| calls.len() * 50) // ~50 tokens per tool call structure
            .unwrap_or(0);

        (content_tokens as usize) + tool_call_overhead + 10 // ~10 tokens base overhead
    }

    /// Calculate estimated token count for a slice of messages.
    pub fn estimate_tokens_for_slice(messages: &[Message]) -> usize {
        messages.iter().map(Self::estimate_tokens).sum()
    }

    /// Get current context statistics.
    pub fn stats(&self, messages: &[Message]) -> ContextStats {
        let estimated_tokens = Self::estimate_tokens_for_slice(messages);
        let message_count = messages.len();

        let context_window = self.config.context_window_target.max(1);
        let usage_percent =
            ((estimated_tokens as f64 / context_window as f64) * 100.0).min(100.0) as u8;

        let needs_attention = usage_percent >= 80;
        let should_summarize = self.config.enable_auto_summarize
            && usage_percent >= self.config.auto_summarize_threshold;

        ContextStats {
            model: self.model.clone(),
            context_window,
            estimated_tokens,
            message_count,
            usage_percent,
            needs_attention,
            should_summarize,
        }
    }

    /// Count system messages at the start of the list.
    /// System messages should always be preserved.
    fn count_system_messages(messages: &[Message]) -> usize {
        messages
            .iter()
            .take_while(|m| m.role == Role::System)
            .count()
    }

    /// Adjust a cutoff index so it never lands inside a tool_use/tool_result group.
    /// If the message at `cutoff` is a Role::Tool, walk backwards to include the
    /// preceding assistant message that contains the matching tool_calls.
    /// `min_cutoff` prevents walking past system messages.
    fn adjust_cutoff_for_tool_groups(
        messages: &[Message],
        mut cutoff: usize,
        min_cutoff: usize,
    ) -> usize {
        while cutoff > min_cutoff && cutoff < messages.len() && messages[cutoff].role == Role::Tool
        {
            cutoff -= 1;
        }
        cutoff
    }

    /// Check if the context needs compaction.
    pub fn needs_compaction(&self, messages: &[Message]) -> bool {
        let stats = self.stats(messages);
        stats.should_summarize || stats.needs_attention
    }

    /// Compact messages using a sliding window strategy.
    /// Returns the indices of messages to keep.
    ///
    /// Strategy:
    /// 1. Always keep all system messages (at start)
    /// 2. Keep recent messages up to max_retained_messages
    /// 3. Mark older messages for summarization
    pub fn get_compaction_plan(&self, messages: &[Message], system_count: usize) -> CompactionPlan {
        // Calculate how many non-system messages we can keep
        let non_system_count = messages.len().saturating_sub(system_count);
        let keep_non_system = non_system_count.saturating_sub(self.config.max_retained_messages);

        // If we need to compact, find the boundary
        if keep_non_system > 0 {
            // Keep last max_retained_messages non-system messages
            let cutoff_index = messages.len() - self.config.max_retained_messages;

            CompactionPlan {
                keep_indices: (0..cutoff_index)
                    .rev()
                    .take(self.config.max_retained_messages)
                    .collect(),
                summarize_range: Some(SummarizeRange {
                    start: system_count,
                    end: cutoff_index,
                }),
                preserve_system: true,
            }
        } else {
            CompactionPlan {
                keep_indices: (0..messages.len()).collect(),
                summarize_range: None,
                preserve_system: true,
            }
        }
    }

    /// Get messages that should be summarized.
    /// These are the older messages that will be replaced by a summary.
    pub fn get_messages_to_summarize(&self, messages: &[Message]) -> Vec<Message> {
        let system_count = Self::count_system_messages(messages);
        let plan = self.get_compaction_plan(messages, system_count);

        if let Some(range) = plan.summarize_range {
            // Skip system messages, get the middle-old messages
            let start = (range.start).max(system_count);

            messages[start..range.end].to_vec()
        } else {
            Vec::new()
        }
    }

    /// Apply a summary to the context, replacing old messages.
    /// Returns the new message list with the summary inserted.
    pub fn apply_summary(&self, messages: &[Message], summary: &str) -> Vec<Message> {
        let system_count = Self::count_system_messages(messages);
        let plan = self.get_compaction_plan(messages, system_count);

        // If there's nothing to summarize, just return the original messages
        let Some(range) = plan.summarize_range else {
            return messages.to_vec();
        };

        // Get recent messages to keep (everything after the summarize range).
        // Adjust to avoid splitting tool_use/tool_result groups.
        let recent_start = Self::adjust_cutoff_for_tool_groups(messages, range.end, system_count);

        // Build new message list: system + summary + recent
        let mut result = Vec::with_capacity(system_count + 10);

        // Add system messages
        result.extend(messages.iter().take(system_count).cloned());

        // Insert summary as a special system message
        if !summary.trim().is_empty() {
            result.push(Message {
                role: Role::System,
                content: MessageContent::Text(format!(
                    "## Conversation Summary (Earlier Context)\n\n{}",
                    summary
                )),
                tool_call_id: None,
                tool_calls: None,
                reasoning_content: None,
            });
        }

        // Add recent messages
        result.extend_from_slice(&messages[recent_start..]);

        result
    }

    /// Get a sliding window of recent messages for context.
    /// Preserves system messages and keeps recent conversation.
    /// Ensures tool_use/tool_result groups are never split.
    pub fn get_sliding_window(
        &self,
        messages: &[Message],
        max_messages: Option<usize>,
    ) -> Vec<Message> {
        let max = max_messages.unwrap_or(self.config.max_retained_messages);
        let system_count = Self::count_system_messages(messages);

        if messages.len() <= max {
            return messages.to_vec();
        }

        // Keep system messages + last (max - system_count) messages
        let keep_count = max.saturating_sub(system_count);
        let cutoff = messages.len() - keep_count;
        let cutoff = Self::adjust_cutoff_for_tool_groups(messages, cutoff, system_count);

        let mut result: Vec<Message> = messages[..system_count].to_vec();
        result.extend_from_slice(&messages[cutoff..]);

        result
    }

    /// Truncate very long messages for summary generation.
    pub fn prepare_for_summary(&self, messages: &[Message]) -> Vec<Message> {
        messages
            .iter()
            .map(|m| {
                if m.content.approx_chars() > self.config.max_message_chars_for_summary {
                    let truncated: String = m
                        .content
                        .to_summary_text()
                        .chars()
                        .take(self.config.max_message_chars_for_summary)
                        .collect();
                    Message {
                        role: m.role.clone(),
                        content: MessageContent::Text(format!("{}...[truncated]", truncated)),
                        tool_call_id: m.tool_call_id.clone(),
                        tool_calls: m.tool_calls.clone(),
                        reasoning_content: m.reasoning_content.clone(),
                    }
                } else {
                    m.clone()
                }
            })
            .collect()
    }

    /// Generate a prompt for the AI to summarize conversation.
    pub fn summary_prompt(&self, messages: &[Message]) -> String {
        let prepared = self.prepare_for_summary(messages);
        let stats = self.stats(&prepared);

        format!(
            r#"Please summarize the following conversation concisely.

The summary should:
1. Capture the key topics and goals discussed
2. Note any important decisions or findings
3. Preserve critical context (file paths, variable names, errors, etc.)
4. Be written as if you're continuing the conversation

Keep the summary under 500 words.

Current context stats:
- Messages: {}
- Estimated tokens: {} (target: {})

---

Conversation to summarize:

"#,
            stats.message_count, stats.estimated_tokens, self.config.context_window_target
        )
    }

    /// Get the config.
    pub fn config(&self) -> &ContextManagerConfig {
        &self.config
    }

    /// Update the config.
    pub fn set_config(&mut self, config: ContextManagerConfig) {
        self.config = config;
    }
}

/// Plan for compacting messages.
#[derive(Debug, Clone)]
pub struct CompactionPlan {
    /// Indices of messages to keep
    pub keep_indices: Vec<usize>,
    /// Range of messages to summarize (if any)
    pub summarize_range: Option<SummarizeRange>,
    /// Whether to preserve system messages
    pub preserve_system: bool,
}

/// Range of messages to summarize.
#[derive(Debug, Clone)]
pub struct SummarizeRange {
    pub start: usize,
    pub end: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_message(role: Role, content: &str) -> Message {
        Message {
            role,
            content: MessageContent::Text(content.to_string()),
            tool_call_id: None,
            tool_calls: None,
            reasoning_content: None,
        }
    }

    #[test]
    fn test_estimate_tokens() {
        let msg = make_message(Role::User, "Hello, world!");
        assert!(ContextManager::estimate_tokens(&msg) > 0);
    }

    #[test]
    fn test_stats() {
        let config = ContextManagerConfig {
            context_window_target: 1000,
            max_retained_messages: 10,
            auto_summarize_threshold: 50,
            enable_auto_summarize: true,
            max_message_chars_for_summary: 1000,
        };
        let manager = ContextManager::new(config, "test-model".to_string());

        let messages = vec![
            make_message(Role::System, "You are a helpful assistant."),
            make_message(Role::User, "Hello"),
            make_message(Role::Assistant, "Hi there!"),
        ];

        let stats = manager.stats(&messages);
        assert!(stats.message_count == 3);
        assert!(stats.estimated_tokens > 0);
        assert_eq!(stats.model, "test-model");
        assert_eq!(stats.context_window, 1000); // stats use configured target
    }

    #[test]
    fn test_sliding_window() {
        let config = ContextManagerConfig {
            context_window_target: 32000,
            max_retained_messages: 3,
            auto_summarize_threshold: 75,
            enable_auto_summarize: true,
            max_message_chars_for_summary: 10000,
        };
        let manager = ContextManager::new(config, "test-model".to_string());

        let messages: Vec<Message> = (0..10)
            .map(|i| make_message(Role::User, &format!("Message {}", i)))
            .collect();

        let window = manager.get_sliding_window(&messages, None);
        // System messages + last 3
        assert!(window.len() <= 3);
    }

    #[test]
    fn test_apply_summary() {
        // Use a config that will trigger compaction for 5 messages
        let config = ContextManagerConfig {
            context_window_target: 32_000,
            max_retained_messages: 2, // Only keep 2 messages, forcing compaction
            auto_summarize_threshold: 75,
            enable_auto_summarize: true,
            max_message_chars_for_summary: 10_000,
        };
        let manager = ContextManager::new(config, "test-model".to_string());

        let messages = vec![
            make_message(Role::System, "System"),
            make_message(Role::User, "Hello"),
            make_message(Role::Assistant, "Hi"),
            make_message(Role::User, "How are you?"),
            make_message(Role::Assistant, "I'm fine!"),
        ];

        let summary = "User greeted the assistant and asked how it was doing.";
        let result = manager.apply_summary(&messages, summary);

        // Should have system + summary + recent messages (2)
        // Note: The compaction plan keeps last 2 non-system messages
        assert!(
            result.len() >= 2,
            "Expected at least 2 messages, got {}",
            result.len()
        );
        // The summary should be in a system message
        assert!(
            result
                .iter()
                .any(|m| m.content.to_summary_text().contains("Conversation Summary"))
        );
    }

    use nca_common::message::MessageToolCall;

    fn make_tool_call(id: &str, name: &str) -> MessageToolCall {
        MessageToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }
    }

    /// Helper: assert no Role::Tool message appears without its matching
    /// assistant tool_use in the preceding assistant message.
    fn assert_no_orphaned_tool_results(messages: &[Message]) {
        let mut expected_tool_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for msg in messages {
            match msg.role {
                Role::Assistant => {
                    expected_tool_ids.clear();
                    if let Some(calls) = &msg.tool_calls {
                        for call in calls {
                            expected_tool_ids.insert(call.id.clone());
                        }
                    }
                }
                Role::Tool => {
                    let id = msg.tool_call_id.as_deref().unwrap_or("");
                    assert!(
                        expected_tool_ids.contains(id),
                        "Orphaned tool_result with id '{}' — no matching tool_use in preceding assistant message. Messages: {:?}",
                        id,
                        messages
                            .iter()
                            .map(|m| (&m.role, m.tool_call_id.as_deref()))
                            .collect::<Vec<_>>()
                    );
                }
                _ => {
                    expected_tool_ids.clear();
                }
            }
        }
    }

    #[test]
    fn sliding_window_preserves_tool_use_result_pairs() {
        // max_retained=4: cutoff would land on tool result msg if naive
        let config = ContextManagerConfig {
            context_window_target: 32_000,
            max_retained_messages: 4,
            auto_summarize_threshold: 75,
            enable_auto_summarize: true,
            max_message_chars_for_summary: 10_000,
        };
        let manager = ContextManager::new(config, "test-model".to_string());

        // 10 messages: user, assistant+tool_calls, tool_result, user, ...
        let messages = vec![
            make_message(Role::System, "system prompt"),
            make_message(Role::User, "msg 1"),
            make_message(Role::Assistant, "reply 1"),
            make_message(Role::User, "msg 2"),
            // This is the critical group: assistant calls tool, then tool result
            Message::assistant_with_tool_calls(
                "Let me check",
                vec![make_tool_call("call-1", "read_file")],
            ),
            Message::tool("call-1", "file contents here"),
            // After tool result
            make_message(Role::User, "msg 3"),
            make_message(Role::Assistant, "final reply"),
        ];

        let window = manager.get_sliding_window(&messages, None);

        // The window must never contain an orphaned tool_result
        assert_no_orphaned_tool_results(&window);
    }

    #[test]
    fn apply_summary_preserves_tool_use_result_pairs() {
        // max_retained=3: cutoff lands right on the tool_result
        let config = ContextManagerConfig {
            context_window_target: 32_000,
            max_retained_messages: 3,
            auto_summarize_threshold: 75,
            enable_auto_summarize: true,
            max_message_chars_for_summary: 10_000,
        };
        let manager = ContextManager::new(config, "test-model".to_string());

        let messages = vec![
            make_message(Role::System, "system prompt"),
            make_message(Role::User, "msg 1"),
            make_message(Role::Assistant, "reply 1"),
            make_message(Role::User, "msg 2"),
            // Tool use group that straddles the naive cutoff
            Message::assistant_with_tool_calls(
                "Running tool",
                vec![make_tool_call("call-2", "bash")],
            ),
            Message::tool("call-2", "ok"),
            make_message(Role::User, "msg 3"),
            make_message(Role::Assistant, "done"),
        ];

        let result = manager.apply_summary(&messages, "Earlier conversation summary.");

        assert_no_orphaned_tool_results(&result);
    }

    #[test]
    fn sliding_window_preserves_multi_tool_call_group() {
        // Assistant calls 2 tools → 2 tool results. Window must keep entire group.
        let config = ContextManagerConfig {
            context_window_target: 32_000,
            max_retained_messages: 4,
            auto_summarize_threshold: 75,
            enable_auto_summarize: true,
            max_message_chars_for_summary: 10_000,
        };
        let manager = ContextManager::new(config, "test-model".to_string());

        let messages = vec![
            make_message(Role::System, "system prompt"),
            make_message(Role::User, "msg 1"),
            make_message(Role::Assistant, "reply 1"),
            make_message(Role::User, "msg 2"),
            Message::assistant_with_tool_calls(
                "Using two tools",
                vec![
                    make_tool_call("call-a", "read_file"),
                    make_tool_call("call-b", "grep"),
                ],
            ),
            Message::tool("call-a", "contents a"),
            Message::tool("call-b", "contents b"),
            make_message(Role::User, "msg 3"),
            make_message(Role::Assistant, "all done"),
        ];

        let window = manager.get_sliding_window(&messages, None);

        assert_no_orphaned_tool_results(&window);
    }
}
