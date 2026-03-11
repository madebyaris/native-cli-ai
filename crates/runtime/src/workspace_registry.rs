use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A registered workspace that the desktop app tracks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredWorkspace {
    pub path: PathBuf,
    pub name: String,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub last_opened_at: chrono::DateTime<chrono::Utc>,
}

/// Persisted registry of workspaces the user has opened.
/// Stored at `~/.nca/workspaces.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceRegistry {
    pub workspaces: Vec<RegisteredWorkspace>,
}

impl WorkspaceRegistry {
    fn registry_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".nca").join("workspaces.json")
    }

    /// Load the registry from disk. Returns an empty registry if the file doesn't exist.
    pub fn load() -> Self {
        let path = Self::registry_path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save the registry to disk.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::registry_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())
    }

    /// Add a workspace to the registry. If it already exists, update last_opened_at.
    pub fn add(&mut self, path: impl AsRef<Path>) -> &RegisteredWorkspace {
        let path = path.as_ref().to_path_buf();
        let now = chrono::Utc::now();

        if let Some(existing) = self.workspaces.iter_mut().find(|w| w.path == path) {
            existing.last_opened_at = now;
            let idx = self.workspaces.iter().position(|w| w.path == path).unwrap();
            return &self.workspaces[idx];
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        self.workspaces.push(RegisteredWorkspace {
            path: path.clone(),
            name,
            added_at: now,
            last_opened_at: now,
        });

        self.workspaces.last().unwrap()
    }

    /// Remove a workspace from the registry.
    pub fn remove(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        self.workspaces.retain(|w| w.path != path);
    }

    /// Get all registered workspaces, sorted by most recently opened.
    pub fn recent(&self) -> Vec<&RegisteredWorkspace> {
        let mut sorted: Vec<_> = self.workspaces.iter().collect();
        sorted.sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at));
        sorted
    }

    /// Find a workspace by path.
    pub fn find(&self, path: impl AsRef<Path>) -> Option<&RegisteredWorkspace> {
        let path = path.as_ref();
        self.workspaces.iter().find(|w| w.path == path)
    }
}
