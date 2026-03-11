use nca_common::event::{AgentCommand, AgentEvent};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

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

    pub fn socket_path(&self) -> PathBuf {
        self.socket_path.clone()
    }

    /// Start listening for client connections.
    pub async fn start(&self) -> Result<IpcHandle, IpcError> {
        if let Some(parent) = self.socket_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
        }
        if self.socket_path.exists() {
            let _ = tokio::fs::remove_file(&self.socket_path).await;
        }

        let listener = UnixListener::bind(&self.socket_path)
            .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
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
            let _ = tokio::fs::remove_file(socket_path).await;
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

    pub async fn broadcast(&self, event: &AgentEvent) -> Result<(), IpcError> {
        let line = serde_json::to_string(event)
            .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
        let _ = self.event_tx.send(line);
        Ok(())
    }

    pub async fn recv_command(&mut self) -> Option<AgentCommand> {
        self.command_rx.recv().await
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

    pub async fn connect(&self) -> Result<mpsc::Receiver<AgentEvent>, IpcError> {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
        let (tx, rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let reader = BufReader::new(stream);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(event) = serde_json::from_str::<AgentEvent>(&line) {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
        });
        Ok(rx)
    }

    pub async fn send_command(&self, cmd: &AgentCommand) -> Result<(), IpcError> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|err| IpcError::ConnectionFailed(err.to_string()))?;
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

async fn handle_connection(
    stream: UnixStream,
    mut event_rx: broadcast::Receiver<String>,
    command_tx: mpsc::UnboundedSender<AgentCommand>,
) {
    let (reader, mut writer) = stream.into_split();
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
