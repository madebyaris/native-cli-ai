//! Integration test that shell commands run inside a real PTY (not a pipe).

#![allow(clippy::pedantic, dead_code, unused_imports, unused_mut)]

use nca_runtime::pty::PtyManager;

#[tokio::test]
async fn pty_real_pty_reports_columns() {
    let pty = PtyManager::new(std::env::temp_dir());
    let output = pty
        .exec("tput cols", 10)
        .await
        .expect("pty exec should succeed in a real terminal session");

    let cols = output.stdout.trim().parse::<u32>().unwrap_or_else(|err| {
        panic!(
            "tput cols should print a number when stdout is a TTY (got {:?}): {err}",
            output.stdout
        );
    });
    // PtyManager opens the PTY at 80 columns.
    assert_eq!(cols, 80, "PTY column count should match openpty size");
    assert_eq!(output.exit_code, 0);
}
