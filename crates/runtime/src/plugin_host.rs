//! Out-of-process plugin host — discovers, spawns, and communicates with
//! plugin processes via JSON-over-Unix-socket RPC.
//!
//! Discovery: nca scans `$XDG_CONFIG_DIR/nca/plugins/` for executables,
//! runs each with `--describe` to get metadata, then manages their lifecycle.

use nca_common::config::NcaConfig;
use nca_core::plugin::NcaPlugin;
use nca_core::plugin_protocol::{PluginRequest, PluginResponse};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// A discovered plugin ready to be spawned.
#[derive(Debug, Clone)]
pub struct PluginDescriptor {
    pub name: String,
    pub binary: PathBuf,
    pub args: Vec<String>,
}

/// Manages plugin process lifecycle.
pub struct PluginHost {
    plugins: Vec<RemotePluginInstance>,
    next_id: Arc<AtomicU64>,
}

struct RemotePluginInstance {
    name: String,
    process: Option<Child>,
    socket_path: PathBuf,
    client: Option<Arc<Mutex<PluginClient>>>,
}

/// JSON-RPC client over a Unix socket.
pub(crate) struct PluginClient {
    stream: tokio::net::UnixStream,
    read_buf: Vec<u8>,
}

// ─── Discovery ───────────────────────────────────────────────────────────────

/// Scan `$XDG_CONFIG_DIR/nca/plugins/` for executable plugin binaries.
///
/// Each binary is probed with `--describe` flag.  Only those that return
/// valid JSON metadata are included.
pub fn discover_plugins() -> Vec<PluginDescriptor> {
    let plugins_dir = plugins_dir();
    if !plugins_dir.is_dir() {
        tracing::debug!("no plugins dir at {}", plugins_dir.display());
        return Vec::new();
    }

    let mut descriptors = Vec::new();

    let entries = match std::fs::read_dir(&plugins_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("failed to read plugins dir: {e}");
            return Vec::new();
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Only regular files that are executable
        if !path.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&path)
                && meta.permissions().mode() & 0o111 == 0
            {
                tracing::debug!("skipping non-executable: {}", path.display());
                continue;
            }
        }

        // Probe with --describe
        match probe_plugin(&path) {
            Some(desc) => {
                tracing::info!("discovered plugin: {} ({})", desc.name, path.display());
                descriptors.push(desc);
            }
            None => {
                tracing::debug!("ignoring {} — no valid --describe response", path.display());
            }
        }
    }

    descriptors
}

/// Run a plugin binary with `--describe` and parse the JSON output.
fn probe_plugin(path: &Path) -> Option<PluginDescriptor> {
    let output = std::process::Command::new(path)
        .arg("--describe")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = std::str::from_utf8(&output.stdout).ok()?.trim();
    let info: serde_json::Value = serde_json::from_str(stdout).ok()?;

    let name = info.get("name")?.as_str()?.to_string();
    let _version = info.get("version").and_then(|v| v.as_str()).unwrap_or("0");

    Some(PluginDescriptor {
        name,
        binary: path.to_path_buf(),
        args: vec![],
    })
}

/// Resolve the plugins directory path.
fn plugins_dir() -> PathBuf {
    nca_common::config::xdg_config_dir()
        .map(|d| d.join("nca/plugins"))
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config/nca/plugins"))
                .unwrap_or_else(|| PathBuf::from(".nca/plugins"))
        })
}

// ─── Lifecycle ───────────────────────────────────────────────────────────────

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginHost {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Spawn all given plugin descriptors.
    pub async fn start_all(&mut self, descriptors: &[PluginDescriptor]) -> Vec<String> {
        let mut errors = Vec::new();
        for desc in descriptors {
            match self.spawn_one(desc).await {
                Ok(()) => tracing::info!("plugin started: {}", desc.name),
                Err(e) => {
                    tracing::error!("failed to start plugin {}: {}", desc.name, e);
                    errors.push(format!("{}: {}", desc.name, e));
                }
            }
        }
        errors
    }

    async fn spawn_one(&mut self, desc: &PluginDescriptor) -> Result<(), String> {
        let tmp_dir = std::env::temp_dir().join(format!("nca-plugin-{}", desc.name));
        std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create socket dir: {e}"))?;
        let socket_path = tmp_dir.join(format!("{}.sock", desc.name));
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path)
            .map_err(|e| format!("bind socket {}: {e}", socket_path.display()))?;

        let mut cmd = Command::new(&desc.binary);
        cmd.args(&desc.args);
        cmd.arg("--socket");
        cmd.arg(socket_path.to_str().unwrap());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::inherit());
        cmd.kill_on_drop(true);

        let process = cmd
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", desc.name))?;

        let accept = listener.accept();
        let timeout = tokio::time::Duration::from_secs(10);
        let (stream, _) = tokio::time::timeout(timeout, accept)
            .await
            .map_err(|_| format!("plugin {} did not connect within 10s", desc.name))?
            .map_err(|e| format!("accept: {e}"))?;

        let client = Arc::new(Mutex::new(PluginClient::new(stream)));

        self.plugins.push(RemotePluginInstance {
            name: desc.name.clone(),
            process: Some(process),
            socket_path,
            client: Some(client),
        });

        Ok(())
    }

    /// Build a [`PluginRegistry`] with remote-backed [`NcaPlugin`] implementations.
    pub fn registry(&self) -> nca_core::plugin::PluginRegistry {
        let mut reg = nca_core::plugin::PluginRegistry::new();
        for instance in &self.plugins {
            if let Some(client) = &instance.client {
                let remote =
                    RemotePlugin::new(&instance.name, client.clone(), self.next_id.clone());
                reg.register(Box::new(remote));
            }
        }
        reg
    }

    /// Gracefully shut down all plugin processes.
    pub async fn shutdown(&mut self) {
        for instance in &mut self.plugins {
            if let Some(client) = &instance.client {
                let mut guard = client.lock().await;
                let req = PluginRequest::new_shutdown(&instance.name);
                let _ = guard.send(&req).await;
            }
            if let Some(mut process) = instance.process.take() {
                let _ = process.kill().await;
                let _ = process.wait().await;
            }
            let _ = std::fs::remove_file(&instance.socket_path);
        }
        self.plugins.clear();
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        for instance in &mut self.plugins {
            if let Some(mut p) = instance.process.take() {
                let _ = p.start_kill();
            }
            let _ = std::fs::remove_file(&instance.socket_path);
        }
    }
}

// ─── Remote NcaPlugin ────────────────────────────────────────────────────────

pub(crate) struct RemotePlugin {
    name: String,
    client: Arc<Mutex<PluginClient>>,
    next_id: Arc<AtomicU64>,
}

impl RemotePlugin {
    pub fn new(
        name: impl Into<String>,
        client: Arc<Mutex<PluginClient>>,
        next_id: Arc<AtomicU64>,
    ) -> Self {
        Self {
            name: name.into(),
            client,
            next_id,
        }
    }
}

impl NcaPlugin for RemotePlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_system_prompt(&self, _config: &NcaConfig, workspace_root: &Path) -> Option<String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let req = PluginRequest::new_system_prompt(&id, workspace_root.to_str().unwrap_or("."));

        let handle = tokio::runtime::Handle::try_current().ok()?;
        handle.block_on(async {
            let mut guard = self.client.lock().await;
            guard.send(&req).await.ok()?;
            guard.recv().await.ok()?
        })
    }
}

// ─── I/O ─────────────────────────────────────────────────────────────────────

impl PluginClient {
    fn new(stream: tokio::net::UnixStream) -> Self {
        Self {
            stream,
            read_buf: Vec::with_capacity(4096),
        }
    }

    async fn send(&mut self, req: &PluginRequest) -> Result<(), String> {
        let json = serde_json::to_string(req).map_err(|e| format!("serialize: {e}"))?;
        let mut payload = json.into_bytes();
        payload.push(b'\n');
        self.stream
            .write_all(&payload)
            .await
            .map_err(|e| format!("write: {e}"))?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<String>, String> {
        self.read_buf.clear();
        loop {
            self.stream
                .readable()
                .await
                .map_err(|e| format!("readable: {e}"))?;
            let mut byte = [0u8; 1];
            match self.stream.try_read(&mut byte) {
                Ok(0) => return Ok(None),
                Ok(1) if byte[0] == b'\n' => break,
                Ok(1) => self.read_buf.push(byte[0]),
                Ok(_) => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(format!("read: {e}")),
            }
        }

        let line = std::str::from_utf8(&self.read_buf)
            .map_err(|e| format!("utf8: {e}"))?
            .trim();

        if line.is_empty() {
            return Ok(None);
        }

        let resp: PluginResponse = serde_json::from_str(line).map_err(|e| format!("parse: {e}"))?;

        if let Some(error) = resp.error {
            return Err(error);
        }

        match resp.result {
            Some(val) => Ok(val
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_plugin_rejects_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bad-plugin");
        std::fs::write(&bin, "#!/bin/sh\necho not-json\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();

        let desc = probe_plugin(&bin);
        assert!(desc.is_none());
    }

    #[test]
    fn plugins_dir_defaults_to_xdg() {
        let dir = plugins_dir();
        // Should contain "nca/plugins"
        assert!(dir.to_string_lossy().contains("nca"));
        assert!(dir.to_string_lossy().contains("plugins"));
    }
}
