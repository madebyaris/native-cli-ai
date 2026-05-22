//! Filesystem paths and workspace-scoped cache helpers.

use std::env;
use std::path::{Path, PathBuf};

/// `$HOME/.nca/config.toml` when `HOME` is set.
#[must_use]
pub fn global_config_path() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".nca/config.toml"))
}

/// `$HOME/.nca` when `HOME` is set.
#[must_use]
pub fn nca_home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".nca"))
}

/// Per-workspace `config.local.toml` path.
#[must_use]
pub fn workspace_config_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".nca").join("config.local.toml")
}

/// Stable per-workspace id: `{slug}-{hex}` derived from the canonical workspace path.
///
/// # Errors
///
/// Returns [`WorkspaceCacheError::Canonicalize`] if the workspace path cannot be
/// canonicalized.
pub fn workspace_cache_id(workspace_root: &Path) -> Result<(String, PathBuf), WorkspaceCacheError> {
    let canonical =
        workspace_root
            .canonicalize()
            .map_err(|source| WorkspaceCacheError::Canonicalize {
                path: workspace_root.to_path_buf(),
                source,
            })?;
    let path_str = canonical.to_string_lossy();
    let suffix = workspace_path_hash_suffix(path_str.as_ref());
    let slug = workspace_dir_slug(&canonical);
    Ok((format!("{slug}-{suffix}"), canonical))
}

/// `~/.nca/workspaces/<workspace-id>/`
///
/// # Errors
///
/// Returns [`WorkspaceCacheError::NoHomeDir`] when `HOME` is unset, or forwards
/// canonicalization errors from [`workspace_cache_id`].
pub fn workspace_cache_dir(workspace_root: &Path) -> Result<PathBuf, WorkspaceCacheError> {
    let (id, _) = workspace_cache_id(workspace_root)?;
    let home = nca_home_dir().ok_or(WorkspaceCacheError::NoHomeDir)?;
    Ok(home.join("workspaces").join(id))
}

/// Cached CLI index JSON for this workspace.
///
/// # Errors
///
/// Forwards errors from [`workspace_cache_dir`].
pub fn workspace_cli_index_path(workspace_root: &Path) -> Result<PathBuf, WorkspaceCacheError> {
    Ok(workspace_cache_dir(workspace_root)?.join("cli-index.json"))
}

fn workspace_dir_slug(path: &Path) -> String {
    let raw = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace")
        .to_ascii_lowercase();
    let mut out = String::new();
    let mut prev_sep = false;
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_sep = false;
        } else if !out.is_empty() && !prev_sep {
            out.push('-');
            prev_sep = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed
    }
}

fn workspace_path_hash_suffix(canonical_path: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(canonical_path.as_bytes());
    let digest = hasher.finalize();
    // 16 hex chars — stable across Rust versions (unlike std::collections::hash_map::DefaultHasher).
    format!("{digest:x}")[..16].to_string()
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceCacheError {
    #[error("HOME is not set")]
    NoHomeDir,
    #[error("failed to canonicalize workspace path {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        source: std::io::Error,
    },
}
