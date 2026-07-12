//! Default chat composer key handling (subset — orchestrator keeps global shortcuts).

use crate::tui::state::TuiSessionState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKeyResult {
    Handled,
    OpenPalette,
    NotHandled,
}

pub fn handle_chat_key(state: &mut TuiSessionState, key: KeyEvent) -> ChatKeyResult {
    match (key.code, key.modifiers) {
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            state.open_command_palette();
            ChatKeyResult::OpenPalette
        }
        (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
            state.blocks.clear();
            state.streaming_assistant = None;
            state.scroll_lines = 0;
            state.transcript_follow_tail = true;
            state.mark_transcript_dirty();
            ChatKeyResult::Handled
        }
        (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
            state.leader_pending = true;
            ChatKeyResult::Handled
        }
        _ => ChatKeyResult::NotHandled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ctrl_p_opens_palette() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        assert_eq!(
            handle_chat_key(
                &mut st,
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)
            ),
            ChatKeyResult::OpenPalette
        );
        assert!(st.command_palette_open());
    }
}
