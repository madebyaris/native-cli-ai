//! Component identifiers for enum-based dispatch.

/// Unique identifier for each TUI component or popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CompId {
    // Core components
    StatusBar,
    Transcript,
    Composer,
    // Popups (mount/unmount via Option<T>)
    CommandPalette,
    ModelPicker,
    BranchPicker,
    ProviderPicker,
    AgentPicker,
    PermissionPicker,
    SessionPicker,
    ConnectModal,
    ApiKeyModal,
    InfoModal,
    SlashPanel,
    AtCompletionPanel,
    // Modal overlays
    Onboarding,
}
