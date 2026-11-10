//! What the TUI and CLI show and change about installed plugins.
//!
//! Plugins are loaded once per process (see [`crate::init`]), so enabling or disabling one
//! rewrites the registry at once but only takes effect on the next start. [`PluginStatus`]
//! says which of the two states a plugin is in, so a screen never claims more than is true.

use std::path::Path;

use crate::error::PluginError;
use crate::registry::{Entry, Registry};

/// How a plugin's saved choice relates to what this process has loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadState {
    /// Enabled and running in this process.
    Active,
    /// Enabled in the registry, not loaded yet: needs a restart.
    PendingEnable,
    /// Disabled in the registry but still loaded: needs a restart.
    PendingDisable,
    /// Disabled and not loaded.
    Off,
}

#[derive(Debug, Clone)]
pub struct PluginStatus {
    pub entry: Entry,
    /// Hook names from the verified manifest, or the integrity error.
    pub hooks: Result<Vec<String>, String>,
    pub state: LoadState,
}

impl PluginStatus {
    pub fn needs_restart(&self) -> bool {
        matches!(
            self.state,
            LoadState::PendingEnable | LoadState::PendingDisable
        )
    }

    /// One short phrase for a list row.
    pub fn state_label(&self) -> &'static str {
        match self.state {
            LoadState::Active => "enabled",
            LoadState::PendingEnable => "enabled · restart to load",
            LoadState::PendingDisable => "disabled · restart to unload",
            LoadState::Off => "disabled",
        }
    }
}

fn state_of(entry: &Entry, loaded: &[String]) -> LoadState {
    match (entry.enabled, loaded.iter().any(|n| n == &entry.name)) {
        (true, true) => LoadState::Active,
        (true, false) => LoadState::PendingEnable,
        (false, true) => LoadState::PendingDisable,
        (false, false) => LoadState::Off,
    }
}

/// Plugins that apply to `workspace` (global ones and this workspace's own).
pub fn list(workspace: &Path) -> Result<Vec<PluginStatus>, PluginError> {
    let loaded = crate::loaded_names();
    Ok(Registry::load()?
        .plugins
        .into_iter()
        .filter(|e| e.applies_to(workspace))
        .map(|entry| {
            let hooks = entry
                .load_verified()
                .map(|(m, _)| m.hooks.names().iter().map(|h| h.to_string()).collect())
                .map_err(|e| e.to_string());
            let state = state_of(&entry, &loaded);
            PluginStatus {
                entry,
                hooks,
                state,
            }
        })
        .collect())
}

/// Finds an installed plugin by name: this workspace's copy first, then the global one.
pub fn find(name: &str, workspace: &Path) -> Result<Entry, PluginError> {
    let registry = Registry::load()?;
    registry
        .find(name, Some(workspace))
        .or_else(|| registry.find(name, None))
        .cloned()
        .ok_or_else(|| PluginError::Install(format!("plugin `{name}` is not installed")))
}

/// Saves the enabled flag. Returns the plugin's status after the change.
pub fn set_enabled(
    name: &str,
    workspace: &Path,
    enabled: bool,
) -> Result<PluginStatus, PluginError> {
    let entry = find(name, workspace)?;
    let scope = entry.workspace.as_ref().map(std::path::PathBuf::from);
    crate::install::set_enabled(&entry.name, scope.as_deref(), enabled)?;
    list(workspace)?
        .into_iter()
        .find(|s| s.entry.name == entry.name)
        .ok_or_else(|| PluginError::Install(format!("plugin `{name}` is not installed")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(enabled: bool) -> Entry {
        Entry {
            name: "demo".into(),
            version: "0.1.0".into(),
            source: "https://example.test/demo".into(),
            requested_ref: None,
            commit: "0".repeat(40),
            wasm_sha256: String::new(),
            manifest_sha256: String::new(),
            installed_at: String::new(),
            enabled,
            workspace: None,
            dir: "/nonexistent".into(),
        }
    }

    #[test]
    fn state_follows_the_registry_and_what_is_loaded() {
        let loaded = vec!["demo".to_string()];
        assert_eq!(state_of(&entry(true), &loaded), LoadState::Active);
        assert_eq!(state_of(&entry(false), &loaded), LoadState::PendingDisable);
        assert_eq!(state_of(&entry(true), &[]), LoadState::PendingEnable);
        assert_eq!(state_of(&entry(false), &[]), LoadState::Off);
    }

    #[test]
    fn only_a_mismatch_needs_a_restart() {
        let status = |state| PluginStatus {
            entry: entry(true),
            hooks: Ok(vec![]),
            state,
        };
        assert!(!status(LoadState::Active).needs_restart());
        assert!(!status(LoadState::Off).needs_restart());
        assert!(status(LoadState::PendingEnable).needs_restart());
        assert!(status(LoadState::PendingDisable).needs_restart());
    }
}
