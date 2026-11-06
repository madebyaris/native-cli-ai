//! `nca-plugin.toml` schema and validation.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path};

use crate::error::PluginError;

/// Plugin API version this build of nca implements.
pub const API_VERSION: &str = "1";
pub const MANIFEST_FILE: &str = "nca-plugin.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub plugin: PluginMeta,
    #[serde(default)]
    pub hooks: Hooks,
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Free-form string settings, readable by the plugin through `host.config-get`.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    pub api_version: String,
    /// Path of the prebuilt component, relative to the repository root.
    pub wasm: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Lower runs first (closest to the user, furthest from core).
    #[serde(default)]
    pub priority: i32,
}

/// Which extension points the plugin overrides. A hook that is not declared
/// here is never called, even if the component exports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hooks {
    #[serde(default)]
    pub auth: bool,
    #[serde(default)]
    pub subagent: bool,
    #[serde(default)]
    pub agent: bool,
    #[serde(default)]
    pub cosmetic: bool,
}

impl Hooks {
    pub fn any(&self) -> bool {
        self.auth || self.subagent || self.agent || self.cosmetic
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.auth {
            out.push("auth");
        }
        if self.subagent {
            out.push("subagent");
        }
        if self.agent {
            out.push("agent");
        }
        if self.cosmetic {
            out.push("cosmetic");
        }
        out
    }
}

/// Deny-by-default grants. `$NCA_PLUGIN_DATA` in path lists expands to the
/// plugin's private data directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Hostnames the plugin may reach over HTTP(S).
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default)]
    pub fs_read: Vec<String>,
    #[serde(default)]
    pub fs_write: Vec<String>,
    /// Environment variables the plugin may read.
    #[serde(default)]
    pub env: Vec<String>,
}

impl Capabilities {
    pub fn is_empty(&self) -> bool {
        self.network.is_empty()
            && self.fs_read.is_empty()
            && self.fs_write.is_empty()
            && self.env.is_empty()
    }

    /// Human-readable lines for the install approval prompt.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.network.is_empty() {
            lines.push(format!("network: {}", self.network.join(", ")));
        }
        if !self.fs_read.is_empty() {
            lines.push(format!("read files: {}", self.fs_read.join(", ")));
        }
        if !self.fs_write.is_empty() {
            lines.push(format!("write files: {}", self.fs_write.join(", ")));
        }
        if !self.env.is_empty() {
            lines.push(format!("environment: {}", self.env.join(", ")));
        }
        lines
    }
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, PluginError> {
        let manifest: Manifest =
            toml::from_str(text).map_err(|e| PluginError::Manifest(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load(repo_root: &Path) -> Result<Self, PluginError> {
        let path = repo_root.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| PluginError::Manifest(format!("cannot read {}: {e}", path.display())))?;
        Self::parse(&text)
    }

    pub fn validate(&self) -> Result<(), PluginError> {
        let name = &self.plugin.name;
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            || name.starts_with('-')
        {
            return Err(PluginError::Manifest(format!(
                "plugin.name `{name}` must be lowercase [a-z0-9-_] and not start with `-`"
            )));
        }
        if self.plugin.api_version != API_VERSION {
            return Err(PluginError::Manifest(format!(
                "plugin `{name}` targets plugin API v{}, this nca supports v{API_VERSION}",
                self.plugin.api_version
            )));
        }
        if !self.hooks.any() {
            return Err(PluginError::Manifest(format!(
                "plugin `{name}` declares no hooks; enable at least one under [hooks]"
            )));
        }
        relative_path(&self.plugin.wasm, "plugin.wasm")?;
        if !self.plugin.wasm.ends_with(".wasm") {
            return Err(PluginError::Manifest(
                "plugin.wasm must point at a .wasm file".into(),
            ));
        }
        for host in &self.capabilities.network {
            if host.is_empty() || host.contains('/') || host.contains(':') || host.contains('*') {
                return Err(PluginError::Manifest(format!(
                    "capabilities.network entry `{host}` must be a bare hostname"
                )));
            }
        }
        Ok(())
    }
}

/// Reject absolute paths and `..` so a manifest cannot point outside its repo.
pub fn relative_path(value: &str, field: &str) -> Result<(), PluginError> {
    let path = Path::new(value);
    let bad = path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)));
    if value.is_empty() || bad {
        return Err(PluginError::Manifest(format!(
            "{field} `{value}` must be a relative path inside the plugin repository"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
[plugin]
name = "acme-sso"
version = "0.1.0"
api_version = "1"
wasm = "plugin.wasm"
[hooks]
auth = true
[capabilities]
network = ["login.acme.com"]
"#;

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::parse(OK).unwrap();
        assert_eq!(m.plugin.name, "acme-sso");
        assert_eq!(m.hooks.names(), vec!["auth"]);
        assert_eq!(m.capabilities.network, vec!["login.acme.com"]);
    }

    #[test]
    fn rejects_wrong_api_version() {
        let err =
            Manifest::parse(&OK.replace("api_version = \"1\"", "api_version = \"2\"")).unwrap_err();
        assert!(err.to_string().contains("API v2"), "{err}");
    }

    #[test]
    fn rejects_no_hooks_and_traversal_and_bad_network() {
        assert!(Manifest::parse(&OK.replace("auth = true", "auth = false")).is_err());
        assert!(Manifest::parse(&OK.replace("plugin.wasm", "../evil.wasm")).is_err());
        assert!(Manifest::parse(&OK.replace("plugin.wasm", "/abs/evil.wasm")).is_err());
        assert!(Manifest::parse(&OK.replace("login.acme.com", "https://x.com")).is_err());
        assert!(Manifest::parse(&OK.replace("acme-sso", "Acme..")).is_err());
    }
}
