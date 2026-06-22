//! Full-screen session TUI (transcript + streaming + composer).

pub mod app;
pub mod bridge;
pub mod busy_indicator;
pub mod connect_modal;
pub mod elm;
pub mod onboarding;
pub mod replay;
pub mod state;
pub mod text_utils;

pub use app::{
    TuiCmd, git_create_branch, git_current_branch, git_list_branches, git_switch_branch,
};
pub use bridge::spawn_tui_bridge;
pub use replay::replay_events_to_feedback;
pub use state::{ModelPickerAction, ModelPickerEntry};
