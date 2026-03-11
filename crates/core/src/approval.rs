use nca_common::config::{PermissionConfig, PermissionMode};
use nca_common::tool::{PermissionTier, ToolCall};
use std::sync::Arc;

#[async_trait::async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn resolve(&self, call: &ToolCall, description: &str) -> bool;
}

/// Determines whether a tool call or command is allowed, needs approval, or is denied.
pub struct ApprovalPolicy {
    config: PermissionConfig,
    handler: Option<Arc<dyn ApprovalHandler>>,
}

impl ApprovalPolicy {
    pub fn new(config: PermissionConfig) -> Self {
        Self {
            config,
            handler: None,
        }
    }

    pub fn with_handler(mut self, handler: Arc<dyn ApprovalHandler>) -> Self {
        self.handler = Some(handler);
        self
    }

    /// Check the permission tier for a given tool name and input description.
    pub fn check(&self, tool_name: &str, description: &str) -> PermissionTier {
        let key = format!("{tool_name}:{description}");

        for pattern in &self.config.deny {
            if key.contains(pattern) {
                return PermissionTier::Denied;
            }
        }

        let explicitly_allowed = self
            .config
            .allow
            .iter()
            .any(|pattern| key.contains(pattern));

        let readonly = matches!(
            tool_name,
            "read_file"
                | "list_directory"
                | "search_code"
                | "git_status"
                | "git_diff"
                | "query_symbols"
                | "web_search"
                | "fetch_url"
        );
        let file_edit = matches!(
            tool_name,
            "write_file"
                | "create_directory"
                | "apply_patch"
                | "edit_file"
                | "rename_path"
                | "move_path"
                | "copy_path"
        );
        let destructive = matches!(tool_name, "delete_path");
        let execution = matches!(tool_name, "execute_bash" | "run_validation");

        match self.config.mode {
            PermissionMode::BypassPermissions => PermissionTier::Allowed,
            PermissionMode::Plan => {
                if readonly {
                    PermissionTier::Allowed
                } else {
                    PermissionTier::Denied
                }
            }
            PermissionMode::AcceptEdits => {
                if destructive {
                    PermissionTier::Ask
                } else if explicitly_allowed {
                    PermissionTier::Allowed
                } else if readonly || file_edit {
                    PermissionTier::Allowed
                } else if execution {
                    PermissionTier::Ask
                } else {
                    PermissionTier::Ask
                }
            }
            PermissionMode::DontAsk => {
                if readonly {
                    PermissionTier::Allowed
                } else {
                    PermissionTier::Denied
                }
            }
            PermissionMode::Default => {
                if explicitly_allowed || readonly {
                    PermissionTier::Allowed
                } else {
                    PermissionTier::Ask
                }
            }
        }
    }

    pub async fn resolve(&self, call: &ToolCall, description: &str) -> bool {
        match &self.handler {
            Some(handler) => handler.resolve(call, description).await,
            None => false,
        }
    }

    pub fn mode(&self) -> PermissionMode {
        self.config.mode
    }

    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.config.mode = mode;
    }
}
