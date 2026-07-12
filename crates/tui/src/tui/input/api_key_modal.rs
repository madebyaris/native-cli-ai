//! API key entry modal input.

use crate::tui::state::{OnboardingValidation, TuiSessionState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nca_common::config::ProviderKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyModalKeyResult {
    Handled,
    Validate(ProviderKind, String),
    SubmitEmpty,
    ReopenConnect,
}

pub fn handle_api_key_modal_key(
    state: &mut TuiSessionState,
    key: KeyEvent,
) -> ApiKeyModalKeyResult {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            state.close_api_key_modal();
            if state.onboarding_mode {
                state.open_connect_modal();
                ApiKeyModalKeyResult::ReopenConnect
            } else {
                ApiKeyModalKeyResult::Handled
            }
        }
        (KeyCode::Enter, _) => {
            if state.onboarding_mode {
                if matches!(
                    state.validation_status,
                    Some(OnboardingValidation::Validating)
                ) {
                    return ApiKeyModalKeyResult::Handled;
                }
                if let Some(provider) = state.api_key_target_provider() {
                    let key_text = state.api_key_input().trim().to_string();
                    if key_text.is_empty() {
                        ApiKeyModalKeyResult::Handled
                    } else {
                        state.validation_status = Some(OnboardingValidation::Validating);
                        ApiKeyModalKeyResult::Validate(provider, key_text)
                    }
                } else {
                    ApiKeyModalKeyResult::Handled
                }
            } else {
                ApiKeyModalKeyResult::SubmitEmpty
            }
        }
        (KeyCode::Backspace, _) => {
            if let Some(input) = state.api_key_input_mut() {
                input.pop();
            }
            if state.onboarding_mode {
                state.validation_status = None;
            }
            ApiKeyModalKeyResult::Handled
        }
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            if let Some(input) = state.api_key_input_mut() {
                input.push(c);
            }
            if state.onboarding_mode {
                state.validation_status = None;
            }
            ApiKeyModalKeyResult::Handled
        }
        _ => ApiKeyModalKeyResult::Handled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::config::ProviderKind;
    use std::path::PathBuf;

    #[test]
    fn backspace_clears_validation_status_during_onboarding() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.onboarding_mode = true;
        st.open_api_key_modal(ProviderKind::MiniMax, false, true);
        st.validation_status = Some(OnboardingValidation::Failed("bad".into()));
        handle_api_key_modal_key(
            &mut st,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert!(st.validation_status.is_none());
    }
}
