//! Web tool configuration (timeouts, fetch limits, user-agent).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    pub timeout_secs: u64,
    pub max_fetch_chars: usize,
    pub default_search_limit: usize,
    pub user_agent: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 15,
            max_fetch_chars: 25_000,
            default_search_limit: 5,
            user_agent: "nca/0.5 (+https://github.com/user/native-cli-ai)".into(),
        }
    }
}

impl WebConfig {
    pub(super) fn merge(&mut self, partial: PartialWebConfig) {
        if let Some(timeout_secs) = partial.timeout_secs {
            self.timeout_secs = timeout_secs;
        }
        if let Some(max_fetch_chars) = partial.max_fetch_chars {
            self.max_fetch_chars = max_fetch_chars;
        }
        if let Some(default_search_limit) = partial.default_search_limit {
            self.default_search_limit = default_search_limit;
        }
        if let Some(user_agent) = partial.user_agent {
            self.user_agent = user_agent;
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialWebConfig {
    pub(super) timeout_secs: Option<u64>,
    pub(super) max_fetch_chars: Option<usize>,
    pub(super) default_search_limit: Option<usize>,
    pub(super) user_agent: Option<String>,
}
