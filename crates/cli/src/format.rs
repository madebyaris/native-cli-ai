//! Small display-formatting helpers shared across the TUI and REPL renderers.

/// Format a duration given in milliseconds into a compact human-readable string.
///
/// Mirrors OpenCode's `Locale.duration`:
/// `123ms`, `1.2s`, `1m 2s`, `1h 2m`, `1d 2h`.
pub(crate) fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    if ms < 60_000 {
        return format!("{:.1}s", ms as f64 / 1000.0);
    }
    if ms < 3_600_000 {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        return format!("{mins}m {secs}s");
    }
    if ms < 86_400_000 {
        let hours = ms / 3_600_000;
        let mins = (ms % 3_600_000) / 60_000;
        return format!("{hours}h {mins}m");
    }
    let days = ms / 86_400_000;
    let hours = (ms % 86_400_000) / 3_600_000;
    format!("{days}d {hours}h")
}
