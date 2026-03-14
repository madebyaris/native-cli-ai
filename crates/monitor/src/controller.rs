//! Live attach to running sessions: connect, stream events to UI thread, reconnect with backoff.

use nca_common::event::{AgentCommand, AgentEvent, EventEnvelope};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Manages live attach to a session socket. Streams events to the UI thread via a channel.
pub struct LiveAttachController {
    event_rx: mpsc::Receiver<AgentEvent>,
    stop_flag: Arc<AtomicBool>,
    command_tx: mpsc::Sender<AgentCommand>,
    _join_handle: Option<thread::JoinHandle<()>>,
    _command_join_handle: Option<thread::JoinHandle<()>>,
}

impl LiveAttachController {
    /// Start attaching to the given socket. Events are sent to the returned receiver.
    /// The background task will reconnect with backoff when the connection drops.
    pub fn attach(socket_path: PathBuf) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel::<AgentCommand>();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop = stop_flag.clone();
        let command_stop = stop_flag.clone();

        let socket_path_for_handle = socket_path.clone();
        let join_handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(Self::attach_loop(socket_path_for_handle, event_tx, stop));
        });

        let socket_path_for_commands = socket_path.clone();
        let command_join_handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            while !command_stop.load(Ordering::SeqCst) {
                let cmd = match command_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(cmd) => cmd,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let client = nca_runtime::ipc::IpcClient::new(socket_path_for_commands.clone());
                let _ = rt.block_on(client.send_command(&cmd));
            }
        });

        Self {
            event_rx,
            stop_flag,
            command_tx,
            _join_handle: Some(join_handle),
            _command_join_handle: Some(command_join_handle),
        }
    }

    async fn attach_loop(
        socket_path: PathBuf,
        event_tx: mpsc::Sender<AgentEvent>,
        stop: Arc<AtomicBool>,
    ) {
        let mut backoff_secs: u64 = 1;
        const MAX_BACKOFF: u64 = 30;

        while !stop.load(Ordering::SeqCst) {
            let client = nca_runtime::ipc::IpcClient::new(socket_path.clone());
            match client.connect().await {
                Ok(mut rx) => {
                    backoff_secs = 1;
                    while let Some(EventEnvelope { event, .. }) = rx.recv().await {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        if event_tx.send(event).is_err() {
                            break;
                        }
                    }
                }
                Err(_) => {}
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF);
        }
    }

    /// Drain all pending events.
    pub fn drain(&self) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.event_rx.try_recv() {
            out.push(e);
        }
        out
    }

    /// Stop the attach loop.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    /// Send a command to the running session through the command worker.
    pub fn send_command(&self, cmd: &AgentCommand) {
        let _ = self.command_tx.send(cmd.clone());
    }
}
