//! Isolated git worktree management for agent runs.
//! Each agent run can operate in its own worktree/branch so that parallel
//! runs don't interfere with each other or the user's working tree.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Metadata about a worktree created for an agent run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorktreeInfo {
    pub worktree_path: PathBuf,
    pub branch_name: String,
    pub base_branch: String,
    pub session_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Manages git worktrees for isolated agent runs.
pub struct WorktreeManager {
    repo_root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git command failed: {0}")]
    GitFailed(String),
    #[error("not a git repository: {0}")]
    NotGitRepo(String),
    #[error("IO error: {0}")]
    Io(String),
}

impl WorktreeManager {
    pub fn new(repo_root: impl AsRef<Path>) -> Self {
        Self {
            repo_root: repo_root.as_ref().to_path_buf(),
        }
    }

    /// Check if the workspace is a git repository.
    pub fn is_git_repo(&self) -> bool {
        Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&self.repo_root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get the current branch name.
    pub fn current_branch(&self) -> Result<String, WorktreeError> {
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| WorktreeError::Io(e.to_string()))?;

        if !output.status.success() {
            return Err(WorktreeError::GitFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Create an isolated worktree for an agent run.
    /// The worktree is created at `<repo>/.nca/worktrees/<session_id>` with
    /// a new branch `nca/<session_id>` based on the current HEAD.
    pub fn create_worktree(&self, session_id: &str) -> Result<WorktreeInfo, WorktreeError> {
        if !self.is_git_repo() {
            return Err(WorktreeError::NotGitRepo(
                self.repo_root.display().to_string(),
            ));
        }

        let base_branch = self.current_branch()?;
        let branch_name = format!("nca/{session_id}");
        let worktree_path = self.repo_root.join(".nca").join("worktrees").join(session_id);

        if worktree_path.exists() {
            std::fs::remove_dir_all(&worktree_path)
                .map_err(|e| WorktreeError::Io(e.to_string()))?;
        }

        if let Some(parent) = worktree_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WorktreeError::Io(e.to_string()))?;
        }

        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &branch_name,
                &worktree_path.display().to_string(),
                "HEAD",
            ])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| WorktreeError::Io(e.to_string()))?;

        if !output.status.success() {
            return Err(WorktreeError::GitFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(WorktreeInfo {
            worktree_path,
            branch_name,
            base_branch,
            session_id: session_id.to_string(),
            created_at: chrono::Utc::now(),
        })
    }

    /// Remove a worktree and optionally its branch.
    pub fn remove_worktree(
        &self,
        session_id: &str,
        delete_branch: bool,
    ) -> Result<(), WorktreeError> {
        let worktree_path = self.repo_root.join(".nca").join("worktrees").join(session_id);

        if worktree_path.exists() {
            let output = Command::new("git")
                .args([
                    "worktree",
                    "remove",
                    "--force",
                    &worktree_path.display().to_string(),
                ])
                .current_dir(&self.repo_root)
                .output()
                .map_err(|e| WorktreeError::Io(e.to_string()))?;

            if !output.status.success() {
                let _ = std::fs::remove_dir_all(&worktree_path);
            }
        }

        if delete_branch {
            let branch_name = format!("nca/{session_id}");
            let _ = Command::new("git")
                .args(["branch", "-D", &branch_name])
                .current_dir(&self.repo_root)
                .output();
        }

        Ok(())
    }

    /// List all nca worktrees.
    pub fn list_worktrees(&self) -> Vec<WorktreeInfo> {
        let worktrees_dir = self.repo_root.join(".nca").join("worktrees");
        if !worktrees_dir.exists() {
            return Vec::new();
        }

        let mut result = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&worktrees_dir) {
            for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            let session_id = name.to_string();
                            let branch_name = format!("nca/{session_id}");
                            let base_branch = self.current_branch().unwrap_or_else(|_| "main".into());
                            result.push(WorktreeInfo {
                                worktree_path: path,
                                branch_name,
                                base_branch,
                                session_id,
                                created_at: chrono::Utc::now(),
                            });
                        }
                    }
                }
        }
        result
    }

    /// Get changed files in a worktree relative to its base branch.
    pub fn changed_files(&self, worktree_path: &Path, base_branch: &str) -> Vec<ChangedFile> {
        let output = Command::new("git")
            .args(["diff", "--name-status", base_branch])
            .current_dir(worktree_path)
            .output();

        let output = match output {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(2, '\t');
                let status = parts.next()?;
                let path = parts.next()?;
                let change_type = match status.chars().next()? {
                    'A' => ChangeType::Added,
                    'M' => ChangeType::Modified,
                    'D' => ChangeType::Deleted,
                    'R' => ChangeType::Renamed,
                    _ => ChangeType::Modified,
                };
                Some(ChangedFile {
                    path: PathBuf::from(path),
                    change_type,
                })
            })
            .collect()
    }

    /// Get the diff for a specific file in a worktree.
    pub fn file_diff(
        &self,
        worktree_path: &Path,
        base_branch: &str,
        file_path: &Path,
    ) -> String {
        let output = Command::new("git")
            .args([
                "diff",
                base_branch,
                "--",
                &file_path.display().to_string(),
            ])
            .current_dir(worktree_path)
            .output();

        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => String::new(),
        }
    }

    /// Get ahead/behind counts relative to base branch.
    pub fn ahead_behind(
        &self,
        worktree_path: &Path,
        base_branch: &str,
    ) -> (usize, usize) {
        let output = Command::new("git")
            .args(["rev-list", "--left-right", "--count", &format!("{base_branch}...HEAD")])
            .current_dir(worktree_path)
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                let mut parts = text.trim().split('\t');
                let behind = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let ahead = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                (ahead, behind)
            }
            _ => (0, 0),
        }
    }

    /// Merge the worktree branch back into the base branch.
    pub fn merge_into_base(
        &self,
        session_id: &str,
        base_branch: &str,
    ) -> Result<(), WorktreeError> {
        let branch_name = format!("nca/{session_id}");

        let output = Command::new("git")
            .args(["checkout", base_branch])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| WorktreeError::Io(e.to_string()))?;

        if !output.status.success() {
            return Err(WorktreeError::GitFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output = Command::new("git")
            .args(["merge", "--no-ff", &branch_name, "-m", &format!("Merge nca session {session_id}")])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| WorktreeError::Io(e.to_string()))?;

        if !output.status.success() {
            return Err(WorktreeError::GitFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Prune stale worktrees that no longer have active sessions.
    pub fn prune_stale(&self) {
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&self.repo_root)
            .output();
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub change_type: ChangeType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeType::Added => write!(f, "A"),
            ChangeType::Modified => write!(f, "M"),
            ChangeType::Deleted => write!(f, "D"),
            ChangeType::Renamed => write!(f, "R"),
        }
    }
}
