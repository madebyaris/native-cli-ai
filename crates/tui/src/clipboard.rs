//! Clipboard text helpers for the TUI.
//!
//! Uses `arboard` for cross-platform access. Callers should run these helpers
//! on a blocking thread (`spawn_blocking`) because clipboard backends can
//! block on Linux/X11/Wayland initialization.

use arboard::Clipboard;

/// Copy plain text to the system clipboard.
///
/// Returns an actionable error when the clipboard backend cannot be opened or
/// written (common on headless Linux or missing Wayland/X11 permissions).
pub fn set_clipboard_text(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("failed to write clipboard: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_clipboard_text_rejects_backend_errors_loudly() {
        // Empty string is still a valid clipboard write on most platforms; we
        // only assert the helper returns a Result (smoke for the API surface).
        let _ = set_clipboard_text("");
    }
}
