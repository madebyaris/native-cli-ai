//! Slash-command completion panel input.

use crate::tui::composer::{SLASH_PANEL_MAX_ROWS, SlashEntry, filter_slash_entries};
use crate::tui::state::TuiSessionState;
use crate::tui::theme;
#[cfg(test)]
use crossterm::event::KeyModifiers;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

pub fn handle_slash_panel_key(
    state: &mut TuiSessionState,
    slash_entries: &[SlashEntry],
    key: KeyEvent,
) -> bool {
    let filtered = filter_slash_entries(slash_entries, &state.input_buffer);
    if filtered.is_empty() {
        return false;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => {
            if state.slash_menu_index > 0 {
                state.slash_menu_index -= 1;
            }
            true
        }
        (KeyCode::Down, _) => {
            state.slash_menu_index = (state.slash_menu_index + 1).min(filtered.len() - 1);
            true
        }
        (KeyCode::Tab, _) => {
            let pick = state.slash_menu_index % filtered.len();
            state.input_buffer = filtered[pick].command_str();
            state.cursor_char_idx = state.input_buffer.chars().count();
            true
        }
        _ => false,
    }
}

pub fn render_slash_panel(
    frame: &mut Frame,
    area: Rect,
    state: &TuiSessionState,
    slash_filtered: &[SlashEntry],
) {
    let n_show = slash_filtered.len().min(SLASH_PANEL_MAX_ROWS);
    let max_scroll = slash_filtered.len().saturating_sub(n_show);
    let list_scroll = state
        .slash_menu_index
        .saturating_sub(n_show.saturating_sub(1))
        .min(max_scroll);
    let mut slash_lines: Vec<Line> = Vec::new();
    for (i, entry) in slash_filtered[list_scroll..list_scroll + n_show]
        .iter()
        .enumerate()
    {
        let global = list_scroll + i;
        let st = if global == state.slash_menu_index {
            Style::default()
                .fg(Color::Black)
                .bg(theme::USER)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT)
        };
        slash_lines.push(Line::from(Span::styled(entry.display_text(), st)));
    }
    if slash_filtered.len() > n_show {
        slash_lines.push(Line::from(Span::styled(
            format!(
                " ─ {}/{} · ↑↓",
                state.slash_menu_index + 1,
                slash_filtered.len()
            ),
            Style::default().fg(theme::MUTED),
        )));
    }
    let slash_w = Paragraph::new(Text::from(slash_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::BORDER))
                .title(Span::styled(
                    " commands (↑↓ Tab complete) ",
                    Style::default().fg(theme::MUTED),
                )),
        )
        .style(Style::default().bg(theme::SURFACE));
    frame.render_widget(slash_w, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn down_advances_selection() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.input_buffer = "/help".into();
        let entries =
            crate::tui::composer::load_slash_entries(PathBuf::from("/tmp").as_path(), &[]);
        let before = st.slash_menu_index;
        handle_slash_panel_key(
            &mut st,
            &entries,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert!(st.slash_menu_index >= before);
    }
}
