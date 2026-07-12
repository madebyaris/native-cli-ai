//! `@` file-mention completion panel input.

use crate::tui::composer::{
    SLASH_PANEL_MAX_ROWS, apply_at_completion, apply_selected_at_completion,
};
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

pub fn handle_at_panel_key(
    state: &mut TuiSessionState,
    workspace_files: &[String],
    key: KeyEvent,
) -> bool {
    let at_matches = crate::tui::composer::at_completion_matches(
        workspace_files,
        &state.input_buffer,
        state.cursor_char_idx,
    );
    if at_matches.is_empty() {
        return false;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => {
            if state.at_menu_index > 0 {
                state.at_menu_index -= 1;
            }
            true
        }
        (KeyCode::Down, _) => {
            state.at_menu_index = (state.at_menu_index + 1).min(at_matches.len() - 1);
            true
        }
        (KeyCode::Tab, _) | (KeyCode::Enter, _) => {
            if let Some((buf, cidx)) = apply_selected_at_completion(
                workspace_files,
                &state.input_buffer,
                state.cursor_char_idx,
                state.at_menu_index,
                matches!(key.code, KeyCode::Enter),
            ) {
                state.input_buffer = buf;
                state.cursor_char_idx = cidx;
            }
            true
        }
        _ => false,
    }
}

pub fn handle_at_panel_mouse(
    state: &mut TuiSessionState,
    area: Rect,
    at_matches: &[String],
    row: u16,
) -> bool {
    if at_matches.is_empty() {
        return false;
    }
    let inner_y = row.saturating_sub(area.y).saturating_sub(1);
    let n_show = at_matches.len().min(SLASH_PANEL_MAX_ROWS);
    let max_scroll = at_matches.len().saturating_sub(n_show);
    let pick = state.at_menu_index.min(at_matches.len().saturating_sub(1));
    let list_scroll = pick
        .saturating_sub(n_show.saturating_sub(1))
        .min(max_scroll);
    if (inner_y as usize) < n_show {
        let idx = list_scroll + inner_y as usize;
        if let Some(choice) = at_matches.get(idx) {
            let cur = state.cursor_char_idx;
            let (buf, cidx) = apply_at_completion(&state.input_buffer, cur, choice);
            state.input_buffer = buf;
            state.cursor_char_idx = cidx;
            return true;
        }
    }
    false
}

pub fn render_at_panel(
    frame: &mut Frame,
    area: Rect,
    state: &TuiSessionState,
    at_matches: &[String],
) {
    let n_show = at_matches.len().min(SLASH_PANEL_MAX_ROWS);
    let max_scroll = at_matches.len().saturating_sub(n_show);
    let pick = state.at_menu_index.min(at_matches.len().saturating_sub(1));
    let list_scroll = pick
        .saturating_sub(n_show.saturating_sub(1))
        .min(max_scroll);
    let mut lines: Vec<Line> = Vec::new();
    for (i, path) in at_matches[list_scroll..list_scroll + n_show]
        .iter()
        .enumerate()
    {
        let global = list_scroll + i;
        let st = if global == pick {
            Style::default()
                .fg(Color::Black)
                .bg(theme::USER)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT)
        };
        lines.push(Line::from(Span::styled(format!(" {path}"), st)));
    }
    if at_matches.len() > n_show {
        lines.push(Line::from(Span::styled(
            format!(" ─ {}/{} · ↑↓ Tab", pick + 1, at_matches.len()),
            Style::default().fg(theme::MUTED),
        )));
    }
    let at_w = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::BORDER))
                .title(Span::styled(
                    " files (@ mention) ",
                    Style::default().fg(theme::MUTED),
                )),
        )
        .style(Style::default().bg(theme::SURFACE));
    frame.render_widget(at_w, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_applies_first_match() {
        let files = vec!["src/main.rs".into()];
        let mut st = crate::tui::state::TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            std::path::PathBuf::from("/tmp"),
        );
        st.input_buffer = "see @src/m".into();
        st.cursor_char_idx = st.input_buffer.chars().count();
        handle_at_panel_key(
            &mut st,
            &files,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        assert!(st.input_buffer.contains("@src/main.rs"));
    }
}
