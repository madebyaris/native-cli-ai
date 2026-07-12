//! Skill installation: source parsing, git clone, file copy, lock file management.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn user_home_or_temp() -> PathBuf {
    nca_common::config::user_home_dir().unwrap_or_else(std::env::temp_dir)
}

/// Parsed source for skill installation.
#[derive(Debug, Clone)]
pub enum SkillSource {
    /// GitHub repo: clone URL derived from owner/repo or full URL.
    GitHub { clone_url: String },
    /// Local directory path.
    Local { path: PathBuf },
}

/// Parse a source string into a SkillSource.
///
/// Supports:
/// - `owner/repo` → GitHub clone URL
/// - `https://github.com/owner/repo` → GitHub clone URL
/// - `./path` or `/absolute/path` → Local path
pub fn parse_source(source: &str) -> Result<SkillSource, String> {
    let trimmed = source.trim();

    // Local paths: starts with ./ or / or ~
    if trimmed.starts_with('/')
        || trimmed.starts_with("./")
        || trimmed.starts_with("~/")
        || trimmed.starts_with("..")
    {
        let path = if trimmed.starts_with("~/") {
            user_home_or_temp().join(trimmed.strip_prefix("~/").unwrap())
        } else {
            PathBuf::from(trimmed)
        };
        return Ok(SkillSource::Local { path });
    }

    // GitHub URL: https://github.com/owner/repo
    if trimmed.starts_with("https://github.com/") {
        let url = trimmed.trim_end_matches('/');
        let clone_url = if url.ends_with(".git") {
            url.to_string()
        } else {
            format!("{url}.git")
        };
        return Ok(SkillSource::GitHub { clone_url });
    }

    // GitHub shorthand: owner/repo (exactly one slash, no dots or colons)
    if trimmed.contains('/')
        && trimmed.matches('/').count() == 1
        && !trimmed.contains(':')
        && !trimmed.contains('.')
    {
        let clone_url = format!("https://github.com/{trimmed}.git");
        return Ok(SkillSource::GitHub { clone_url });
    }

    Err(format!(
        "Cannot parse source '{trimmed}'. Use owner/repo, a GitHub URL, or a local path."
    ))
}

/// Sanitize a skill name for use as a directory name.
/// Rejects names containing `..` (path traversal).
/// Converts to kebab-case: lowercase, non-alphanumeric → `-`, strip leading/trailing `-`.
pub fn sanitize_skill_name(name: &str) -> Result<String, String> {
    if name.contains("..") {
        return Err(format!("Skill name '{name}' contains path traversal (..)"));
    }

    let mut out = String::new();
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let sanitized = out.trim_matches('-').to_string();
    if sanitized.is_empty() {
        return Err(format!("Skill name '{name}' produces empty sanitized name"));
    }
    Ok(sanitized)
}

/// Lock file tracking installed skills.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillLock {
    pub skills: BTreeMap<String, SkillLockEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillLockEntry {
    pub source: String,
    pub commit: Option<String>,
    pub installed_at: String,
}

impl SkillLock {
    /// Read lock file from path. Returns empty lock if file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read lock file: {e}"))?;
        serde_json::from_str(&content).map_err(|e| format!("failed to parse lock file: {e}"))
    }

    /// Write lock file to path. Creates parent directories if needed.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create lock file directory: {e}"))?;
        }
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize lock file: {e}"))?;
        std::fs::write(path, content).map_err(|e| format!("failed to write lock file: {e}"))
    }

    /// Add or update a skill entry.
    pub fn upsert(&mut self, name: &str, entry: SkillLockEntry) {
        self.skills.insert(name.to_string(), entry);
    }

    /// Remove a skill entry. Returns true if it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        self.skills.remove(name).is_some()
    }
}

/// Get the lock file path for the given scope.
pub fn lock_file_path(global: bool, workspace_root: &Path) -> PathBuf {
    if global {
        user_home_or_temp().join(".nca/skills.lock")
    } else {
        workspace_root.join(".nca/skills.lock")
    }
}

/// Get the skills directory for the given scope.
pub fn skills_dir(global: bool, workspace_root: &Path) -> PathBuf {
    if global {
        user_home_or_temp().join(".nca/skills")
    } else {
        workspace_root.join(".nca/skills")
    }
}

/// Clone a git repo to a temp directory (shallow, depth=1).
/// Returns the temp directory path.
pub fn git_clone_to_temp(clone_url: &str) -> Result<tempfile::TempDir, String> {
    let tmp = tempfile::tempdir().map_err(|e| format!("failed to create temp dir: {e}"))?;
    let status = std::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            clone_url,
            &tmp.path().display().to_string(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status()
        .map_err(|e| format!("failed to run git clone: {e}"))?;
    if !status.success() {
        return Err(format!(
            "git clone failed for '{clone_url}' (exit code: {status})"
        ));
    }
    Ok(tmp)
}

/// Get the HEAD commit hash of a git repo.
pub fn git_head_commit(repo_path: &Path) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(["-C", &repo_path.display().to_string(), "rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("failed to run git rev-parse: {e}"))?;
    if !output.status.success() {
        return Err("git rev-parse HEAD failed".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Discover SKILL.md files in a directory tree.
/// Returns (skill_name, skill_directory) pairs.
pub fn discover_skills_in_dir(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut found = Vec::new();
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("failed to read dir {}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            let skill_file = path.join("SKILL.md");
            if skill_file.exists()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                found.push((name.to_string(), path));
            }
        }
    }

    Ok(found)
}

/// Copy a skill directory (including all files) to the target location.
/// Creates the target directory if it doesn't exist.
pub fn copy_skill_dir(source: &Path, target: &Path) -> Result<(), String> {
    if target.exists() {
        std::fs::remove_dir_all(target)
            .map_err(|e| format!("failed to remove existing skill dir: {e}"))?;
    }
    copy_dir_recursive(source, target)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("failed to create dir {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read dir: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {}: {e}", src_path.display()))?;
        }
    }
    Ok(())
}

/// Install skills from a parsed source.
/// Returns list of installed skill names.
pub fn install_skills(
    source: &SkillSource,
    skill_filter: &[String],
    global: bool,
    workspace_root: &Path,
) -> Result<Vec<String>, String> {
    let (skills_root, commit, _tmp) = match source {
        SkillSource::GitHub { clone_url } => {
            let tmp = git_clone_to_temp(clone_url)?;
            let commit = git_head_commit(tmp.path()).ok();
            // Look for skills in root or in a "skills" subdirectory
            let skills_path = if tmp.path().join("skills").is_dir() {
                tmp.path().join("skills")
            } else {
                tmp.path().to_path_buf()
            };
            (skills_path, commit, Some(tmp))
        }
        SkillSource::Local { path } => {
            let resolved = if path.is_relative() {
                workspace_root.join(path)
            } else {
                path.clone()
            };
            if !resolved.exists() {
                return Err(format!(
                    "Local path '{}' does not exist",
                    resolved.display()
                ));
            }
            // Look for skills in root or in a "skills" subdirectory
            let skills_path = if resolved.join("skills").is_dir() {
                resolved.join("skills")
            } else {
                resolved
            };
            (skills_path, None, None)
        }
    };

    let discovered = discover_skills_in_dir(&skills_root)?;
    if discovered.is_empty() {
        return Err(format!(
            "No SKILL.md files found in '{}'",
            skills_root.display()
        ));
    }

    let filtered: Vec<_> = if skill_filter.is_empty() {
        discovered
    } else {
        discovered
            .into_iter()
            .filter(|(name, _)| skill_filter.iter().any(|f| f == name))
            .collect()
    };

    if filtered.is_empty() {
        let available: Vec<_> = discover_skills_in_dir(&skills_root)?
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        return Err(format!(
            "No matching skills found. Available: {}",
            available.join(", ")
        ));
    }

    let target_dir = skills_dir(global, workspace_root);
    let lock_path = lock_file_path(global, workspace_root);
    let mut lock = SkillLock::load(&lock_path)?;
    let now = chrono::Utc::now().to_rfc3339();

    let source_label = match source {
        SkillSource::GitHub { clone_url } => format!(
            "github:{}",
            clone_url
                .trim_end_matches(".git")
                .trim_start_matches("https://github.com/")
        ),
        SkillSource::Local { path } => format!("local:{}", path.display()),
    };

    let mut installed = Vec::new();
    for (name, src_dir) in &filtered {
        let safe_name = sanitize_skill_name(name)?;
        let dest = target_dir.join(&safe_name);

        if dest.exists() {
            eprintln!("Warning: overwriting existing skill '{safe_name}'");
        }

        copy_skill_dir(src_dir, &dest)?;

        lock.upsert(
            &safe_name,
            SkillLockEntry {
                source: source_label.clone(),
                commit: commit.clone(),
                installed_at: now.clone(),
            },
        );

        installed.push(safe_name);
    }

    lock.save(&lock_path)?;
    Ok(installed)
}

/// Remove an installed skill by name.
pub fn remove_skill(name: &str, global: bool, workspace_root: &Path) -> Result<(), String> {
    let lock_path = lock_file_path(global, workspace_root);
    let mut lock = SkillLock::load(&lock_path)?;

    if !lock.remove(name) {
        let available: Vec<_> = lock.skills.keys().cloned().collect();
        let scope = if global { "global" } else { "local" };
        return Err(if available.is_empty() {
            format!("No {scope} skills installed")
        } else {
            format!(
                "Skill '{name}' not found in {scope} lock file. Available: {}",
                available.join(", ")
            )
        });
    }

    let dir = skills_dir(global, workspace_root).join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("failed to remove skill dir: {e}"))?;
    }

    lock.save(&lock_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_shorthand() {
        let result = parse_source("vercel-labs/agent-skills").unwrap();
        match result {
            SkillSource::GitHub { clone_url } => {
                assert_eq!(clone_url, "https://github.com/vercel-labs/agent-skills.git");
            }
            _ => panic!("expected GitHub source"),
        }
    }

    #[test]
    fn parses_github_url() {
        let result = parse_source("https://github.com/owner/repo").unwrap();
        match result {
            SkillSource::GitHub { clone_url } => {
                assert_eq!(clone_url, "https://github.com/owner/repo.git");
            }
            _ => panic!("expected GitHub source"),
        }
    }

    #[test]
    fn parses_github_url_with_git_suffix() {
        let result = parse_source("https://github.com/owner/repo.git").unwrap();
        match result {
            SkillSource::GitHub { clone_url } => {
                assert_eq!(clone_url, "https://github.com/owner/repo.git");
            }
            _ => panic!("expected GitHub source"),
        }
    }

    #[test]
    fn parses_local_relative_path() {
        let result = parse_source("./my-skills").unwrap();
        match result {
            SkillSource::Local { path } => assert_eq!(path, PathBuf::from("./my-skills")),
            _ => panic!("expected Local source"),
        }
    }

    #[test]
    fn parses_local_absolute_path() {
        let result = parse_source("/home/user/skills").unwrap();
        match result {
            SkillSource::Local { path } => assert_eq!(path, PathBuf::from("/home/user/skills")),
            _ => panic!("expected Local source"),
        }
    }

    #[test]
    fn rejects_invalid_source() {
        assert!(parse_source("just-a-word").is_err());
    }

    #[test]
    fn sanitizes_skill_name() {
        assert_eq!(sanitize_skill_name("My Skill").unwrap(), "my-skill");
        assert_eq!(
            sanitize_skill_name("test-driven-development").unwrap(),
            "test-driven-development"
        );
        assert_eq!(
            sanitize_skill_name("  Spaces & Symbols! ").unwrap(),
            "spaces-symbols"
        );
    }

    #[test]
    fn rejects_path_traversal_in_name() {
        assert!(sanitize_skill_name("../evil").is_err());
        assert!(sanitize_skill_name("foo/../bar").is_err());
    }

    #[test]
    fn rejects_empty_sanitized_name() {
        assert!(sanitize_skill_name("!!!").is_err());
    }

    #[test]
    fn lock_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".nca/skills.lock");

        let mut lock = SkillLock::default();
        lock.upsert(
            "brainstorming",
            SkillLockEntry {
                source: "github:owner/repo".into(),
                commit: Some("abc123".into()),
                installed_at: "2026-03-25T00:00:00Z".into(),
            },
        );

        lock.save(&lock_path).unwrap();
        let loaded = SkillLock::load(&lock_path).unwrap();
        assert_eq!(loaded.skills.len(), 1);
        assert_eq!(
            loaded.skills["brainstorming"].commit.as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn lock_file_load_missing_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let lock = SkillLock::load(&dir.path().join("nonexistent.lock")).unwrap();
        assert!(lock.skills.is_empty());
    }

    #[test]
    fn lock_file_remove_entry() {
        let mut lock = SkillLock::default();
        lock.upsert(
            "test",
            SkillLockEntry {
                source: "github:a/b".into(),
                commit: None,
                installed_at: "2026-03-25T00:00:00Z".into(),
            },
        );
        assert!(lock.remove("test"));
        assert!(!lock.remove("test"));
        assert!(lock.skills.is_empty());
    }

    #[test]
    fn install_skills_from_local_path() {
        let src = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();

        // Create a skill in the source
        let skill_dir = src.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: My Skill\ncommand: my-skill\n---\nBody.\n",
        )
        .unwrap();
        std::fs::write(skill_dir.join("helper.md"), "Helper.").unwrap();

        let source = SkillSource::Local {
            path: src.path().to_path_buf(),
        };
        let installed = install_skills(&source, &[], false, ws.path()).unwrap();

        assert_eq!(installed, vec!["my-skill"]);

        // Verify files were copied
        let dest = ws.path().join(".nca/skills/my-skill/SKILL.md");
        assert!(dest.exists());
        let helper = ws.path().join(".nca/skills/my-skill/helper.md");
        assert!(helper.exists());

        // Verify lock file
        let lock = SkillLock::load(&ws.path().join(".nca/skills.lock")).unwrap();
        assert!(lock.skills.contains_key("my-skill"));
        assert!(lock.skills["my-skill"].commit.is_none()); // local install
    }

    #[test]
    fn install_skills_no_skills_found_errors() {
        let src = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();

        // Empty directory — no SKILL.md
        let source = SkillSource::Local {
            path: src.path().to_path_buf(),
        };
        let result = install_skills(&source, &[], false, ws.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No SKILL.md"));
    }

    #[test]
    fn remove_skill_deletes_dir_and_lock_entry() {
        let ws = tempfile::tempdir().unwrap();
        let skill_dir = ws.path().join(".nca/skills/test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "test").unwrap();

        // Create lock entry
        let lock_path = ws.path().join(".nca/skills.lock");
        let mut lock = SkillLock::default();
        lock.upsert(
            "test-skill",
            SkillLockEntry {
                source: "github:a/b".into(),
                commit: Some("abc".into()),
                installed_at: "2026-03-25T00:00:00Z".into(),
            },
        );
        lock.save(&lock_path).unwrap();

        remove_skill("test-skill", false, ws.path()).unwrap();

        assert!(!skill_dir.exists());
        let lock = SkillLock::load(&lock_path).unwrap();
        assert!(!lock.skills.contains_key("test-skill"));
    }
}
