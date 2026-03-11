//! Discovers saved sessions from `.nca/sessions/*.json` across all registered
//! workspaces and detects live sockets in the runtime directory.

use nca_common::session::{SessionMeta, SessionStatus};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A session entry in the index, merging disk metadata with live socket status.
#[derive(Debug, Clone)]
pub struct IndexedSession {
    pub id: String,
    pub meta: Option<SessionMeta>,
    pub is_live: bool,
    pub workspace: PathBuf,
}

/// Discovers sessions across multiple workspaces and the runtime socket dir.
#[derive(Debug)]
pub struct SessionIndex {
    workspaces: Vec<PathBuf>,
    runtime_dir: PathBuf,
    last_refresh: Option<Instant>,
    cached: Vec<IndexedSession>,
    refresh_interval_secs: u64,
}

impl SessionIndex {
    /// Create a session index for a single workspace (legacy mode).
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            workspaces: vec![workspace.as_ref().to_path_buf()],
            runtime_dir: runtime_socket_dir(),
            last_refresh: None,
            cached: Vec::new(),
            refresh_interval_secs: 2,
        }
    }

    /// Create a session index that scans multiple workspaces.
    pub fn multi(workspaces: Vec<PathBuf>) -> Self {
        Self {
            workspaces,
            runtime_dir: runtime_socket_dir(),
            last_refresh: None,
            cached: Vec::new(),
            refresh_interval_secs: 2,
        }
    }

    /// Replace the set of tracked workspaces.
    pub fn set_workspaces(&mut self, workspaces: Vec<PathBuf>) {
        if self.workspaces != workspaces {
            self.workspaces = workspaces;
            self.last_refresh = None;
        }
    }

    /// Refresh if enough time has passed since last refresh.
    pub fn maybe_refresh(&mut self) -> &[IndexedSession] {
        let now = Instant::now();
        let should_refresh = self
            .last_refresh
            .map(|t| now.duration_since(t).as_secs() >= self.refresh_interval_secs)
            .unwrap_or(true);
        if should_refresh {
            self.last_refresh = Some(now);
            self.cached = self.do_refresh();
        }
        &self.cached
    }

    /// Force a full refresh.
    pub fn refresh(&mut self) -> &[IndexedSession] {
        self.last_refresh = Some(Instant::now());
        self.cached = self.do_refresh();
        &self.cached
    }

    fn do_refresh(&self) -> Vec<IndexedSession> {
        let mut by_id: std::collections::HashMap<String, IndexedSession> =
            std::collections::HashMap::new();

        for workspace in &self.workspaces {
            scan_sessions_dir(workspace, workspace, &mut by_id);

            let worktrees_dir = workspace.join(".nca").join("worktrees");
            if worktrees_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&worktrees_dir) {
                    for entry in entries.flatten() {
                        let wt_path = entry.path();
                        if wt_path.is_dir() {
                            scan_sessions_dir(&wt_path, workspace, &mut by_id);
                        }
                    }
                }
            }
        }

        // Scan runtime dir for live sockets not yet in index
        if self.runtime_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&self.runtime_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "sock") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            if !by_id.contains_key(stem) {
                                let workspace = self
                                    .workspaces
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| PathBuf::from("."));
                                by_id.insert(
                                    stem.to_string(),
                                    IndexedSession {
                                        id: stem.to_string(),
                                        meta: None,
                                        is_live: true,
                                        workspace,
                                    },
                                );
                            } else if let Some(s) = by_id.get_mut(stem) {
                                s.is_live = true;
                            }
                        }
                    }
                }
            }
        }

        let mut sessions: Vec<_> = by_id.into_values().collect();
        sessions.sort_by(|a, b| {
            let a_ts = a.meta.as_ref().map(|m| m.updated_at.timestamp());
            let b_ts = b.meta.as_ref().map(|m| m.updated_at.timestamp());
            b_ts.cmp(&a_ts)
        });
        sessions
    }
}

fn scan_sessions_dir(
    root: &Path,
    workspace: &Path,
    by_id: &mut std::collections::HashMap<String, IndexedSession>,
) {
    let sessions_dir = root.join(".nca").join("sessions");
    if !sessions_dir.exists() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let meta = load_session_meta(&path);
                    let is_live = meta
                        .as_ref()
                        .and_then(|m| m.socket_path.as_ref())
                        .map(|p| p.exists())
                        .unwrap_or(false);
                    by_id.insert(
                        stem.to_string(),
                        IndexedSession {
                            id: stem.to_string(),
                            meta,
                            is_live,
                            workspace: workspace.to_path_buf(),
                        },
                    );
                }
            }
        }
    }
}

fn runtime_socket_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("nca")
}

fn load_session_meta(path: &Path) -> Option<SessionMeta> {
    let json = std::fs::read_to_string(path).ok()?;
    #[derive(serde::Deserialize)]
    struct Wrapper {
        meta: SessionMeta,
    }
    serde_json::from_str::<Wrapper>(&json)
        .map(|w| w.meta)
        .or_else(|_| serde_json::from_str::<SessionMeta>(&json))
        .ok()
}

impl IndexedSession {
    pub fn status_display(&self) -> &'static str {
        if self.is_live {
            "live"
        } else if let Some(ref m) = self.meta {
            match m.status {
                SessionStatus::Running => "running",
                SessionStatus::Completed => "done",
                SessionStatus::Error => "error",
                SessionStatus::Cancelled => "cancelled",
            }
        } else {
            "unknown"
        }
    }

    pub fn model_display(&self) -> String {
        self.meta
            .as_ref()
            .map(|m| m.model.clone())
            .unwrap_or_else(|| "\u{2014}".to_string())
    }

    pub fn updated_display(&self) -> String {
        self.meta
            .as_ref()
            .map(|m| m.updated_at.format("%H:%M %m/%d").to_string())
            .unwrap_or_else(|| "\u{2014}".to_string())
    }

    pub fn workspace_name(&self) -> String {
        self.workspace
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    }

    pub fn socket_path(&self) -> Option<PathBuf> {
        if self.is_live {
            self.meta
                .as_ref()
                .and_then(|m| m.socket_path.clone())
                .or_else(|| {
                    let p = runtime_socket_dir().join(format!("{}.sock", self.id));
                    if p.exists() { Some(p) } else { None }
                })
        } else {
            None
        }
    }

    pub fn events_path(&self) -> PathBuf {
        self.workspace
            .join(".nca")
            .join("sessions")
            .join(format!("{}.events.jsonl", self.id))
    }
}
