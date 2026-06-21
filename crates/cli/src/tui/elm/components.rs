//! Concrete component storage — enum-based dispatch, no dyn trait.

use ratatui::Frame;
use ratatui::layout::Rect;

use super::comp_id::CompId;
use super::msg::Msg;
use crate::tui::app::TuiCmd;

pub(crate) mod composer;
pub(crate) mod global_listener;
pub(crate) mod searchable_list;
pub(crate) mod status_bar;
pub(crate) mod theme;
pub(crate) mod transcript;
pub(crate) mod transcript_render;

// Popup components
pub(crate) mod agent_picker;
pub(crate) mod api_key_modal;
pub(crate) mod branch_picker;
pub(crate) mod command_palette;
pub(crate) mod connect_modal;
pub(crate) mod info_modal;
pub(crate) mod model_picker;
pub(crate) mod permission_picker;
pub(crate) mod provider_picker;
pub(crate) mod session_picker;

// ── Remaining stub components (not yet migrated) ────────────────

macro_rules! stub_component {
    ($name:ident, $label:expr) => {
        pub(crate) struct $name;

        impl $name {
            pub(crate) fn new() -> Self {
                Self
            }
        }

        impl super::component::NcaComponent for $name {
            fn view(&mut self, frame: &mut Frame, area: Rect) {
                use ratatui::widgets::Paragraph;
                frame.render_widget(Paragraph::new($label), area);
            }
        }
    };
}

stub_component!(SlashPanel, "");
stub_component!(AtCompletionPanel, "");
stub_component!(Onboarding, "");

// ── Components container ──────────────────────────────────────────

/// Holds all concrete component instances.
///
/// Popups are stored as `Option<T>` — `Some` when mounted (visible), `None` when unmounted.
pub(crate) struct Components {
    pub(crate) status_bar: status_bar::StatusBar,
    pub(crate) transcript: transcript::TranscriptState,
    pub(crate) composer: composer::Composer,
    pub(crate) global_listener: global_listener::GlobalListener,
    // Popups (mount/unmount)
    pub(crate) command_palette: Option<command_palette::CommandPaletteState>,
    pub(crate) model_picker: Option<model_picker::ModelPickerState>,
    pub(crate) branch_picker: Option<branch_picker::BranchPickerState>,
    pub(crate) provider_picker: Option<provider_picker::ProviderPickerState>,
    pub(crate) agent_picker: Option<agent_picker::AgentPickerState>,
    pub(crate) permission_picker: Option<permission_picker::PermissionPickerState>,
    pub(crate) session_picker: Option<session_picker::SessionPickerState>,
    pub(crate) connect_modal: Option<connect_modal::ConnectModalState>,
    pub(crate) api_key_modal: Option<api_key_modal::ApiKeyModalState>,
    pub(crate) info_modal: Option<info_modal::InfoModalState>,
    // Remaining stubs
    pub(crate) slash_panel: Option<SlashPanel>,
    pub(crate) at_completion_panel: Option<AtCompletionPanel>,
    pub(crate) onboarding: Option<Onboarding>,
}

impl Components {
    pub(crate) fn new() -> Self {
        Self {
            status_bar: status_bar::StatusBar::new(),
            transcript: transcript::TranscriptState::new(),
            composer: composer::Composer::new(),
            global_listener: global_listener::GlobalListener::new(),
            command_palette: None,
            model_picker: None,
            branch_picker: None,
            provider_picker: None,
            agent_picker: None,
            permission_picker: None,
            session_picker: None,
            connect_modal: None,
            api_key_modal: None,
            info_modal: None,
            slash_panel: None,
            at_completion_panel: None,
            onboarding: None,
        }
    }

    /// Whether any popup is currently mounted (open).
    pub(crate) fn is_any_popup_open(&self) -> bool {
        self.command_palette.is_some()
            || self.model_picker.is_some()
            || self.branch_picker.is_some()
            || self.provider_picker.is_some()
            || self.agent_picker.is_some()
            || self.permission_picker.is_some()
            || self.session_picker.is_some()
            || self.connect_modal.is_some()
            || self.api_key_modal.is_some()
            || self.info_modal.is_some()
    }

    /// Update popup_open flag from current popup mount state.
    pub(crate) fn sync_popup_open(&self) -> bool {
        self.is_any_popup_open()
    }

    /// Dispatch an event to the focused/mounted component.
    /// Returns `Some(Msg)` if the component produced a side-effect message.
    pub(crate) fn handle_event(&mut self, id: CompId, ev: &Msg) -> Option<Msg> {
        match id {
            // Global listener handles all key events regardless of focus.
            CompId::StatusBar => None, // status bar is read-only
            CompId::Transcript => {
                // Transcript handles Key and Mouse events directly via NcaModel.
                None
            }
            CompId::Composer => match ev {
                Msg::Key(key) => self.composer.handle_key(*key),
                Msg::Paste(text) => {
                    self.composer.handle_paste(text);
                    None
                }
                _ => None,
            },
            _ => {
                // Check global listener first (it may intercept keys like Ctrl+Q, Ctrl+P, etc.)
                if let Some(msg) = self.global_listener.on(ev) {
                    return Some(msg);
                }
                // Popup dispatch (only if mounted)
                match id {
                    CompId::CommandPalette if self.command_palette.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.command_palette.as_mut()?.handle_key(*key);
                            self.popup_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::ModelPicker if self.model_picker.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.model_picker.as_mut()?.handle_key(*key);
                            self.model_picker_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::BranchPicker if self.branch_picker.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.branch_picker.as_mut()?.handle_key(*key);
                            self.branch_picker_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::ProviderPicker if self.provider_picker.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.provider_picker.as_mut()?.handle_key(*key);
                            self.provider_picker_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::AgentPicker if self.agent_picker.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.agent_picker.as_mut()?.handle_key(*key);
                            self.agent_picker_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::PermissionPicker if self.permission_picker.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.permission_picker.as_mut()?.handle_key(*key);
                            self.permission_picker_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::SessionPicker if self.session_picker.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.session_picker.as_mut()?.handle_key(*key);
                            self.session_picker_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::ConnectModal if self.connect_modal.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.connect_modal.as_mut()?.handle_key(*key);
                            self.connect_modal_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::ApiKeyModal if self.api_key_modal.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.api_key_modal.as_mut()?.handle_key(*key);
                            self.api_key_modal_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::InfoModal if self.info_modal.is_some() => {
                        if let Msg::Key(key) = ev {
                            let action = self.info_modal.as_mut()?.handle_key(*key);
                            self.info_modal_action_to_msg(action)
                        } else {
                            None
                        }
                    }
                    CompId::SlashPanel if self.slash_panel.is_some() => None,
                    CompId::AtCompletionPanel if self.at_completion_panel.is_some() => None,
                    CompId::Onboarding if self.onboarding.is_some() => None,
                    _ => None,
                }
            }
        }
    }

    // ── Popup action → Msg conversion ──────────────────────────────

    fn popup_action_to_msg(
        &mut self,
        action: command_palette::CommandPaletteAction,
    ) -> Option<Msg> {
        match action {
            command_palette::CommandPaletteAction::None => None,
            command_palette::CommandPaletteAction::Close => {
                self.command_palette = None;
                None
            }
            command_palette::CommandPaletteAction::SubmitCommand(cmd) => {
                self.command_palette = None;
                Some(Msg::Cmd(TuiCmd::Submit(cmd)))
            }
        }
    }

    fn model_picker_action_to_msg(
        &mut self,
        action: model_picker::ModelPickerPopupAction,
    ) -> Option<Msg> {
        match action {
            model_picker::ModelPickerPopupAction::None => None,
            model_picker::ModelPickerPopupAction::Close => {
                self.model_picker = None;
                None
            }
            model_picker::ModelPickerPopupAction::ApplyModel(m) => {
                self.model_picker = None;
                Some(Msg::Cmd(TuiCmd::ApplyModel(m)))
            }
            model_picker::ModelPickerPopupAction::SwitchProvider(p) => {
                self.model_picker = None;
                Some(Msg::Cmd(TuiCmd::ApplyModelProvider(p)))
            }
        }
    }

    fn branch_picker_action_to_msg(
        &mut self,
        action: branch_picker::BranchPickerAction,
    ) -> Option<Msg> {
        match action {
            branch_picker::BranchPickerAction::None => None,
            branch_picker::BranchPickerAction::Close => {
                self.branch_picker = None;
                None
            }
            branch_picker::BranchPickerAction::SwitchBranch(name) => {
                self.branch_picker = None;
                Some(Msg::Cmd(TuiCmd::SwitchBranch(name)))
            }
            branch_picker::BranchPickerAction::CreateBranch(name) => {
                self.branch_picker = None;
                Some(Msg::Cmd(TuiCmd::CreateBranch(name)))
            }
        }
    }

    fn provider_picker_action_to_msg(
        &mut self,
        action: provider_picker::ProviderPickerAction,
    ) -> Option<Msg> {
        match action {
            provider_picker::ProviderPickerAction::None => None,
            provider_picker::ProviderPickerAction::Close => {
                self.provider_picker = None;
                None
            }
            provider_picker::ProviderPickerAction::ApplyDefaultProvider(p) => {
                self.provider_picker = None;
                Some(Msg::Cmd(TuiCmd::ApplyDefaultProvider(p)))
            }
            provider_picker::ProviderPickerAction::PromptApiKey(p) => {
                self.provider_picker = None;
                Some(Msg::Cmd(TuiCmd::PromptApiKey(p, false)))
            }
        }
    }

    fn agent_picker_action_to_msg(
        &mut self,
        action: agent_picker::AgentPickerAction,
    ) -> Option<Msg> {
        match action {
            agent_picker::AgentPickerAction::None => None,
            agent_picker::AgentPickerAction::Close => {
                self.agent_picker = None;
                None
            }
            agent_picker::AgentPickerAction::SwitchAgent(idx) => {
                self.agent_picker = None;
                Some(Msg::Cmd(TuiCmd::SwitchAgent(idx)))
            }
        }
    }

    fn permission_picker_action_to_msg(
        &mut self,
        action: permission_picker::PermissionPickerAction,
    ) -> Option<Msg> {
        match action {
            permission_picker::PermissionPickerAction::None => None,
            permission_picker::PermissionPickerAction::Close => {
                self.permission_picker = None;
                None
            }
            permission_picker::PermissionPickerAction::ApplyPermission(idx) => {
                self.permission_picker = None;
                Some(Msg::Cmd(TuiCmd::ApplyPermission(idx)))
            }
        }
    }

    fn session_picker_action_to_msg(
        &mut self,
        action: session_picker::SessionPickerAction,
    ) -> Option<Msg> {
        match action {
            session_picker::SessionPickerAction::None => None,
            session_picker::SessionPickerAction::Close => {
                self.session_picker = None;
                None
            }
            session_picker::SessionPickerAction::ResumeSession(id) => {
                self.session_picker = None;
                Some(Msg::Cmd(TuiCmd::ResumeSession(id)))
            }
        }
    }

    fn connect_modal_action_to_msg(
        &mut self,
        action: connect_modal::ConnectModalAction,
    ) -> Option<Msg> {
        match action {
            connect_modal::ConnectModalAction::None => None,
            connect_modal::ConnectModalAction::Close => {
                self.connect_modal = None;
                None
            }
            connect_modal::ConnectModalAction::ConnectProvider(p) => {
                self.connect_modal = None;
                Some(Msg::Cmd(TuiCmd::ApplyDefaultProvider(p)))
            }
        }
    }

    fn api_key_modal_action_to_msg(
        &mut self,
        action: api_key_modal::ApiKeyModalAction,
    ) -> Option<Msg> {
        match action {
            api_key_modal::ApiKeyModalAction::None => None,
            api_key_modal::ApiKeyModalAction::Close => {
                self.api_key_modal = None;
                None
            }
            api_key_modal::ApiKeyModalAction::Confirm(key) => {
                let provider = self
                    .api_key_modal
                    .as_ref()
                    .and_then(|p| p.provider())
                    .unwrap_or(nca_common::config::ProviderKind::MiniMax);
                self.api_key_modal = None;
                Some(Msg::Cmd(TuiCmd::ValidateApiKey(provider, key)))
            }
        }
    }

    fn info_modal_action_to_msg(&mut self, action: info_modal::InfoModalAction) -> Option<Msg> {
        match action {
            info_modal::InfoModalAction::None => None,
            info_modal::InfoModalAction::Close => {
                self.info_modal = None;
                None
            }
        }
    }
}
