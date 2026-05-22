//! Config file load/save and partial TOML deserialization.

use std::path::Path;

use serde::Deserialize;

use super::{
    ConfigError, NcaConfig,
    hooks::PartialHookConfig,
    mcp::PartialMcpConfig,
    memory::PartialMemoryConfig,
    model::PartialModelConfig,
    permissions::PartialPermissionConfig,
    provider::PartialProviderConfig,
    ui::PartialUiConfig,
    web::PartialWebConfig,
    workspace::{PartialHarnessConfig, PartialSessionConfig},
};

pub(super) fn load_partial(path: &Path) -> Result<PartialNcaConfig, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;

    toml::from_str(&raw).map_err(|source| ConfigError::ParseToml {
        path: path.to_path_buf(),
        source,
    })
}

pub(super) fn save_config_to_path(config: &NcaConfig, path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            action: "create config directory",
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let raw = toml::to_string_pretty(config).map_err(|source| ConfigError::SerializeToml {
        path: path.to_path_buf(),
        source,
    })?;

    std::fs::write(path, raw).map_err(|source| ConfigError::Io {
        action: "write config file",
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialNcaConfig {
    pub(super) provider: Option<PartialProviderConfig>,
    pub(super) model: Option<PartialModelConfig>,
    pub(super) permissions: Option<PartialPermissionConfig>,
    pub(super) session: Option<PartialSessionConfig>,
    pub(super) harness: Option<PartialHarnessConfig>,
    pub(super) mcp: Option<PartialMcpConfig>,
    pub(super) memory: Option<PartialMemoryConfig>,
    pub(super) hooks: Option<PartialHookConfig>,
    pub(super) web: Option<PartialWebConfig>,
    pub(super) ui: Option<PartialUiConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NcaConfig, ProviderKind};

    #[test]
    fn session_accepts_max_turn_per_run_typo_alias() {
        let raw = r"
            [session]
            max_turn_per_run = 99
        ";
        let partial: PartialNcaConfig = toml::from_str(raw).expect("parse");
        let session = partial.session.expect("session table");
        assert_eq!(session.max_turns_per_run, Some(99));
    }

    #[test]
    fn ui_editor_roundtrips_through_workspace_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = NcaConfig::default();
        config.ui.editor = Some("vim".into());
        config.set_default_provider(ProviderKind::MiniMax);
        config.save_workspace_file(dir.path()).expect("save");

        let loaded = NcaConfig::load_for_workspace(dir.path()).expect("load");
        assert_eq!(loaded.ui.editor.as_deref(), Some("vim"));
    }
}
