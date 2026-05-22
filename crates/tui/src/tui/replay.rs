//! Replay persisted `*.events.jsonl` into `TuiSessionState` so resume shows prior transcript.

use crate::tui::state::TuiSessionState;
use nca_common::event::{AgentEvent, EventEnvelope};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};

fn parse_json_values_on_line(line: &str) -> Vec<Value> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let de = serde_json::Deserializer::from_str(trimmed);
    for res in de.into_iter::<Value>() {
        match res {
            Ok(v) => out.push(v),
            Err(_) => break,
        }
    }
    out
}

fn value_to_event(v: &Value) -> Option<AgentEvent> {
    if v.get("event").is_some() {
        serde_json::from_value::<EventEnvelope>(v.clone())
            .ok()
            .map(|e| e.event)
    } else {
        serde_json::from_value::<AgentEvent>(v.clone()).ok()
    }
}

/// Skip streaming deltas when replaying; final `MessageReceived` for assistant already has full text.
fn should_skip_on_replay(ev: &AgentEvent) -> bool {
    matches!(ev, AgentEvent::TokensStreamed { .. })
}

/// Load historical events from disk into TUI state (used on resume / reopen same log).
pub async fn replay_event_log_into_state(log_path: &Path, state: &Arc<Mutex<TuiSessionState>>) {
    let Ok(raw) = tokio::fs::read_to_string(log_path).await else {
        return;
    };

    let mut applied = 0u64;
    for line in raw.lines() {
        for v in parse_json_values_on_line(line) {
            let Some(ev) = value_to_event(&v) else {
                continue;
            };
            if should_skip_on_replay(&ev) {
                continue;
            }
            if let Ok(mut g) = state.lock() {
                g.apply_event(&ev);
                applied += 1;
            }
        }
    }

    if let Ok(mut g) = state.lock() {
        g.streaming_assistant = None;
        g.clear_replayed_interaction_state();
    }

    tracing::debug!(path = %log_path.display(), events_applied = applied, "replayed event log into TUI");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_envelope_and_bare_event() {
        let line = r#"{"id":1,"event":{"type":"MessageReceived","role":"user","content":"hi"}}"#;
        let vals = parse_json_values_on_line(line);
        assert_eq!(vals.len(), 1);
        let ev = value_to_event(&vals[0]).expect("event");
        assert!(matches!(ev, AgentEvent::MessageReceived { .. }));

        let line2 = r#"{"type":"MessageReceived","role":"assistant","content":"yo"}"#;
        let vals2 = parse_json_values_on_line(line2);
        let ev2 = value_to_event(&vals2[0]).expect("event");
        assert!(matches!(ev2, AgentEvent::MessageReceived { .. }));
    }

    #[test]
    fn parses_two_concatenated_json_objects() {
        let line = concat!(
            r#"{"type":"SessionEnded","reason":{"Completed":null}}"#,
            r#"{"type":"SessionStarted","session_id":"s","workspace":".","model":"m"}"#
        );
        let vals = parse_json_values_on_line(line);
        assert_eq!(vals.len(), 2);
    }

    #[test]
    fn skips_tokens_streamed_on_replay_flag() {
        assert!(should_skip_on_replay(&AgentEvent::TokensStreamed {
            delta: "x".into()
        }));
        assert!(!should_skip_on_replay(&AgentEvent::MessageReceived {
            role: "user".into(),
            content: "a".into(),
        }));
    }
}
