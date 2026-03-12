//! Desktop workspace management: tracks registered workspaces, provides
//! workspace-level navigation, and feeds the session index.

use std::path::PathBuf;

/// In-memory workspace state for the desktop app.
/// Wraps the persisted `WorkspaceRegistry` from the runtime crate
/// (via JSON file) without importing runtime directly -- we read/write
/// the same JSON file format.
#[derive(Debug, Default)]
pub struct WorkspaceManager {
    pub workspaces: Vec<WorkspaceEntry>,
    pub selected_workspace: Option<usize>,
    dirty: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceEntry {
    pub path: PathBuf,
    pub name: String,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub last_opened_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct RegistryFile {
    workspaces: Vec<WorkspaceEntry>,
}

impl WorkspaceManager {
    pub fn load() -> Self {
        let path = registry_path();
        let workspaces = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|json| serde_json::from_str::<RegistryFile>(&json).ok())
                .map(|r| r.workspaces)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        Self {
            workspaces,
            selected_workspace: None,
            dirty: false,
        }
    }

    pub fn save(&self) {
        let path = registry_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let registry = RegistryFile {
            workspaces: self.workspaces.clone(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&registry) {
            let _ = std::fs::write(&path, json);
        }
    }

    pub fn add_workspace(&mut self, path: PathBuf) {
        let now = chrono::Utc::now();
        if let Some(existing) = self.workspaces.iter_mut().find(|w| w.path == path) {
            existing.last_opened_at = now;
        } else {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            self.workspaces.push(WorkspaceEntry {
                path,
                name,
                added_at: now,
                last_opened_at: now,
            });
        }
        self.dirty = true;
        self.save();
    }

    pub fn remove_workspace(&mut self, idx: usize) {
        if idx < self.workspaces.len() {
            self.workspaces.remove(idx);
            if self.selected_workspace == Some(idx) {
                self.selected_workspace = None;
            }
            self.dirty = true;
            self.save();
        }
    }

    pub fn select(&mut self, idx: Option<usize>) {
        self.selected_workspace = idx;
        if let Some(i) = idx {
            if let Some(w) = self.workspaces.get_mut(i) {
                w.last_opened_at = chrono::Utc::now();
                self.save();
            }
        }
    }

    pub fn selected_path(&self) -> Option<&PathBuf> {
        self.selected_workspace
            .and_then(|i| self.workspaces.get(i))
            .map(|w| &w.path)
    }

    /// Sort workspaces by most recently opened.
    pub fn sort_by_recent(&mut self) {
        self.workspaces
            .sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at));
    }
}

fn registry_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".nca").join("workspaces.json")
}
