use nca_common::event::{AgentCommand, EventEnvelope};
use std::path::Path;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

#[cfg(windows)]
type IpcListener = tokio::net::TcpListener;
#[cfg(unix)]
type IpcListener = tokio::net::UnixListener;
#[cfg(windows)]
type IpcStream = tokio::net::TcpStream;
#[cfg(unix)]
type IpcStream = tokio::net::UnixStream;

/// IPC server that broadcasts AgentEvents and receives AgentCommands
/// over a Unix domain socket or Windows loopback TCP.
pub struct IpcServer {
    socket_path: PathBuf,
}

impl IpcServer {
    pub fn new(session_id: &str) -> Self {
        #[cfg(unix)]
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        #[cfg(unix)]
        let socket_path = runtime_dir.join("nca").join(format!("{session_id}.sock"));
        #[cfg(windows)]
        let socket_path = windows_tcp_endpoint(session_id);
        Self { socket_path }
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket_path.clone()
    }

    /// Start listening for client connections.
    pub async fn start(&self) -> Result<IpcHandle, IpcError> {
        #[cfg(unix)]
        if let Some(parent) = self.socket_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
        }
        #[cfg(unix)]
        if self.socket_path.exists() {
            let _ = tokio::fs::remove_file(&self.socket_path).await;
        }

        let listener = bind_listener(&self.socket_path).await?;
        let (event_tx, _) = broadcast::channel::<String>(256);
        let accept_event_tx = event_tx.clone();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let socket_path = self.socket_path.clone();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let event_rx = accept_event_tx.subscribe();
                let command_tx = command_tx.clone();
                tokio::spawn(handle_connection(stream, event_rx, command_tx));
            }
            cleanup_endpoint(&socket_path).await;
        });

        Ok(IpcHandle {
            socket_path: self.socket_path.clone(),
            event_tx,
            command_rx,
        })
    }
}

pub struct IpcHandle {
    socket_path: PathBuf,
    event_tx: broadcast::Sender<String>,
    command_rx: mpsc::UnboundedReceiver<AgentCommand>,
}

impl IpcHandle {
    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    pub async fn broadcast(&self, event: &EventEnvelope) -> Result<(), IpcError> {
        let line = serde_json::to_string(event)
            .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
        let _ = self.event_tx.send(line);
        Ok(())
    }

    pub async fn recv_command(&mut self) -> Option<AgentCommand> {
        self.command_rx.recv().await
    }

    /// Split into parts for separate tasks: event broadcast and command receiver.
    pub fn into_parts(
        self,
    ) -> (
        broadcast::Sender<String>,
        mpsc::UnboundedReceiver<AgentCommand>,
    ) {
        (self.event_tx, self.command_rx)
    }
}

/// IPC client for connecting to a running session socket (events, approvals, shutdown).
pub struct IpcClient {
    socket_path: PathBuf,
}

impl IpcClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    pub async fn connect(&self) -> Result<mpsc::Receiver<EventEnvelope>, IpcError> {
        let stream = connect_stream(&self.socket_path).await?;
        let (tx, rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let reader = BufReader::new(stream);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(event) = serde_json::from_str::<EventEnvelope>(&line)
                    && tx.send(event).await.is_err()
                {
                    break;
                }
            }
        });
        Ok(rx)
    }

    pub async fn send_command(&self, cmd: &AgentCommand) -> Result<(), IpcError> {
        let mut stream = connect_stream(&self.socket_path).await?;
        let line = serde_json::to_string(cmd)
            .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
        stream
            .write_all(line.as_bytes())
            .await
            .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
        stream
            .write_all(b"\n")
            .await
            .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
}

#[cfg(unix)]
async fn bind_listener(endpoint: &Path) -> Result<IpcListener, IpcError> {
    IpcListener::bind(endpoint).map_err(|err| IpcError::ConnectionFailed(err.to_string()))
}

#[cfg(windows)]
async fn bind_listener(endpoint: &Path) -> Result<IpcListener, IpcError> {
    IpcListener::bind(endpoint.to_string_lossy().as_ref())
        .await
        .map_err(|err| IpcError::ConnectionFailed(err.to_string()))
}

#[cfg(unix)]
async fn connect_stream(endpoint: &Path) -> Result<IpcStream, IpcError> {
    IpcStream::connect(endpoint)
        .await
        .map_err(|err| IpcError::ConnectionFailed(err.to_string()))
}

#[cfg(windows)]
async fn connect_stream(endpoint: &Path) -> Result<IpcStream, IpcError> {
    IpcStream::connect(endpoint.to_string_lossy().as_ref())
        .await
        .map_err(|err| IpcError::ConnectionFailed(err.to_string()))
}

#[cfg(unix)]
async fn cleanup_endpoint(endpoint: &Path) {
    let _ = tokio::fs::remove_file(endpoint).await;
}

#[cfg(windows)]
async fn cleanup_endpoint(_endpoint: &Path) {}

#[cfg(any(windows, test))]
fn windows_tcp_endpoint(session_id: &str) -> PathBuf {
    // FNV-1a gives every process the same endpoint for a session without
    // requiring a filesystem rendezvous file. The broad dynamic-port range
    // keeps collisions between concurrently active sessions unlikely.
    let hash = session_id.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    let port = 20_000 + (hash % 40_000) as u16;
    PathBuf::from(format!("127.0.0.1:{port}"))
}

async fn handle_connection(
    stream: IpcStream,
    mut event_rx: broadcast::Receiver<String>,
    command_tx: mpsc::UnboundedSender<AgentCommand>,
) {
    let (reader, mut writer) = tokio::io::split(stream);
    let read_task = tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(command) = serde_json::from_str::<AgentCommand>(&line) {
                let _ = command_tx.send(command);
            }
        }
    });

    let write_task = tokio::spawn(async move {
        while let Ok(line) = event_rx.recv().await {
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if writer.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    let _ = tokio::join!(read_task, write_task);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_endpoint_is_stable_loopback_tcp() {
        let first = windows_tcp_endpoint("session-123");
        let second = windows_tcp_endpoint("session-123");
        assert_eq!(first, second);
        assert!(first.to_string_lossy().starts_with("127.0.0.1:"));
    }
}
