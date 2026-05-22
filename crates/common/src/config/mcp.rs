//! MCP (Model Context Protocol) server configuration.

use super::default_true;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub expose_in_safe_mode: bool,
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl McpConfig {
    pub(super) fn merge(&mut self, partial: PartialMcpConfig) {
        if let Some(expose_in_safe_mode) = partial.expose_in_safe_mode {
            self.expose_in_safe_mode = expose_in_safe_mode;
        }
        if let Some(servers) = partial.servers {
            self.servers = servers;
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialMcpConfig {
    pub(super) expose_in_safe_mode: Option<bool>,
    pub(super) servers: Option<Vec<McpServerConfig>>,
}
