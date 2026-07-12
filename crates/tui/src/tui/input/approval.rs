//! Approval + inline question answer parsing for the composer.

use crate::tui::state::TuiSessionState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nca_common::event::{InteractiveQuestionPayload, QuestionSelection};
use nca_core::approval::suggest_allow_pattern;

/// Message from TUI to the approval dispatch task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalAnswer {
    Verdict { call_id: String, approved: bool },
    AllowPattern { call_id: String, pattern: String },
}

pub fn parse_tui_question_answer(
    raw: &str,
    q: &InteractiveQuestionPayload,
) -> Option<QuestionSelection> {
    let t = raw.trim();
    if t.is_empty() || t == "0" || t.eq_ignore_ascii_case("s") {
        return Some(QuestionSelection::Suggested);
    }
    if let Ok(n) = t.parse::<usize>()
        && n >= 1
        && n <= q.options.len()
    {
        return Some(QuestionSelection::Option {
            option_id: q.options[n - 1].id.clone(),
        });
    }
    if q.allow_custom && !t.is_empty() {
        return Some(QuestionSelection::Custom {
            text: t.to_string(),
        });
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalShortcutResult {
    Verdict { call_id: String, approved: bool },
    AllowPattern { call_id: String, pattern: String },
}

pub fn handle_approval_shortcut(
    state: &TuiSessionState,
    key: KeyEvent,
) -> Option<ApprovalShortcutResult> {
    let req = state.active_approval.as_ref()?;
    match (key.code, key.modifiers) {
        (KeyCode::Char('y'), KeyModifiers::CONTROL) => Some(ApprovalShortcutResult::Verdict {
            call_id: req.call_id.clone(),
            approved: true,
        }),
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => Some(ApprovalShortcutResult::Verdict {
            call_id: req.call_id.clone(),
            approved: false,
        }),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            let input_json: serde_json::Value =
                serde_json::from_str(&req.input).unwrap_or_default();
            let pattern = suggest_allow_pattern(&req.tool, &input_json);
            Some(ApprovalShortcutResult::AllowPattern {
                call_id: req.call_id.clone(),
                pattern,
            })
        }
        _ => None,
    }
}

/// Ctrl+shortcut approval handling (clears composer when triggered).
pub fn handle_approval_key(state: &mut TuiSessionState, key: KeyEvent) -> Option<ApprovalAnswer> {
    let shortcut = handle_approval_shortcut(state, key)?;
    state.input_buffer.clear();
    state.cursor_char_idx = 0;
    match shortcut {
        ApprovalShortcutResult::Verdict { call_id, approved } => {
            Some(ApprovalAnswer::Verdict { call_id, approved })
        }
        ApprovalShortcutResult::AllowPattern { call_id, pattern } => {
            state
                .blocks
                .push(crate::tui::state::DisplayBlock::System(format!(
                    "Always allowing: {pattern}"
                )));
            Some(ApprovalAnswer::AllowPattern { call_id, pattern })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_suggested_answer_aliases() {
        let q = InteractiveQuestionPayload {
            question_id: "q".into(),
            call_id: "c".into(),
            prompt: "p".into(),
            options: vec![],
            allow_custom: true,
            suggested_answer: "x".into(),
        };
        assert_eq!(
            parse_tui_question_answer("0", &q),
            Some(QuestionSelection::Suggested)
        );
    }
}
