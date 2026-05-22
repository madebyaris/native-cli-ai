//! Question modal picker input.

use crate::tui::state::TuiSessionState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nca_common::event::QuestionSelection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionModalKeyResult {
    Handled,
    Answer(QuestionSelection),
    ChatAboutThis,
}

pub fn handle_question_modal_key(
    state: &mut TuiSessionState,
    key: KeyEvent,
) -> QuestionModalKeyResult {
    let Some(q) = state.active_question.clone() else {
        state.close_question_modal();
        return QuestionModalKeyResult::Handled;
    };
    let has_chat = q.allow_custom;
    let total = 1 + q.options.len() + usize::from(has_chat);
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            state.close_question_modal();
            QuestionModalKeyResult::ChatAboutThis
        }
        (KeyCode::Up, _) => {
            if let Some(idx) = state.question_modal_index_mut() {
                *idx = idx.saturating_sub(1);
            }
            QuestionModalKeyResult::Handled
        }
        (KeyCode::Down, _) => {
            if let Some(idx) = state.question_modal_index_mut() {
                *idx = (*idx + 1).min(total.saturating_sub(1));
            }
            QuestionModalKeyResult::Handled
        }
        (KeyCode::Enter, _) => {
            let idx = state.question_modal_index();
            state.close_question_modal();
            if idx == 0 {
                QuestionModalKeyResult::Answer(QuestionSelection::Suggested)
            } else if idx <= q.options.len() {
                QuestionModalKeyResult::Answer(QuestionSelection::Option {
                    option_id: q.options[idx - 1].id.clone(),
                })
            } else if has_chat {
                QuestionModalKeyResult::ChatAboutThis
            } else {
                QuestionModalKeyResult::Handled
            }
        }
        _ => QuestionModalKeyResult::Handled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::event::{InteractiveQuestionPayload, QuestionOption};
    use std::path::PathBuf;

    #[test]
    fn enter_on_suggested_returns_selection() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.active_question = Some(InteractiveQuestionPayload {
            question_id: "q".into(),
            call_id: "c".into(),
            prompt: "Pick".into(),
            options: vec![QuestionOption {
                id: "a".into(),
                label: "A".into(),
            }],
            allow_custom: true,
            suggested_answer: "A".into(),
        });
        st.open_question_modal();
        let result =
            handle_question_modal_key(&mut st, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            result,
            QuestionModalKeyResult::Answer(QuestionSelection::Suggested)
        );
    }
}
