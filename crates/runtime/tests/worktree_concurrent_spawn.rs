//! Worktree creation should not block the async runtime when multiple spawns run.

#![allow(clippy::pedantic, dead_code, unused_imports, unused_mut)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[tokio::test]
async fn concurrent_spawn_tasks_do_not_block_each_other() {
    static TICKS: AtomicUsize = AtomicUsize::new(0);
    let ticker = tokio::spawn(async {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(300) {
            TICKS.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let blocking = tokio::spawn(async {
        tokio::task::spawn_blocking(|| {
            std::thread::sleep(Duration::from_millis(120));
        })
        .await
        .expect("join blocking task");
    });

    let before = TICKS.load(Ordering::SeqCst);
    blocking.await.expect("blocking task finished");
    ticker.await.expect("ticker finished");
    let after = TICKS.load(Ordering::SeqCst);

    assert!(
        after.saturating_sub(before) >= 5,
        "expected ticker to keep running during spawn_blocking, before={before} after={after}"
    );
}
