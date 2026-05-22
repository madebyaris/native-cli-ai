//! Connect-provider modal input.

use crate::tui::TuiCmd;
use crate::tui::connect_modal::{
    build_connect_rows, clamp_selection, provider_at_selection, selectable_row_indices,
};
use crate::tui::state::TuiSessionState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nca_common::config::ProviderKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectModalKeyResult {
    Handled,
    PromptApiKey(ProviderKind),
}

pub fn handle_connect_modal_key(
    state: &mut TuiSessionState,
    key: KeyEvent,
) -> ConnectModalKeyResult {
    let rows = build_connect_rows(state.connect_search());
    let n_sel = selectable_row_indices(&rows).len();
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) if !state.onboarding_mode => {
            state.close_connect_modal();
            ConnectModalKeyResult::Handled
        }
        (KeyCode::Up, _) => {
            if let Some(idx) = state.connect_menu_index_mut()
                && n_sel > 0
            {
                *idx = (*idx).saturating_sub(1).min(n_sel - 1);
            }
            ConnectModalKeyResult::Handled
        }
        (KeyCode::Down, _) => {
            if let Some(idx) = state.connect_menu_index_mut()
                && n_sel > 0
            {
                *idx = (*idx + 1).min(n_sel - 1);
            }
            ConnectModalKeyResult::Handled
        }
        (KeyCode::Enter, _) => {
            if let Some(p) = provider_at_selection(&rows, state.connect_menu_index()) {
                state.close_connect_modal();
                ConnectModalKeyResult::PromptApiKey(p)
            } else {
                ConnectModalKeyResult::Handled
            }
        }
        (KeyCode::Backspace, _) => {
            if let Some(search) = state.connect_search_mut() {
                search.pop();
            }
            if let Some(idx) = state.connect_menu_index_mut() {
                *idx = 0;
            }
            if let Some(scroll) = state.connect_modal_scroll_mut() {
                *scroll = 0;
            }
            let rows2 = build_connect_rows(state.connect_search());
            if let Some(idx) = state.connect_menu_index_mut() {
                *idx = clamp_selection(*idx, &rows2);
            }
            ConnectModalKeyResult::Handled
        }
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            if let Some(search) = state.connect_search_mut() {
                search.push(c);
            }
            if let Some(idx) = state.connect_menu_index_mut() {
                *idx = 0;
            }
            if let Some(scroll) = state.connect_modal_scroll_mut() {
                *scroll = 0;
            }
            let rows2 = build_connect_rows(state.connect_search());
            if let Some(idx) = state.connect_menu_index_mut() {
                *idx = clamp_selection(*idx, &rows2);
            }
            ConnectModalKeyResult::Handled
        }
        _ => ConnectModalKeyResult::Handled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn esc_closes_when_not_onboarding() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.open_connect_modal();
        handle_connect_modal_key(&mut st, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!st.connect_modal_open());
    }
}
