//! Plain Rust mirror of the WIT types, so callers never touch generated bindings.

/// Provider credential override. Unset fields keep the configured value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Credential {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRequest {
    pub task: String,
    pub focus_files: Vec<String>,
    pub skills: Vec<String>,
    pub use_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnDecision {
    Allow(SpawnRequest),
    Deny(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDecision {
    Allow,
    Deny(String),
}

/// `(token, "#rrggbb")` pairs.
pub type ThemeEntries = Vec<(String, String)>;
