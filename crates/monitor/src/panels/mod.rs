pub mod diff;
pub mod files;
pub mod log;
pub mod review;
pub mod sessions;
pub mod stats;
pub mod terminal;
pub mod timeline;
pub mod tools;

/// Char-boundary-safe string truncation. Truncates to at most `max_chars`
/// Unicode characters, appending "..." if the string was shortened.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}
