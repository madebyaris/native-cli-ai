use std::path::{Path, PathBuf};

/// Wraps process execution with workspace confinement.
pub struct SandboxedProcess {
    workspace_root: PathBuf,
}

impl SandboxedProcess {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    /// Validate that a path is inside the workspace.
    pub fn validate_path(&self, path: &Path) -> Result<PathBuf, SandboxError> {
        let canonical = path
            .canonicalize()
            .map_err(|e| SandboxError::PathResolution(e.to_string()))?;

        if canonical.starts_with(&self.workspace_root) {
            Ok(canonical)
        } else {
            Err(SandboxError::OutsideWorkspace(
                canonical.display().to_string(),
            ))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("Path is outside the workspace: {0}")]
    OutsideWorkspace(String),
    #[error("Path resolution failed: {0}")]
    PathResolution(String),
}
