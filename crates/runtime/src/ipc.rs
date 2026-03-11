use nca_common::event::{AgentCommand, AgentEvent};
use std::path::PathBuf;

/// IPC server that broadcasts AgentEvents and receives AgentCommands
/// over a Unix domain socket.
pub struct IpcServer {
    socket_path: PathBuf,
}

impl IpcServer {
    pub fn new(session_id: &str) -> Self {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        let socket_path = runtime_dir.join("nca").join(format!("{session_id}.sock"));
        Self { socket_path }
    }

    /// Start listening for client connections.
    pub async fn start(&self) -> Result<(), IpcError> {
        // TODO: bind Unix socket, accept connections, broadcast events
        let _ = &self.socket_path;
        Err(IpcError::NotImplemented)
    }

    /// Broadcast an event to all connected clients.
    pub async fn broadcast(&self, _event: &AgentEvent) -> Result<(), IpcError> {
        Err(IpcError::NotImplemented)
    }
}

/// IPC client used by the monitor app to connect to a running session.
pub struct IpcClient {
    socket_path: PathBuf,
}

impl IpcClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    pub async fn connect(&self) -> Result<(), IpcError> {
        let _ = &self.socket_path;
        Err(IpcError::NotImplemented)
    }

    pub async fn send_command(&self, _cmd: &AgentCommand) -> Result<(), IpcError> {
        Err(IpcError::NotImplemented)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("IPC not yet implemented")]
    NotImplemented,
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
}
