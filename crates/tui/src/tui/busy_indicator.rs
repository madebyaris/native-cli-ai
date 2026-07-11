//! Advanced animated busy indicator (Claude-style).

use nca_common::event::BusyState;
use ratatui::style::Color;
use std::time::Instant;

/// Animation frames for different busy states.
const THINKING_FRAMES: &[&str] = &["◐", "◓", "◑", "◒"];
const STREAMING_FRAMES: &[&str] = &["▌▌", "▍▍", "▎▎", "▏▏"];
const TOOL_FRAMES: &[&str] = &["⚙", "⚙", "⚙", "⚙"];
const APPROVAL_FRAMES: &[&str] = &["◆", "◆", "◆", "◆"];

/// Get the color for a given busy state.
pub fn color_for_state(state: BusyState) -> Color {
    match state {
        BusyState::Idle => Color::Rgb(74, 222, 128),     // Green
        BusyState::Thinking => Color::Rgb(180, 120, 80), // Brown
        BusyState::Streaming => Color::Rgb(255, 165, 0), // Orange
        BusyState::ToolRunning => Color::Rgb(94, 234, 212), // Cyan/Teal
        BusyState::ApprovalPending => Color::Rgb(251, 191, 36), // Amber/Yellow
        BusyState::Error => Color::Rgb(248, 113, 113),   // Red
    }
}

/// Get the animated frame for a given state and elapsed time.
pub fn frame_for_state(state: BusyState, elapsed_ms: u128) -> &'static str {
    let frames = match state {
        BusyState::Thinking => THINKING_FRAMES,
        BusyState::Streaming => STREAMING_FRAMES,
        BusyState::ToolRunning => TOOL_FRAMES,
        BusyState::ApprovalPending => APPROVAL_FRAMES,
        _ => return "●",
    };

    let frame_idx = (elapsed_ms / 120) as usize % frames.len();
    frames[frame_idx]
}

/// Build the busy indicator span with animation.
pub fn render_indicator(state: BusyState, state_since: Instant) -> String {
    let elapsed_ms = state_since.elapsed().as_millis();
    let frame = frame_for_state(state, elapsed_ms);
    let label = state.label();

    match state {
        BusyState::Idle => format!(" ○ {label} "),
        _ => format!(" {frame} {label} "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_for_idle_is_green() {
        let c = color_for_state(BusyState::Idle);
        assert_eq!(c, Color::Rgb(74, 222, 128));
    }

    #[test]
    fn color_for_thinking_is_brown() {
        let c = color_for_state(BusyState::Thinking);
        assert_eq!(c, Color::Rgb(180, 120, 80));
    }

    #[test]
    fn color_for_streaming_is_orange() {
        let c = color_for_state(BusyState::Streaming);
        assert_eq!(c, Color::Rgb(255, 165, 0));
    }

    #[test]
    fn frame_cycles_through_thinking_frames() {
        let frames: Vec<&str> = (0..500)
            .step_by(120)
            .map(|ms| frame_for_state(BusyState::Thinking, ms as u128))
            .collect();
        // Should cycle: ◐ ◓ ◑ ◒ ◐ ...
        assert_eq!(frames[0], "◐");
        assert_eq!(frames[1], "◓");
        assert_eq!(frames[2], "◑");
        assert_eq!(frames[3], "◒");
        assert_eq!(frames[4], "◐");
    }

    #[test]
    fn render_indicator_idle() {
        let ind = render_indicator(BusyState::Idle, Instant::now());
        assert!(ind.contains("idle"));
        assert!(ind.contains("○"));
    }

    #[test]
    fn render_indicator_thinking() {
        let ind = render_indicator(BusyState::Thinking, Instant::now());
        assert!(ind.contains("thinking"));
        // Should contain one of the thinking frames
        assert!(ind.contains("◐") || ind.contains("◓") || ind.contains("◑") || ind.contains("◒"));
    }
}
