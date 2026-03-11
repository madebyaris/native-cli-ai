use std::path::PathBuf;

/// Discovers and connects to running nca sessions via Unix sockets.
pub struct MonitorIpcClient {
    runtime_dir: PathBuf,
}

impl MonitorIpcClient {
    pub fn new() -> Self {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("nca");
        Self { runtime_dir }
    }

    /// List session IDs that have active sockets.
    pub fn discover_sessions(&self) -> Vec<String> {
        // TODO: scan runtime_dir for .sock files
        Vec::new()
    }
}
