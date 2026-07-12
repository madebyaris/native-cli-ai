//! Full-screen session TUI (transcript + streaming + composer).

pub mod app;
pub mod bridge;
pub mod busy_indicator;
pub mod composer;
pub mod connect_modal;
pub mod git;
pub mod input;
pub mod layout;
pub mod onboarding;
pub mod overlay;
pub mod replay;
pub mod shared;
pub mod state;
pub mod terminal;
pub mod theme;
pub mod transcript;

pub use app::{TuiCmd, run_blocking};
pub use bridge::spawn_tui_bridge;
pub use git::{git_create_branch, git_current_branch, git_list_branches, git_switch_branch};
pub use input::ApprovalAnswer;
pub use overlay::{UiOverlay, UiOverlayKind};
pub use replay::replay_event_log_into_state;
pub use shared::SharedTuiState;
pub use state::{DisplayBlock, ModelPickerAction, ModelPickerEntry, TuiSessionState};
