//! Workspace memory note storage.

use crate::cmd::util::print_json;
use nca_common::config::NcaConfig;
use nca_runtime::memory_store::{MemoryNote, MemoryStore};
use std::path::Path;

pub fn workspace_memory_store(config: &NcaConfig, workspace_root: &Path) -> MemoryStore {
    if config.memory.file_path.is_absolute() {
        MemoryStore::new(config.memory.file_path.clone())
    } else {
        MemoryStore::new(workspace_root.join(&config.memory.file_path))
    }
}

pub async fn show_memory(
    config: &NcaConfig,
    workspace_root: &Path,
    json: bool,
) -> anyhow::Result<()> {
    let store = workspace_memory_store(config, workspace_root);
    let state = store.load().await.map_err(anyhow::Error::msg)?;
    if json {
        print_json(&state, false)?;
    } else if state.notes.is_empty() {
        println!("No memory notes stored");
    } else {
        for note in state.notes {
            println!(
                "{}  {}  {}",
                note.id,
                note.kind,
                note.title.unwrap_or_else(|| note.created_at.to_rfc3339())
            );
            println!("  {}", note.content.replace('\n', " "));
        }
    }
    Ok(())
}

pub async fn add_memory_note(
    config: &NcaConfig,
    workspace_root: &Path,
    kind: &str,
    text: &str,
    json: bool,
) -> anyhow::Result<()> {
    let store = workspace_memory_store(config, workspace_root);
    let note = MemoryNote {
        id: format!("{}-{}", kind, chrono::Utc::now().timestamp_millis()),
        created_at: chrono::Utc::now(),
        kind: kind.to_string(),
        title: None,
        content: text.trim().to_string(),
    };
    let state = store
        .append_note(note.clone(), config.memory.max_notes)
        .await
        .map_err(anyhow::Error::msg)?;
    if json {
        print_json(&note, false)?;
    } else {
        println!("Stored memory note {} ({})", note.id, kind);
        println!("Memory path: {}", store.path().display());
        println!("Total notes: {}", state.notes.len());
    }
    Ok(())
}
