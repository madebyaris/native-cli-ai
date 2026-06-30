//! Plugin trait for nca — lets Rust crates hook into the system prompt pipeline
//! and inject always-on behavior rules (like ponytail's lazy-mode personality).
//!
//! # Example
//!
//! ```rust,ignore
//! use nca_core::plugin::NcaPlugin;
//!
//! pub struct MyPlugin;
//!
//! impl NcaPlugin for MyPlugin {
//!     fn name(&self) -> &str { "my-plugin" }
//!
//!     fn on_system_prompt(&self) -> Option<String> {
//!         Some("Always prefer the simplest solution.".into())
//!     }
//! }
//! ```

use std::path::Path;

use nca_common::config::NcaConfig;

/// A Rust-native plugin that extends nca's behavior.
///
/// Each plugin gets a chance to inject system-prompt text on every turn,
/// similar to ponytail.mjs's `experimental.chat.system.transform` hook.
///
/// Plugins are created once at session startup and live for the session's
/// lifetime. State persistence is the plugin's own responsibility (typically
/// via `~/.nca/state/<plugin-name>/`).
pub trait NcaPlugin: Send + Sync {
    /// Stable name used for diagnostics and state directory naming.
    fn name(&self) -> &str;

    /// Called every time the system prompt is built.
    ///
    /// Return `Some(text)` to inject behavior rules into the system prompt.
    /// Return `None` to skip injection (e.g. plugin is disabled).
    ///
    /// The text is appended after local instructions but before the skills
    /// index, so plugin-injected rules appear in a predictable position.
    fn on_system_prompt(&self, _config: &NcaConfig, _workspace_root: &Path) -> Option<String> {
        None
    }
}

/// A collection of plugins loaded at session startup.
///
/// Passed through [`supervisor::SupervisorConfig`] and forwarded to
/// [`crate::harness::build_system_prompt`] so all plugins can inject
/// their system-prompt transforms on every prompt build.
#[derive(Default)]
pub struct PluginRegistry {
    plugins: Vec<Box<dyn NcaPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin. Order matters: plugins are invoked in registration
    /// order, so the last one to return `Some(text)` has its text appended last
    /// in the pipeline.
    pub fn register(&mut self, plugin: Box<dyn NcaPlugin>) {
        self.plugins.push(plugin);
    }

    /// Collect all non-`None` system-prompt contributions from registered plugins.
    pub fn collect_prompts(
        &self,
        config: &NcaConfig,
        workspace_root: &Path,
    ) -> Vec<(String, String)> {
        self.plugins
            .iter()
            .filter_map(|plugin| {
                plugin
                    .on_system_prompt(config, workspace_root)
                    .map(|text| (plugin.name().to_string(), text))
            })
            .collect()
    }

    /// Iterate over all registered plugins.
    pub fn iter(&self) -> impl Iterator<Item = &dyn NcaPlugin> {
        self.plugins.iter().map(|p| p.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::config::NcaConfig;
    use std::path::Path;

    struct TestPlugin;

    impl NcaPlugin for TestPlugin {
        fn name(&self) -> &str {
            "test-plugin"
        }

        fn on_system_prompt(&self, _config: &NcaConfig, _workspace_root: &Path) -> Option<String> {
            Some("Test rule: be concise.".into())
        }
    }

    struct SilentPlugin;

    impl NcaPlugin for SilentPlugin {
        fn name(&self) -> &str {
            "silent-plugin"
        }

        fn on_system_prompt(&self, _config: &NcaConfig, _workspace_root: &Path) -> Option<String> {
            None
        }
    }

    #[test]
    fn registry_collects_non_none_plugins() {
        let mut reg = PluginRegistry::new();
        reg.register(Box::new(TestPlugin));
        reg.register(Box::new(SilentPlugin));

        let config = NcaConfig::default();
        let dir = tempfile::tempdir().unwrap();
        let prompts = reg.collect_prompts(&config, dir.path());

        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].0, "test-plugin");
        assert!(prompts[0].1.contains("Test rule"));
    }

    #[test]
    fn empty_registry_returns_empty() {
        let reg = PluginRegistry::new();
        let config = NcaConfig::default();
        let dir = tempfile::tempdir().unwrap();
        assert!(reg.collect_prompts(&config, dir.path()).is_empty());
    }
}
