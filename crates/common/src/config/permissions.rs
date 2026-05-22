//! Permission (approval policy) configuration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionConfig {
    pub mode: PermissionMode,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
}

impl PermissionConfig {
    pub(super) fn merge(&mut self, partial: PartialPermissionConfig) {
        if let Some(mode) = partial.mode {
            self.mode = mode;
        }
        if let Some(allow) = partial.allow {
            self.allow = allow;
        }
        if let Some(deny) = partial.deny {
            self.deny = deny;
        }
        if let Some(ask) = partial.ask {
            self.ask = ask;
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    #[default]
    Default,
    Plan,
    AcceptEdits,
    DontAsk,
    BypassPermissions,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialPermissionConfig {
    pub(super) mode: Option<PermissionMode>,
    pub(super) allow: Option<Vec<String>>,
    pub(super) deny: Option<Vec<String>>,
    pub(super) ask: Option<Vec<String>>,
}
