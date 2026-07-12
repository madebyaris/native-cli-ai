//! Input routing for the full-screen TUI, split by [`InputContext`].

mod api_key_modal;
mod approval;
mod at_panel;
mod branch_picker;
mod chat;
mod command_palette;
mod connect_modal;
mod question;
mod slash_panel;

pub use api_key_modal::{ApiKeyModalKeyResult, handle_api_key_modal_key};
pub use approval::{ApprovalAnswer, handle_approval_key, parse_tui_question_answer};
pub use at_panel::{handle_at_panel_key, handle_at_panel_mouse, render_at_panel};
pub use branch_picker::{BranchPickerKeyResult, handle_branch_picker_key, render_branch_picker};
pub use chat::handle_chat_key;
pub use command_palette::{PaletteKeyResult, handle_command_palette_key, render_command_palette};
pub use connect_modal::{ConnectModalKeyResult, handle_connect_modal_key};
pub use question::{QuestionModalKeyResult, handle_question_modal_key};
pub use slash_panel::{handle_slash_panel_key, render_slash_panel};

use crate::tui::composer::slash_panel_visible;
use crate::tui::state::TuiSessionState;

/// Which input handler owns the next key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputContext {
    CommandPalette,
    ConnectModal,
    ApiKeyModal,
    BranchPicker,
    QuestionModal,
    Approval,
    SlashPanel,
    AtPanel,
    Chat,
}

/// Resolve the active input context from session state.
pub fn resolve_input_context(state: &TuiSessionState, at_active: bool) -> InputContext {
    // Approval shortcuts must remain available even if a question modal was
    // opened before the approval request arrived.
    if state.active_approval.is_some() {
        return InputContext::Approval;
    }
    match state.overlay.kind() {
        crate::tui::overlay::UiOverlayKind::CommandPalette => InputContext::CommandPalette,
        crate::tui::overlay::UiOverlayKind::ConnectModal => InputContext::ConnectModal,
        crate::tui::overlay::UiOverlayKind::ApiKeyModal => InputContext::ApiKeyModal,
        crate::tui::overlay::UiOverlayKind::BranchPicker => InputContext::BranchPicker,
        crate::tui::overlay::UiOverlayKind::QuestionModal => InputContext::QuestionModal,
        _ if slash_panel_visible(&state.input_buffer) => InputContext::SlashPanel,
        _ if at_active => InputContext::AtPanel,
        _ => InputContext::Chat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::TuiSessionState;
    use std::path::PathBuf;

    #[test]
    fn resolve_palette_over_chat() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.open_command_palette();
        assert_eq!(
            resolve_input_context(&st, false),
            InputContext::CommandPalette
        );
    }

    #[test]
    fn resolve_approval_over_slash_panel() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.input_buffer = "/help".into();
        st.active_approval = Some(crate::tui::state::ApprovalRequest {
            call_id: "c".into(),
            tool: "bash".into(),
            description: "d".into(),
            input: "{}".into(),
        });
        assert_eq!(resolve_input_context(&st, false), InputContext::Approval);
    }

    #[test]
    fn resolve_approval_over_question_modal() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.open_question_modal();
        st.active_approval = Some(crate::tui::state::ApprovalRequest {
            call_id: "c".into(),
            tool: "bash".into(),
            description: "d".into(),
            input: "{}".into(),
        });
        assert_eq!(resolve_input_context(&st, false), InputContext::Approval);
    }
}
