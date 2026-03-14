use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryNote {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryState {
    #[serde(default)]
    pub notes: Vec<MemoryNote>,
}

pub struct MemoryStore {
    path: PathBuf,
}

impl MemoryStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn load(&self) -> Result<MemoryState, String> {
        let raw = match tokio::fs::read_to_string(&self.path).await {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(MemoryState::default());
            }
            Err(err) => return Err(err.to_string()),
        };
        serde_json::from_str(&raw).map_err(|err| err.to_string())
    }

    pub async fn save(&self, state: &MemoryState) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| err.to_string())?;
        }
        let raw = serde_json::to_string_pretty(state).map_err(|err| err.to_string())?;
        tokio::fs::write(&self.path, raw)
            .await
            .map_err(|err| err.to_string())
    }

    pub async fn append_note(
        &self,
        note: MemoryNote,
        max_notes: usize,
    ) -> Result<MemoryState, String> {
        let mut state = self.load().await?;
        state.notes.push(note);
        if state.notes.len() > max_notes {
            let drain = state.notes.len() - max_notes;
            state.notes.drain(0..drain);
        }
        self.save(&state).await?;
        Ok(state)
    }
}
