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

/// How to start a sign-in, as returned by a plugin's `login.begin-login`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginChallenge {
    pub plugin: String,
    pub url: String,
    pub instructions: String,
}

/// Result of telling a plugin its credential was rejected (HTTP 401/403).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthRecovery {
    /// No installed plugin owns the credential for this provider.
    NoPlugin,
    /// A fresh credential; retry the request once.
    Refreshed(String),
    /// Recovery is impossible; show the message and start the sign-in again.
    LoginRequired(String),
    /// The plugin could not try (offline, timeout) or failed. Not a sign-out:
    /// report the error and let the user retry.
    Unavailable(String),
}
