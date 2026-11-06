//! `nca add <git-url>`: clone, validate, approve, install.
//!
//! Nothing from the repository is built or executed here: only the manifest and
//! the prebuilt `.wasm` named by it are copied.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::PluginError;
use crate::manifest::{MANIFEST_FILE, Manifest};
use crate::registry::{Entry, Registry, plugin_home, sha256_hex};

const MAX_WASM_BYTES: u64 = 64 * 1024 * 1024;

/// Source string split into clone URL and optional pinned ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub url: String,
    pub git_ref: Option<String>,
}

/// Parse `<url>[@ref]`. `owner/repo` expands to a GitHub HTTPS URL.
///
/// `@` only starts a ref when what follows has no `:` or `/`, so
/// `git@github.com:o/r.git` and `https://user@host/o/r` stay intact.
pub fn parse_source(input: &str) -> Result<Source, PluginError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(PluginError::Install("empty source".into()));
    }
    let (base, git_ref) = match input.rfind('@') {
        Some(i) if i > 0 && !input[i + 1..].is_empty() && !input[i + 1..].contains([':', '/']) => {
            (&input[..i], Some(input[i + 1..].to_string()))
        }
        _ => (input, None),
    };
    let is_shorthand = base.matches('/').count() == 1
        && !base.contains([':', '.', '@'])
        && !base.starts_with(['/', '~']);
    let url = if is_shorthand {
        format!("https://github.com/{base}.git")
    } else {
        base.to_string()
    };
    Ok(Source { url, git_ref })
}

fn looks_like_sha(r: &str) -> bool {
    r.len() >= 7 && r.len() <= 40 && r.chars().all(|c| c.is_ascii_hexdigit())
}

fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, PluginError> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let out = cmd
        .output()
        .map_err(|e| PluginError::Install(format!("failed to run git: {e}")))?;
    if !out.status.success() {
        return Err(PluginError::Install(format!(
            "`git {}` failed ({})",
            args.join(" "),
            out.status
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Clone with the user's own git credentials (SSH agent, credential helper),
/// so private repositories work without nca ever handling a token.
fn clone(source: &Source, dest: &Path) -> Result<String, PluginError> {
    let dest_s = dest.to_string_lossy();
    match &source.git_ref {
        Some(r) if looks_like_sha(r) => {
            git(&["clone", "--quiet", "--", &source.url, &dest_s], None)?;
            git(&["checkout", "--quiet", r], Some(dest))?;
        }
        Some(r) => {
            git(
                &[
                    "clone",
                    "--quiet",
                    "--depth",
                    "1",
                    "--branch",
                    r,
                    "--",
                    &source.url,
                    &dest_s,
                ],
                None,
            )?;
        }
        None => {
            git(
                &[
                    "clone",
                    "--quiet",
                    "--depth",
                    "1",
                    "--",
                    &source.url,
                    &dest_s,
                ],
                None,
            )?;
        }
    }
    git(&["rev-parse", "HEAD"], Some(dest))
}

/// What the user is asked to approve.
#[derive(Debug, Clone)]
pub struct ApprovalRequest<'a> {
    pub manifest: &'a Manifest,
    pub source: &'a str,
    pub commit: &'a str,
    pub wasm_sha256: &'a str,
    /// Previous manifest when updating; lets the UI show what changed.
    pub previous: Option<&'a Manifest>,
}

pub type Approver<'a> = &'a dyn Fn(&ApprovalRequest<'_>) -> bool;

pub struct InstallOptions<'a> {
    /// `Some` installs project-scoped for that workspace.
    pub workspace: Option<PathBuf>,
    pub approve: Approver<'a>,
}

struct Fetched {
    _tmp: tempfile::TempDir,
    manifest: Manifest,
    manifest_text: String,
    wasm: Vec<u8>,
    commit: String,
}

fn fetch(source: &Source) -> Result<Fetched, PluginError> {
    let tmp = tempfile::tempdir().map_err(|e| PluginError::Install(e.to_string()))?;
    let repo = tmp.path().join("repo");
    let commit = clone(source, &repo)?;
    let manifest_text = std::fs::read_to_string(repo.join(MANIFEST_FILE)).map_err(|e| {
        PluginError::Install(format!("{MANIFEST_FILE} not found in repository root: {e}"))
    })?;
    let manifest = Manifest::parse(&manifest_text)?;
    let wasm_path = repo.join(&manifest.plugin.wasm);
    // Canonicalize to defeat symlinks that point outside the clone.
    let canon = wasm_path
        .canonicalize()
        .map_err(|e| PluginError::Install(format!("{}: {e}", manifest.plugin.wasm)))?;
    if !canon.starts_with(repo.canonicalize().unwrap_or(repo.clone())) {
        return Err(PluginError::Install(format!(
            "{} resolves outside the repository",
            manifest.plugin.wasm
        )));
    }
    let size = std::fs::metadata(&canon)
        .map_err(|e| PluginError::Install(e.to_string()))?
        .len();
    if size > MAX_WASM_BYTES {
        return Err(PluginError::Install(format!(
            "wasm is {size} bytes, limit is {MAX_WASM_BYTES}"
        )));
    }
    let wasm = std::fs::read(&canon).map_err(|e| PluginError::Install(e.to_string()))?;
    // Fails early if the file is not a component or needs imports nca lacks.
    crate::host::check_component(&wasm)?;
    Ok(Fetched {
        _tmp: tmp,
        manifest,
        manifest_text,
        wasm,
        commit,
    })
}

fn install_dir(name: &str, workspace: Option<&Path>) -> Result<PathBuf, PluginError> {
    Ok(match workspace {
        Some(ws) => ws.join(".nca").join("plugins").join(name),
        None => plugin_home()?.join(name),
    })
}

fn write_install(f: &Fetched, dir: &Path) -> Result<(), PluginError> {
    let io = |e: std::io::Error| PluginError::Install(format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    std::fs::create_dir_all(dir.join("data")).map_err(io)?;
    let wasm_dest = dir.join(&f.manifest.plugin.wasm);
    if let Some(parent) = wasm_dest.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    std::fs::write(&wasm_dest, &f.wasm).map_err(io)?;
    std::fs::write(dir.join(MANIFEST_FILE), &f.manifest_text).map_err(io)
}

fn entry_for(f: &Fetched, source: &Source, dir: PathBuf, workspace: Option<&Path>) -> Entry {
    Entry {
        name: f.manifest.plugin.name.clone(),
        version: f.manifest.plugin.version.clone(),
        source: source.url.clone(),
        requested_ref: source.git_ref.clone(),
        commit: f.commit.clone(),
        wasm_sha256: sha256_hex(&f.wasm),
        manifest_sha256: sha256_hex(f.manifest_text.as_bytes()),
        installed_at: chrono::Utc::now().to_rfc3339(),
        enabled: true,
        workspace: workspace.map(|w| w.to_string_lossy().into_owned()),
        dir,
    }
}

/// Install a plugin. Errors if one with the same name already exists in scope.
pub fn add(input: &str, opts: &InstallOptions<'_>) -> Result<Entry, PluginError> {
    let source = parse_source(input)?;
    let fetched = fetch(&source)?;
    let name = fetched.manifest.plugin.name.clone();
    let ws = opts.workspace.as_deref();

    let mut registry = Registry::load()?;
    if registry.find(&name, ws).is_some() {
        return Err(PluginError::Install(format!(
            "plugin `{name}` is already installed; use `nca plugin update {name}` or `nca plugin remove {name}`"
        )));
    }
    let request = ApprovalRequest {
        manifest: &fetched.manifest,
        source: &source.url,
        commit: &fetched.commit,
        wasm_sha256: &sha256_hex(&fetched.wasm),
        previous: None,
    };
    if !(opts.approve)(&request) {
        return Err(PluginError::Install("installation not approved".into()));
    }
    let dir = install_dir(&name, ws)?;
    write_install(&fetched, &dir)?;
    let entry = entry_for(&fetched, &source, dir, ws);
    registry.upsert(entry.clone());
    registry.save()?;
    Ok(entry)
}

/// Outcome of [`update`].
#[derive(Debug)]
pub enum UpdateOutcome {
    Unchanged,
    Updated(Box<Entry>),
}

/// Re-fetch an installed plugin from its recorded source. Asks for approval only
/// when hooks, capabilities or config grant something different from before.
pub fn update(
    name: &str,
    workspace: Option<&Path>,
    approve: Approver<'_>,
) -> Result<UpdateOutcome, PluginError> {
    let mut registry = Registry::load()?;
    let current = registry
        .find(name, workspace)
        .cloned()
        .ok_or_else(|| PluginError::Install(format!("plugin `{name}` is not installed")))?;
    let source = Source {
        url: current.source.clone(),
        git_ref: current.requested_ref.clone(),
    };
    let fetched = fetch(&source)?;
    if fetched.manifest.plugin.name != name {
        return Err(PluginError::Install(format!(
            "source now provides `{}` instead of `{name}`; remove and re-add it",
            fetched.manifest.plugin.name
        )));
    }
    let wasm_sha = sha256_hex(&fetched.wasm);
    if fetched.commit == current.commit && wasm_sha == current.wasm_sha256 {
        return Ok(UpdateOutcome::Unchanged);
    }
    let previous = current.load_verified().ok().map(|(m, _)| m);
    let grants_changed = previous.as_ref().is_none_or(|p| {
        p.hooks != fetched.manifest.hooks
            || p.capabilities != fetched.manifest.capabilities
            || p.config != fetched.manifest.config
    });
    if grants_changed {
        let request = ApprovalRequest {
            manifest: &fetched.manifest,
            source: &source.url,
            commit: &fetched.commit,
            wasm_sha256: &wasm_sha,
            previous: previous.as_ref(),
        };
        if !approve(&request) {
            return Err(PluginError::Install("update not approved".into()));
        }
    }
    write_install(&fetched, &current.dir)?;
    let mut entry = entry_for(&fetched, &source, current.dir.clone(), workspace);
    entry.enabled = current.enabled;
    registry.upsert(entry.clone());
    registry.save()?;
    Ok(UpdateOutcome::Updated(Box::new(entry)))
}

pub fn remove(name: &str, workspace: Option<&Path>) -> Result<Entry, PluginError> {
    let mut registry = Registry::load()?;
    let entry = registry
        .remove(name, workspace)
        .ok_or_else(|| PluginError::Install(format!("plugin `{name}` is not installed")))?;
    registry.save()?;
    if entry.dir.exists() {
        std::fs::remove_dir_all(&entry.dir)
            .map_err(|e| PluginError::Install(format!("{}: {e}", entry.dir.display())))?;
    }
    Ok(entry)
}

pub fn set_enabled(name: &str, workspace: Option<&Path>, enabled: bool) -> Result<(), PluginError> {
    let mut registry = Registry::load()?;
    let entry = registry
        .find_mut(name, workspace)
        .ok_or_else(|| PluginError::Install(format!("plugin `{name}` is not installed")))?;
    entry.enabled = enabled;
    registry.save()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sources() {
        let s = parse_source("https://github.com/acme/p@v1.2.0").unwrap();
        assert_eq!(s.url, "https://github.com/acme/p");
        assert_eq!(s.git_ref.as_deref(), Some("v1.2.0"));

        let s = parse_source("git@github.com:acme/private.git").unwrap();
        assert_eq!(s.url, "git@github.com:acme/private.git");
        assert_eq!(s.git_ref, None);

        let s = parse_source("git@github.com:acme/private.git@main").unwrap();
        assert_eq!(s.url, "git@github.com:acme/private.git");
        assert_eq!(s.git_ref.as_deref(), Some("main"));

        let s = parse_source("https://user@host.example/o/r.git").unwrap();
        assert_eq!(s.url, "https://user@host.example/o/r.git");
        assert_eq!(s.git_ref, None);

        let s = parse_source("acme/plugin").unwrap();
        assert_eq!(s.url, "https://github.com/acme/plugin.git");

        assert!(parse_source("  ").is_err());
    }
}
