//! Shared TUI session state with a watch channel for cheap dirty detection.

use super::state::TuiSessionState;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::watch;

/// `Arc<Mutex<TuiSessionState>>` plus a watch sender notified when
/// [`TuiSessionState::state_version`] changes.
#[derive(Clone)]
pub struct SharedTuiState {
    inner: Arc<Mutex<TuiSessionState>>,
    version_tx: watch::Sender<u64>,
}

impl SharedTuiState {
    /// Wrap `state` and seed the watch channel with its current version.
    pub fn new(state: TuiSessionState) -> Self {
        let version = state.state_version;
        let (version_tx, _) = watch::channel(version);
        Self {
            inner: Arc::new(Mutex::new(state)),
            version_tx,
        }
    }

    /// Clone the inner `Arc` for APIs that still take `Arc<Mutex<…>>`.
    pub fn arc(&self) -> Arc<Mutex<TuiSessionState>> {
        self.inner.clone()
    }

    /// Lock the session state.
    pub fn lock(&self) -> std::sync::LockResult<MutexGuard<'_, TuiSessionState>> {
        self.inner.lock()
    }

    /// Subscribe to monotonic state-version bumps (redraw signal).
    pub fn version_rx(&self) -> watch::Receiver<u64> {
        self.version_tx.subscribe()
    }

    /// Current version without locking (may be slightly stale).
    pub fn version(&self) -> u64 {
        *self.version_tx.borrow()
    }

    /// Notify watchers after a mutation that changed `state_version`.
    pub fn publish_version(&self, version: u64) {
        let _ = self.version_tx.send(version);
    }

    /// Run `f` on the locked state; publish when `state_version` changes.
    pub fn with_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut TuiSessionState) -> R,
    {
        let mut g = self.inner.lock().expect("TUI state lock poisoned");
        let before = g.state_version;
        let out = f(&mut g);
        if g.state_version != before {
            self.publish_version(g.state_version);
        }
        out
    }
}

/// After `apply_event` or other direct mutex mutations, sync the watch channel.
pub fn publish_state_version(state: &Arc<Mutex<TuiSessionState>>, version_tx: &watch::Sender<u64>) {
    if let Ok(g) = state.lock() {
        let _ = version_tx.send(g.state_version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn watch_notified_on_version_bump() {
        let shared = SharedTuiState::new(TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        ));
        let mut rx = shared.version_rx();
        assert_eq!(*rx.borrow(), 1);

        shared.with_mut(|st| st.mark_dirty());
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), 2);
    }
}
