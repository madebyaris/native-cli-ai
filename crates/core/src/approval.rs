use nca_common::config::PermissionConfig;
use nca_common::tool::PermissionTier;

/// Determines whether a tool call or command is allowed, needs approval, or is denied.
pub struct ApprovalPolicy {
    config: PermissionConfig,
}

impl ApprovalPolicy {
    pub fn new(config: PermissionConfig) -> Self {
        Self { config }
    }

    /// Check the permission tier for a given tool name and input description.
    pub fn check(&self, tool_name: &str, description: &str) -> PermissionTier {
        let key = format!("{tool_name}:{description}");

        for pattern in &self.config.deny {
            if key.contains(pattern) {
                return PermissionTier::Denied;
            }
        }

        for pattern in &self.config.allow {
            if key.contains(pattern) {
                return PermissionTier::Allowed;
            }
        }

        // Default read-only tools are always allowed
        match tool_name {
            "read_file"
            | "list_directory"
            | "search_code"
            | "git_status"
            | "git_diff"
            | "query_symbols" => PermissionTier::Allowed,
            _ => PermissionTier::Ask,
        }
    }
}
