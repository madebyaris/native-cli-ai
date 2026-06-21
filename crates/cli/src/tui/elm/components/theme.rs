//! Shared theme colors for all TUI components.

use ratatui::style::Color;

/// Centralized theme color constants used across all TUI components.
/// Use as `Theme::BG`, `Theme::TEXT`, etc.
pub(crate) struct Theme;

impl Theme {
    pub(crate) const BG: Color = Color::Rgb(22, 22, 28);
    pub(crate) const SURFACE: Color = Color::Rgb(32, 32, 42);
    pub(crate) const BORDER: Color = Color::Rgb(55, 55, 70);
    pub(crate) const MENTION_BG: Color = Color::Rgb(48, 62, 94);

    pub(crate) const USER: Color = Color::Rgb(56, 189, 248);
    pub(crate) const ASSISTANT: Color = Color::Rgb(167, 139, 250);
    pub(crate) const TOOL: Color = Color::Rgb(94, 234, 212);
    pub(crate) const MUTED: Color = Color::Rgb(120, 120, 140);
    pub(crate) const TEXT: Color = Color::Rgb(230, 230, 240);
    pub(crate) const SUCCESS: Color = Color::Rgb(74, 222, 128);
    pub(crate) const ERROR: Color = Color::Rgb(248, 113, 113);
    pub(crate) const WARN: Color = Color::Rgb(251, 191, 36);
}

/// Module-level re-exports so existing `theme::BG` syntax continues to work.
pub(crate) mod colors {
    use ratatui::style::Color;

    pub const BG: Color = Color::Rgb(22, 22, 28);
    pub const SURFACE: Color = Color::Rgb(32, 32, 42);
    pub const BORDER: Color = Color::Rgb(55, 55, 70);
    pub const MENTION_BG: Color = Color::Rgb(48, 62, 94);

    pub const USER: Color = Color::Rgb(56, 189, 248);
    pub const ASSISTANT: Color = Color::Rgb(167, 139, 250);
    pub const TOOL: Color = Color::Rgb(94, 234, 212);
    pub const MUTED: Color = Color::Rgb(120, 120, 140);
    pub const TEXT: Color = Color::Rgb(230, 230, 240);
    pub const SUCCESS: Color = Color::Rgb(74, 222, 128);
    pub const ERROR: Color = Color::Rgb(248, 113, 113);
    pub const WARN: Color = Color::Rgb(251, 191, 36);
}
