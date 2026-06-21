//! Workspace filesystem abstraction.
//!
//! Provides a [`WorkspaceFs`] trait that encapsulates workspace-sandbox path
//! validation and file I/O. Tools receive `Arc<dyn WorkspaceFs>` instead of a
//! bare `PathBuf`, concentrating all security-critical path logic in one module.
//!
//! Two resolution strategies:
//! - [`WorkspaceFs::resolve`] — canonicalizes (follows symlinks) + boundary check.
//!   Use for paths that must exist on disk.
//! - [`WorkspaceFs::validate_prefix`] — logical normalization (handles `..`/`.` segments)
//!   without canonicalization. Use for paths that may not exist yet.

use async_trait::async_trait;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// Errors produced by workspace filesystem operations.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The path is (or resolves to) a location outside the workspace root.
    #[error("path '{path}' is outside the workspace")]
    OutsideWorkspace { path: String },

    /// The path could not be resolved (e.g. does not exist for a `resolve` call).
    #[error("path '{path}' not found: {source}")]
    NotFound {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// A generic I/O error on a workspace path.
    #[error("I/O error on '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// The path is syntactically invalid (e.g. empty, all dots).
    #[error("invalid path: {0}")]
    InvalidPath(String),
}

/// A directory entry returned by [`WorkspaceFs::read_dir`].
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Abstraction over workspace-scoped filesystem access.
///
/// All path arguments are **relative to the workspace root**. The
/// implementation is responsible for joining, validating, and — for existing
/// paths — canonicalizing to prevent symlink escapes.
///
/// # Adapters
///
/// - **Production:** [`RealFs`] — delegates to `tokio::fs` with path sandbox.
/// - **Testing:** inject a mock or in-memory adapter (not in this crate; tests
///   construct `RealFs` over `tempfile::tempdir()` today).
#[async_trait]
pub trait WorkspaceFs: Send + Sync {
    /// The workspace root path. Used by tools that shell out to external
    /// processes and need a `current_dir`.
    fn root(&self) -> &Path;

    /// Resolve an **existing** path inside the workspace.
    ///
    /// Canonicalizes the path (follows symlinks) and verifies the result stays
    /// within the workspace root. Returns an error if the path does not exist
    /// or escapes the workspace.
    fn resolve(&self, path: &str) -> Result<PathBuf, SandboxError>;

    /// Validate that a **possibly non-existent** path would stay within the
    /// workspace.
    ///
    /// Does **not** canonicalize (the path may not exist). Instead, it performs
    /// logical normalization of `.` and `..` segments and checks the result
    /// starts with the workspace root.
    fn validate_prefix(&self, path: &str) -> Result<PathBuf, SandboxError>;

    // ── File I/O ──────────────────────────────────────────────────────

    /// Read the entire contents of a file.
    async fn read_file(&self, path: &str) -> Result<String, SandboxError>;

    /// Create or overwrite a file, creating parent directories as needed.
    async fn write_file(&self, path: &str, content: &str) -> Result<(), SandboxError>;

    /// List entries in a directory.
    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, SandboxError>;

    /// Create a directory and all parent directories.
    async fn create_dir_all(&self, path: &str) -> Result<(), SandboxError>;

    /// Remove a file.
    async fn remove_file(&self, path: &str) -> Result<(), SandboxError>;

    /// Remove a directory and all its contents.
    async fn remove_dir_all(&self, path: &str) -> Result<(), SandboxError>;

    /// Rename (move) a file or directory.
    async fn rename(&self, from: &str, to: &str) -> Result<(), SandboxError>;

    /// Copy a file.
    async fn copy(&self, from: &str, to: &str) -> Result<(), SandboxError>;
}

// ---------------------------------------------------------------------------
// RealFs — production adapter backed by tokio::fs
// ---------------------------------------------------------------------------

/// Production filesystem adapter that enforces workspace-sandbox boundaries.
pub struct RealFs {
    root: PathBuf,
    canonical_cache: OnceLock<PathBuf>,
}

impl RealFs {
    /// Create a new `RealFs` rooted at `root`.
    ///
    /// `root` is canonicalized if possible; if canonicalization fails (e.g. the
    /// directory doesn't exist yet), the raw path is used for prefix checks and
    /// canonicalization is retried on each `resolve` call.
    pub fn new(root: PathBuf) -> Self {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        Self {
            root,
            canonical_cache: OnceLock::from(canonical),
        }
    }

    /// Return the cached canonical workspace root.
    fn cached_canonical_root(&self) -> &Path {
        self.canonical_cache
            .get()
            .map(|p| p.as_path())
            .unwrap_or(&self.root)
    }
}

#[async_trait]
impl WorkspaceFs for RealFs {
    fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, SandboxError> {
        let full = self.root.join(path);
        let canonical = full.canonicalize().map_err(|e| SandboxError::NotFound {
            path: full.display().to_string(),
            source: e,
        })?;
        // Re-canonicalize root in case it was initially unavailable.
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.cached_canonical_root().to_path_buf());
        if canonical.starts_with(&root) {
            Ok(canonical)
        } else {
            Err(SandboxError::OutsideWorkspace {
                path: path.to_string(),
            })
        }
    }

    fn validate_prefix(&self, path: &str) -> Result<PathBuf, SandboxError> {
        let full = self.root.join(path);
        let normalized = logical_normalize(&full);
        if normalized.starts_with(self.cached_canonical_root()) {
            Ok(normalized)
        } else {
            Err(SandboxError::OutsideWorkspace {
                path: path.to_string(),
            })
        }
    }

    async fn read_file(&self, path: &str) -> Result<String, SandboxError> {
        let canonical = self.resolve(path)?;
        tokio::fs::read_to_string(&canonical)
            .await
            .map_err(|e| SandboxError::Io {
                path: canonical.display().to_string(),
                source: e,
            })
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), SandboxError> {
        let full = self.root.join(path);
        let parent = full
            .parent()
            .ok_or_else(|| SandboxError::InvalidPath(path.to_string()))?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| SandboxError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        // Validate the now-existing parent is within the workspace.
        let canonical_parent = parent.canonicalize().map_err(|e| SandboxError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.cached_canonical_root().to_path_buf());
        if !canonical_parent.starts_with(&root) {
            return Err(SandboxError::OutsideWorkspace {
                path: path.to_string(),
            });
        }
        tokio::fs::write(&full, content)
            .await
            .map_err(|e| SandboxError::Io {
                path: full.display().to_string(),
                source: e,
            })
    }

    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, SandboxError> {
        let canonical = self.resolve(path)?;
        let mut entries = tokio::fs::read_dir(&canonical)
            .await
            .map_err(|e| SandboxError::Io {
                path: canonical.display().to_string(),
                source: e,
            })?;
        let mut result = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|e| SandboxError::Io {
            path: canonical.display().to_string(),
            source: e,
        })? {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry
                .file_type()
                .await
                .map(|ft| ft.is_dir())
                .unwrap_or(false);
            result.push(DirEntry { name, is_dir });
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    async fn create_dir_all(&self, path: &str) -> Result<(), SandboxError> {
        let validated = self.validate_prefix(path)?;
        // The path may be "." or the root itself; canonicalize what we can.
        let canonical = validated
            .canonicalize()
            .unwrap_or_else(|_| validated.clone());
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.cached_canonical_root().to_path_buf());
        if !canonical.starts_with(&root) {
            return Err(SandboxError::OutsideWorkspace {
                path: path.to_string(),
            });
        }
        tokio::fs::create_dir_all(&validated)
            .await
            .map_err(|e| SandboxError::Io {
                path: validated.display().to_string(),
                source: e,
            })
    }

    async fn remove_file(&self, path: &str) -> Result<(), SandboxError> {
        let canonical = self.resolve(path)?;
        tokio::fs::remove_file(&canonical)
            .await
            .map_err(|e| SandboxError::Io {
                path: canonical.display().to_string(),
                source: e,
            })
    }

    async fn remove_dir_all(&self, path: &str) -> Result<(), SandboxError> {
        let canonical = self.resolve(path)?;
        tokio::fs::remove_dir_all(&canonical)
            .await
            .map_err(|e| SandboxError::Io {
                path: canonical.display().to_string(),
                source: e,
            })
    }

    async fn rename(&self, from: &str, to: &str) -> Result<(), SandboxError> {
        // Source must exist and be in workspace.
        let canonical_from = self.resolve(from)?;
        // Destination parent must be in workspace (create if needed).
        let full_to = self.root.join(to);
        if let Some(parent) = full_to.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SandboxError::Io {
                    path: parent.display().to_string(),
                    source: e,
                })?;
            let canonical_parent = parent.canonicalize().map_err(|e| SandboxError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
            let root = self
                .root
                .canonicalize()
                .unwrap_or_else(|_| self.cached_canonical_root().to_path_buf());
            if !canonical_parent.starts_with(&root) {
                return Err(SandboxError::OutsideWorkspace {
                    path: to.to_string(),
                });
            }
        }
        tokio::fs::rename(&canonical_from, &full_to)
            .await
            .map_err(|e| SandboxError::Io {
                path: format!("{from} -> {to}"),
                source: e,
            })
    }

    async fn copy(&self, from: &str, to: &str) -> Result<(), SandboxError> {
        // Source must exist and be in workspace.
        let canonical_from = self.resolve(from)?;
        // Destination parent must be in workspace (create if needed).
        let full_to = self.root.join(to);
        if let Some(parent) = full_to.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SandboxError::Io {
                    path: parent.display().to_string(),
                    source: e,
                })?;
            let canonical_parent = parent.canonicalize().map_err(|e| SandboxError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
            let root = self
                .root
                .canonicalize()
                .unwrap_or_else(|_| self.cached_canonical_root().to_path_buf());
            if !canonical_parent.starts_with(&root) {
                return Err(SandboxError::OutsideWorkspace {
                    path: to.to_string(),
                });
            }
        }
        tokio::fs::copy(&canonical_from, &full_to)
            .await
            .map_err(|e| SandboxError::Io {
                path: format!("{from} -> {to}"),
                source: e,
            })?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalize a path by resolving `.` and `..` segments **without** touching the
/// filesystem (no symlink resolution). This is the safe fallback when the
/// path may not exist yet.
fn logical_normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => { /* skip */ }
            c => result.push(c),
        }
    }
    result
}

/// Convert a `SandboxError` into a `ToolResult` for return from a tool executor.
pub fn sandbox_error_to_tool_result(
    call_id: &str,
    err: SandboxError,
) -> nca_common::tool::ToolResult {
    nca_common::tool::ToolResult {
        call_id: call_id.to_string(),
        success: false,
        output: String::new(),
        error: Some(err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_file_within_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "world").unwrap();

        let fs = RealFs::new(dir.path().to_path_buf());
        let content = fs.read_file("hello.txt").await.unwrap();
        assert_eq!(content, "world");
    }

    #[tokio::test]
    async fn read_file_outside_workspace_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "world").unwrap();

        let inner = dir.path().join("subdir");
        std::fs::create_dir_all(&inner).unwrap();
        let fs = RealFs::new(inner);

        let err = fs.read_file("../hello.txt").await.unwrap_err();
        assert!(err.to_string().contains("outside the workspace"));
    }

    #[tokio::test]
    async fn write_file_creates_parents() {
        let dir = tempfile::tempdir().unwrap();

        let fs = RealFs::new(dir.path().to_path_buf());
        fs.write_file("a/b/c.txt", "deep").await.unwrap();

        let content = std::fs::read_to_string(dir.path().join("a/b/c.txt")).unwrap();
        assert_eq!(content, "deep");
    }

    #[tokio::test]
    async fn write_file_outside_workspace_fails() {
        let dir = tempfile::tempdir().unwrap();
        let fs = RealFs::new(dir.path().to_path_buf());

        let err = fs.write_file("../escape.txt", "nope").await.unwrap_err();
        assert!(err.to_string().contains("outside the workspace"));
    }

    #[test]
    fn resolve_nonexistent_fails() {
        let dir = tempfile::tempdir().unwrap();
        let fs = RealFs::new(dir.path().to_path_buf());

        let err = fs.resolve("nope.txt").unwrap_err();
        assert!(matches!(err, SandboxError::NotFound { .. }));
    }

    #[test]
    fn validate_prefix_allows_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let fs = RealFs::new(dir.path().to_path_buf());

        let result = fs.validate_prefix("new/file.txt").unwrap();
        assert!(result.ends_with("new/file.txt"));
    }

    #[test]
    fn validate_prefix_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("subdir");
        std::fs::create_dir_all(&inner).unwrap();
        let fs = RealFs::new(inner);

        let err = fs.validate_prefix("../../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("outside the workspace"));
    }

    #[tokio::test]
    async fn rename_within_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "data").unwrap();

        let fs = RealFs::new(dir.path().to_path_buf());
        fs.rename("a.txt", "b.txt").await.unwrap();

        assert!(!dir.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "data"
        );
    }

    #[tokio::test]
    async fn read_dir_returns_sorted_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("z.rs"), "").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();

        let fs = RealFs::new(dir.path().to_path_buf());
        let entries = fs.read_dir(".").await.unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "a.rs");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[1].name, "src");
        assert!(entries[1].is_dir);
        assert_eq!(entries[2].name, "z.rs");
    }

    #[tokio::test]
    async fn delete_file_within_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("del.txt"), "").unwrap();

        let fs = RealFs::new(dir.path().to_path_buf());
        fs.remove_file("del.txt").await.unwrap();
        assert!(!dir.path().join("del.txt").exists());
    }

    #[test]
    fn logical_normalize_handles_dotdot() {
        let path = Path::new("/workspace/src/../etc/passwd");
        let normalized = logical_normalize(path);
        assert_eq!(normalized, Path::new("/workspace/etc/passwd"));
    }

    #[test]
    fn logical_normalize_strips_curdir() {
        let path = Path::new("/workspace/./src/./main.rs");
        let normalized = logical_normalize(path);
        assert_eq!(normalized, Path::new("/workspace/src/main.rs"));
    }

    #[test]
    fn logical_normalize_parent_at_root_is_ok() {
        let path = Path::new("/workspace/../etc/passwd");
        let normalized = logical_normalize(path);
        assert_eq!(normalized, Path::new("/etc/passwd"));
    }
}
