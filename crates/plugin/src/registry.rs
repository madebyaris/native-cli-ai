//! On-disk plugin registry: `~/.nca/plugins/registry.json`.
//!
//! The registry lives in the user's home even for project-scoped plugins, so a
//! repository cannot make nca run plugin code by committing `.nca/plugins/`:
//! only plugins the user installed through `nca add` are listed here.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::error::PluginError;
use crate::manifest::Manifest;

pub const REGISTRY_FILE: &str = "registry.json";

/// Directory holding the registry and global plugin installs.
/// Override with `NCA_PLUGIN_HOME` (used by tests).
pub fn plugin_home() -> Result<PathBuf, PluginError> {
    if let Some(dir) = std::env::var_os("NCA_PLUGIN_HOME")
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    nca_common::config::user_home_dir()
        .map(|h| h.join(".nca").join("plugins"))
        .ok_or_else(|| PluginError::Registry("cannot determine home directory".into()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub version: String,
    /// Source exactly as the user typed it, without the `@ref` suffix.
    pub source: String,
    #[serde(default)]
    pub requested_ref: Option<String>,
    pub commit: String,
    pub wasm_sha256: String,
    /// Hash of the approved manifest; a changed manifest needs re-approval.
    pub manifest_sha256: String,
    pub installed_at: String,
    pub enabled: bool,
    /// `Some(path)` for project-scoped installs: only loaded in that workspace.
    #[serde(default)]
    pub workspace: Option<String>,
    /// Install directory holding `nca-plugin.toml` and the wasm file.
    pub dir: PathBuf,
}

impl Entry {
    pub fn applies_to(&self, workspace: &Path) -> bool {
        match &self.workspace {
            None => true,
            Some(w) => Path::new(w) == workspace,
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dir.join("data")
    }

    /// Re-read the installed manifest + wasm and verify them against the hashes
    /// recorded at install time. Detects tampering after approval.
    pub fn load_verified(&self) -> Result<(Manifest, Vec<u8>), PluginError> {
        let manifest_text = std::fs::read_to_string(self.dir.join(crate::manifest::MANIFEST_FILE))
            .map_err(|e| PluginError::Registry(format!("{}: manifest: {e}", self.name)))?;
        if sha256_hex(manifest_text.as_bytes()) != self.manifest_sha256 {
            return Err(PluginError::Registry(format!(
                "{}: manifest changed since it was approved; reinstall with `nca plugin update`",
                self.name
            )));
        }
        let manifest = Manifest::parse(&manifest_text)?;
        let wasm = std::fs::read(self.dir.join(&manifest.plugin.wasm))
            .map_err(|e| PluginError::Registry(format!("{}: wasm: {e}", self.name)))?;
        if sha256_hex(&wasm) != self.wasm_sha256 {
            return Err(PluginError::Registry(format!(
                "{}: wasm hash mismatch (expected {}); refusing to load",
                self.name, self.wasm_sha256
            )));
        }
        Ok((manifest, wasm))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub plugins: Vec<Entry>,
}

impl Registry {
    pub fn path() -> Result<PathBuf, PluginError> {
        Ok(plugin_home()?.join(REGISTRY_FILE))
    }

    pub fn load() -> Result<Self, PluginError> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| PluginError::Registry(format!("read {}: {e}", path.display())))?;
        serde_json::from_str(&text)
            .map_err(|e| PluginError::Registry(format!("parse {}: {e}", path.display())))
    }

    pub fn save(&self) -> Result<(), PluginError> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| PluginError::Registry(format!("create {}: {e}", parent.display())))?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|e| PluginError::Registry(e.to_string()))?;
        // Write-then-rename so a crash never leaves a truncated registry.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| PluginError::Registry(format!("write {}: {e}", path.display())))
    }

    /// Find by name within the scope: project entries shadow global ones.
    pub fn find(&self, name: &str, workspace: Option<&Path>) -> Option<&Entry> {
        let scoped =
            |e: &&Entry| e.name == name && e.workspace.as_deref().map(Path::new) == workspace;
        self.plugins.iter().find(scoped)
    }

    pub fn find_mut(&mut self, name: &str, workspace: Option<&Path>) -> Option<&mut Entry> {
        self.plugins
            .iter_mut()
            .find(|e| e.name == name && e.workspace.as_deref().map(Path::new) == workspace)
    }

    pub fn upsert(&mut self, entry: Entry) {
        let ws = entry.workspace.clone();
        self.plugins
            .retain(|e| !(e.name == entry.name && e.workspace == ws));
        self.plugins.push(entry);
    }

    pub fn remove(&mut self, name: &str, workspace: Option<&Path>) -> Option<Entry> {
        let idx = self
            .plugins
            .iter()
            .position(|e| e.name == name && e.workspace.as_deref().map(Path::new) == workspace)?;
        Some(self.plugins.remove(idx))
    }
}
