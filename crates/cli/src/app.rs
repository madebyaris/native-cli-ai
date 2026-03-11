/// Application state machine for the CLI.
pub enum AppState {
    Initializing,
    Idle,
    WaitingForInput,
    Processing,
    WaitingForApproval { call_id: String },
    Exiting,
}

pub struct App {
    pub state: AppState,
    pub safe_mode: bool,
}

impl App {
    pub fn new(safe_mode: bool) -> Self {
        Self {
            state: AppState::Initializing,
            safe_mode,
        }
    }
}
