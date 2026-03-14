//! Desktop workspace management: tracks registered workspaces, provides
//! workspace-level navigation, and feeds the session index.

use std::path::PathBuf;

use nca_runtime::workspace_registry::{RegisteredWorkspace, WorkspaceRegistry};

/// In-memory workspace state for the desktop app.
/// Wraps the persisted `WorkspaceRegistry` from the runtime crate
/// (via JSON file) without importing runtime directly -- we read/write
/// the same JSON file format.
#[derive(Debug, Default)]
pub struct WorkspaceManager {
    pub workspaces: Vec<WorkspaceEntry>,
    pub selected_workspace: Option<usize>,
}

pub type WorkspaceEntry = RegisteredWorkspace;

impl WorkspaceManager {
    pub fn load() -> Self {
        let workspaces = WorkspaceRegistry::load().workspaces;

        Self {
            workspaces,
            selected_workspace: None,
        }
    }

    pub fn save(&self) {
        let registry = WorkspaceRegistry {
            workspaces: self.workspaces.clone(),
        };
        let _ = registry.save();
    }

    pub fn add_workspace(&mut self, path: PathBuf) {
        let mut registry = WorkspaceRegistry {
            workspaces: self.workspaces.clone(),
        };
        registry.add(&path);
        self.workspaces = registry.workspaces;
        self.save();
    }

    #[allow(dead_code)]
    pub fn remove_workspace(&mut self, idx: usize) {
        if idx < self.workspaces.len() {
            let path = self.workspaces[idx].path.clone();
            let mut registry = WorkspaceRegistry {
                workspaces: self.workspaces.clone(),
            };
            registry.remove(path);
            self.workspaces = registry.workspaces;
            if self.selected_workspace == Some(idx) {
                self.selected_workspace = None;
            }
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
