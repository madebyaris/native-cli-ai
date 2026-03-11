//! Discovers saved sessions from `.nca/sessions/*.json` across all registered
//! workspaces and detects live sockets in the runtime directory.

use nca_common::session::{SessionMeta, SessionStatus};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A session entry in the index, merging disk metadata with live socket status.
#[derive(Debug, Clone)]
pub struct IndexedSession {
    pub id: String,
    pub meta: Option<SessionMeta>,
    pub is_live: bool,
    pub workspace: PathBuf,
    pub last_action: Option<String>,
}

/// Discovers sessions across multiple workspaces and the runtime socket dir.
#[derive(Debug)]
pub struct SessionIndex {
    workspaces: Vec<PathBuf>,
    runtime_dir: PathBuf,
    last_refresh: Option<Instant>,
    cached: Vec<IndexedSession>,
    refresh_interval_secs: u64,
    file_cache: HashMap<PathBuf, CachedSessionData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileSig {
    modified_secs: u64,
    len: u64,
}

#[derive(Debug, Clone)]
struct CachedSessionData {
    session_sig: Option<FileSig>,
    events_sig: Option<FileSig>,
    meta: Option<SessionMeta>,
    last_action: Option<String>,
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
            file_cache: HashMap::new(),
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
            file_cache: HashMap::new(),
        }
    }

    /// Replace the set of tracked workspaces.
    pub fn set_workspaces(&mut self, workspaces: Vec<PathBuf>) {
        if self.workspaces != workspaces {
            self.workspaces = workspaces;
            self.last_refresh = None;
            self.file_cache.clear();
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

    fn do_refresh(&mut self) -> Vec<IndexedSession> {
        let mut by_id: HashMap<String, IndexedSession> = HashMap::new();
        let mut seen_paths = HashSet::new();

        let workspaces = self.workspaces.clone();
        for workspace in &workspaces {
            self.scan_sessions_dir(workspace, workspace, &mut by_id, &mut seen_paths);

            let worktrees_dir = workspace.join(".nca").join("worktrees");
            if worktrees_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&worktrees_dir) {
                    for entry in entries.flatten() {
                        let wt_path = entry.path();
                        if wt_path.is_dir() {
                            self.scan_sessions_dir(
                                &wt_path,
                                workspace,
                                &mut by_id,
                                &mut seen_paths,
                            );
                        }
                    }
                }
            }
        }

        self.file_cache.retain(|path, _| seen_paths.contains(path));

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
                                        last_action: None,
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

    fn scan_sessions_dir(
        &mut self,
        root: &Path,
        workspace: &Path,
        by_id: &mut HashMap<String, IndexedSession>,
        seen_paths: &mut HashSet<PathBuf>,
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
                        seen_paths.insert(path.clone());
                        let (meta, last_action) =
                            self.session_data_for_path(&path, workspace, stem);
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
                                last_action,
                            },
                        );
                    }
                }
            }
        }
    }

    fn session_data_for_path(
        &mut self,
        session_json_path: &Path,
        workspace: &Path,
        session_id: &str,
    ) -> (Option<SessionMeta>, Option<String>) {
        let session_sig = file_sig(session_json_path);
        let events_path = workspace
            .join(".nca")
            .join("sessions")
            .join(format!("{session_id}.events.jsonl"));
        let events_sig = file_sig(&events_path);

        if let Some(cached) = self.file_cache.get(session_json_path) {
            if cached.session_sig == session_sig && cached.events_sig == events_sig {
                return (cached.meta.clone(), cached.last_action.clone());
            }
        }

        let meta = load_session_meta(session_json_path);
        let last_action = load_last_action(workspace, session_id);
        self.file_cache.insert(
            session_json_path.to_path_buf(),
            CachedSessionData {
                session_sig,
                events_sig,
                meta: meta.clone(),
                last_action: last_action.clone(),
            },
        );

        (meta, last_action)
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

fn load_last_action(workspace: &Path, session_id: &str) -> Option<String> {
    use nca_common::event::{AgentEvent, EventEnvelope};

    let events_path = workspace
        .join(".nca")
        .join("sessions")
        .join(format!("{session_id}.events.jsonl"));
    let data = std::fs::read_to_string(events_path).ok()?;
    let mut last = None;
    for line in data.lines() {
        let event = match serde_json::from_str::<EventEnvelope>(line)
            .map(|envelope| envelope.event)
            .or_else(|_| serde_json::from_str::<AgentEvent>(line))
        {
            Ok(event) => event,
            Err(_) => continue,
        };
        let action = match event {
            AgentEvent::TokensStreamed { .. } => Some("Streaming response".to_string()),
            AgentEvent::ToolCallStarted { tool, .. } => Some(format!("Running tool: {tool}")),
            AgentEvent::ApprovalRequested { tool, .. } => Some(format!("Waiting approval: {tool}")),
            AgentEvent::ChildSessionSpawned {
                child_session_id, ..
            } => Some(format!("Sub-agent started: {child_session_id}")),
            AgentEvent::ChildSessionCompleted {
                child_session_id,
                status,
                ..
            } => Some(format!("Sub-agent {child_session_id}: {status}")),
            AgentEvent::Checkpoint { detail, .. } => Some(detail),
            AgentEvent::SessionEnded { reason } => Some(format!("Session ended: {:?}", reason)),
            AgentEvent::Error { message } => Some(format!("Error: {message}")),
            _ => None,
        };
        if let Some(action) = action {
            last = Some(action);
        }
    }
    last
}

fn file_sig(path: &Path) -> Option<FileSig> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified_secs = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|dur| dur.as_secs())
        .unwrap_or(0);
    Some(FileSig {
        modified_secs,
        len: metadata.len(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nca_common::session::{SessionMeta, SessionState, SessionStatus};
    use tempfile::tempdir;

    #[test]
    fn refresh_updates_cached_last_action_when_events_change() {
        let temp = tempdir().unwrap();
        let workspace = temp.path();
        let sessions_dir = workspace.join(".nca").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let session_id = "session-cache-test";
        let state = SessionState {
            meta: SessionMeta {
                id: session_id.into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                workspace: workspace.to_path_buf(),
                model: "MiniMax-M2.5".into(),
                status: SessionStatus::Running,
                pid: None,
                socket_path: None,
                worktree_path: None,
                branch: None,
                base_branch: None,
                parent_session_id: None,
                child_session_ids: Vec::new(),
                inherited_summary: None,
                spawn_reason: None,
            },
            messages: Vec::new(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            estimated_cost_usd: 0.0,
        };

        let session_file = sessions_dir.join(format!("{session_id}.json"));
        std::fs::write(&session_file, serde_json::to_string_pretty(&state).unwrap()).unwrap();

        let events_file = sessions_dir.join(format!("{session_id}.events.jsonl"));
        std::fs::write(
            &events_file,
            r#"{"type":"ToolCallStarted","call_id":"c1","tool":"read_file","input":{"path":"README.md"}}"#,
        )
        .unwrap();

        let mut index = SessionIndex::new(workspace);
        let sessions = index.refresh().to_vec();
        let first_action = sessions
            .iter()
            .find(|s| s.id == session_id)
            .and_then(|s| s.last_action.clone())
            .unwrap();
        assert_eq!(first_action, "Running tool: read_file");

        std::thread::sleep(std::time::Duration::from_secs(1));
        std::fs::write(
            &events_file,
            r#"{"type":"ApprovalRequested","call_id":"c2","tool":"execute_bash","description":"needs approval"}"#,
        )
        .unwrap();

        let sessions = index.refresh().to_vec();
        let second_action = sessions
            .iter()
            .find(|s| s.id == session_id)
            .and_then(|s| s.last_action.clone())
            .unwrap();
        assert_eq!(second_action, "Waiting approval: execute_bash");
    }
}
