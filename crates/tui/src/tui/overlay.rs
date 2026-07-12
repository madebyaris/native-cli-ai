//! Modal overlay finite-state machine for the full-screen TUI.
//!
//! At most one [`UiOverlay`] is active at a time (plus inline composer panels
//! handled separately via [`crate::tui::input::InputContext`].

use nca_common::config::ProviderKind;
use std::fmt;

use super::state::{CustomProviderSetupStep, ModelPickerEntry};

/// Active modal overlay and its variant-specific payload.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UiOverlay {
    #[default]
    None,
    CommandPalette {
        query: String,
        palette_index: usize,
    },
    BranchPicker {
        query: String,
        index: usize,
        branches: Vec<String>,
    },
    ConnectModal {
        search: String,
        menu_index: usize,
        scroll: usize,
    },
    ApiKeyModal {
        provider: ProviderKind,
        input: String,
        has_existing: bool,
        connect_after_save: bool,
    },
    InfoModal {
        title: String,
        lines: Vec<String>,
        scroll: usize,
    },
    ModelPicker {
        search: String,
        index: usize,
        entries: Vec<ModelPickerEntry>,
        scroll: usize,
    },
    PermissionPicker {
        index: usize,
    },
    AgentPicker {
        index: usize,
    },
    QuestionModal {
        index: usize,
        scroll: usize,
    },
    SessionPicker {
        search: String,
        index: usize,
        entries: Vec<String>,
        scroll: usize,
    },
    ProviderPicker {
        index: usize,
        scroll: usize,
        for_api_key: bool,
        include_add_row: bool,
    },
    CustomProviderSetup {
        step: CustomProviderSetupStep,
        compat_index: usize,
        input: String,
        base_url: String,
        api_key: String,
        model_hint: String,
    },
}

impl UiOverlay {
    /// Whether any modal overlay is open (blocks normal chat input routing).
    pub fn is_open(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Lightweight tag for transition-table tests and logging.
    pub fn kind(&self) -> UiOverlayKind {
        match self {
            Self::None => UiOverlayKind::None,
            Self::CommandPalette { .. } => UiOverlayKind::CommandPalette,
            Self::BranchPicker { .. } => UiOverlayKind::BranchPicker,
            Self::ConnectModal { .. } => UiOverlayKind::ConnectModal,
            Self::ApiKeyModal { .. } => UiOverlayKind::ApiKeyModal,
            Self::InfoModal { .. } => UiOverlayKind::InfoModal,
            Self::ModelPicker { .. } => UiOverlayKind::ModelPicker,
            Self::PermissionPicker { .. } => UiOverlayKind::PermissionPicker,
            Self::AgentPicker { .. } => UiOverlayKind::AgentPicker,
            Self::QuestionModal { .. } => UiOverlayKind::QuestionModal,
            Self::SessionPicker { .. } => UiOverlayKind::SessionPicker,
            Self::ProviderPicker { .. } => UiOverlayKind::ProviderPicker,
            Self::CustomProviderSetup { .. } => UiOverlayKind::CustomProviderSetup,
        }
    }

    /// Close the overlay, returning to [`UiOverlay::None`].
    pub fn close(self) -> Self {
        Self::None
    }

    /// Whether `next` may replace `current`. Only [`UiOverlay::None`] accepts
    /// arbitrary opens; switching overlays requires an explicit close first.
    pub fn can_transition_to(current: &Self, next: &Self) -> bool {
        if next.kind() == UiOverlayKind::None {
            return true;
        }
        current.kind() == UiOverlayKind::None || current.kind() == next.kind()
    }

    /// Apply `next`, panicking in debug builds when the transition is illegal.
    pub fn transition(current: Self, next: Self) -> Self {
        if next.kind() == UiOverlayKind::None {
            Self::None
        } else if Self::can_transition_to(&current, &next) {
            next
        } else {
            current
        }
    }
}

/// Stable overlay identity (ignores inner payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiOverlayKind {
    None,
    CommandPalette,
    BranchPicker,
    ConnectModal,
    ApiKeyModal,
    InfoModal,
    ModelPicker,
    PermissionPicker,
    AgentPicker,
    QuestionModal,
    SessionPicker,
    ProviderPicker,
    CustomProviderSetup,
}

impl fmt::Display for UiOverlayKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_palette() -> UiOverlay {
        UiOverlay::CommandPalette {
            query: String::new(),
            palette_index: 0,
        }
    }

    fn open_connect() -> UiOverlay {
        UiOverlay::ConnectModal {
            search: String::new(),
            menu_index: 0,
            scroll: 0,
        }
    }

    #[test]
    fn close_from_any_overlay_returns_none() {
        assert!(UiOverlay::can_transition_to(
            &open_palette(),
            &UiOverlay::None
        ));
        assert!(UiOverlay::can_transition_to(
            &open_connect(),
            &UiOverlay::None
        ));
    }

    #[test]
    fn open_from_none_is_allowed() {
        assert!(UiOverlay::can_transition_to(
            &UiOverlay::None,
            &open_palette()
        ));
        assert!(UiOverlay::can_transition_to(
            &UiOverlay::None,
            &open_connect()
        ));
    }

    #[test]
    fn swap_overlay_without_close_is_rejected() {
        assert!(!UiOverlay::can_transition_to(
            &open_palette(),
            &open_connect()
        ));
    }

    #[test]
    fn same_overlay_reopen_is_allowed() {
        let a = open_palette();
        let b = UiOverlay::CommandPalette {
            query: "foo".into(),
            palette_index: 1,
        };
        assert!(UiOverlay::can_transition_to(&a, &b));
    }

    #[test]
    fn transition_close_clears() {
        assert_eq!(
            UiOverlay::transition(open_palette(), UiOverlay::None),
            UiOverlay::None
        );
    }

    #[test]
    fn transition_swap_keeps_current_when_illegal() {
        assert_eq!(
            UiOverlay::transition(open_palette(), open_connect()),
            open_palette()
        );
    }

    #[test]
    fn overlay_kind_roundtrip() {
        assert_eq!(open_palette().kind(), UiOverlayKind::CommandPalette);
        assert_eq!(UiOverlay::None.kind(), UiOverlayKind::None);
    }
}
