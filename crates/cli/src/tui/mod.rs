//! Full-screen session TUI (transcript + streaming + composer).

#[allow(dead_code)]
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
pub use replay::replay_event_log_into_state;
pub use state::{DisplayBlock, ModelPickerAction, ModelPickerEntry, TuiSessionState};
