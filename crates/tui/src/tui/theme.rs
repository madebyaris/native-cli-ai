//! Colour tokens used across the TUI modules.
//!
//! Extracted from `tui/app.rs` in Phase 2.2.

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

/// Default `(token, color)` table; the token names are the plugin API (`cosmetic` hook).
const TOKENS: [(&str, Color); 12] = [
    ("bg", BG),
    ("surface", SURFACE),
    ("border", BORDER),
    ("mention-bg", MENTION_BG),
    ("user", USER),
    ("assistant", ASSISTANT),
    ("tool", TOOL),
    ("muted", MUTED),
    ("text", TEXT),
    ("success", SUCCESS),
    ("error", ERROR),
    ("warn", WARN),
];

fn to_hex(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#000000".into(),
    }
}

fn parse_hex(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// Colour substitutions requested by cosmetic plugins.
///
/// The built-in palette is referenced as constants all over the TUI, so
/// instead of threading a theme through every widget the finished frame is
/// remapped once: any cell colour equal to a default token becomes the plugin's.
pub struct Remap(Vec<(Color, Color)>);

impl Remap {
    pub fn load() -> Self {
        let defaults = TOKENS
            .iter()
            .map(|(name, color)| ((*name).to_string(), to_hex(*color)))
            .collect();
        Self::from_theme(nca_plugin::theme(defaults))
    }

    fn from_theme(theme: Vec<(String, String)>) -> Self {
        let mut pairs = Vec::new();
        for (token, value) in theme {
            let Some((_, default)) = TOKENS.iter().find(|(name, _)| *name == token) else {
                tracing::warn!("cosmetic plugin set unknown theme token `{token}`");
                continue;
            };
            match parse_hex(&value) {
                Some(color) if color != *default => pairs.push((*default, color)),
                Some(_) => {}
                None => tracing::warn!("cosmetic plugin: `{value}` is not #rrggbb"),
            }
        }
        Self(pairs)
    }

    pub fn apply(&self, buf: &mut ratatui::buffer::Buffer) {
        if self.0.is_empty() {
            return;
        }
        let map = |c: Color| {
            self.0
                .iter()
                .find(|(from, _)| *from == c)
                .map_or(c, |(_, to)| *to)
        };
        for cell in buf.content.iter_mut() {
            cell.fg = map(cell.fg);
            cell.bg = map(cell.bg);
            cell.underline_color = map(cell.underline_color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect, style::Style};

    #[test]
    fn remap_rewrites_only_overridden_tokens_and_ignores_bad_input() {
        let remap = Remap::from_theme(vec![
            ("assistant".into(), "#f97316".into()),
            ("user".into(), "not-a-color".into()),
            ("nonsense".into(), "#ffffff".into()),
            ("text".into(), to_hex(TEXT)), // same as default: no-op
        ]);
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.set_string(0, 0, "a", Style::default().fg(ASSISTANT).bg(BG));
        buf.set_string(1, 0, "u", Style::default().fg(USER));
        remap.apply(&mut buf);
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(0xf9, 0x73, 0x16));
        assert_eq!(buf[(0, 0)].bg, BG);
        assert_eq!(buf[(1, 0)].fg, USER);
    }
}
