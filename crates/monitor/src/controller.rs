//! Live attach to running sessions: connect, stream events to UI thread, reconnect with backoff.

use nca_common::event::{AgentCommand, AgentEvent};
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
    socket_path: PathBuf,
    _join_handle: Option<thread::JoinHandle<()>>,
}

impl LiveAttachController {
    /// Start attaching to the given socket. Events are sent to the returned receiver.
    /// The background task will reconnect with backoff when the connection drops.
    pub fn attach(socket_path: PathBuf) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop = stop_flag.clone();

        let socket_path_for_handle = socket_path.clone();
        let join_handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(Self::attach_loop(socket_path_for_handle, event_tx, stop));
        });

        Self {
            event_rx,
            stop_flag,
            socket_path,
            _join_handle: Some(join_handle),
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
                    while let Some(event) = rx.recv().await {
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

    /// Try to receive events without blocking. Call from the UI thread each frame.
    pub fn try_recv(&self) -> Option<AgentEvent> {
        self.event_rx.try_recv().ok()
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

    /// Send a command to the running session. Spawns a thread for the async send.
    pub fn send_command(&self, cmd: &AgentCommand) {
        let path = self.socket_path.clone();
        let cmd = cmd.clone();
        thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok();
            if let Some(rt) = rt {
                let client = nca_runtime::ipc::IpcClient::new(path);
                let _ = rt.block_on(client.send_command(&cmd));
            }
        });
    }
}
