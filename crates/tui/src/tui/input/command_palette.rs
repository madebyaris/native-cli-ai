//! Command palette (`Ctrl+P`) input + render.

use crate::tui::composer::{
    PaletteRow, filter_palette_rows, palette_command_for_label, palette_selectable_indices,
};
use crate::tui::layout::{COMMAND_PALETTE_MAX_ROWS, COMMAND_PALETTE_WIDTH, centered_rect};
use crate::tui::state::TuiSessionState;
use crate::tui::theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear as ClearWidget, Paragraph, Wrap};

/// Result of handling a key while the palette is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteKeyResult {
    Handled,
    Closed,
    Execute(String),
}

pub fn handle_command_palette_key(state: &mut TuiSessionState, key: KeyEvent) -> PaletteKeyResult {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            state.close_command_palette();
            if let Some(q) = state.command_palette_query_mut() {
                q.clear();
            }
            if let Some(idx) = state.palette_index_mut() {
                *idx = 0;
            }
            PaletteKeyResult::Closed
        }
        (KeyCode::Up, _) => {
            if let Some(idx) = state.palette_index_mut()
                && *idx > 0
            {
                *idx -= 1;
            }
            PaletteKeyResult::Handled
        }
        (KeyCode::Down, _) => {
            let filtered = filter_palette_rows(state.command_palette_query());
            let selectable = palette_selectable_indices(&filtered);
            if let Some(idx) = state.palette_index_mut()
                && !selectable.is_empty()
            {
                *idx = (*idx + 1).min(selectable.len().saturating_sub(1));
            }
            PaletteKeyResult::Handled
        }
        (KeyCode::Enter, _) => {
            let filtered = filter_palette_rows(state.command_palette_query());
            let selectable = palette_selectable_indices(&filtered);
            let pick = state
                .palette_index()
                .min(selectable.len().saturating_sub(1));
            let command = if let Some(&abs_idx) = selectable.get(pick)
                && let PaletteRow::Entry { label, .. } = filtered[abs_idx]
            {
                Some(palette_command_for_label(label).to_string())
            } else {
                None
            };
            state.close_command_palette();
            if let Some(q) = state.command_palette_query_mut() {
                q.clear();
            }
            if let Some(idx) = state.palette_index_mut() {
                *idx = 0;
            }
            command
                .map(PaletteKeyResult::Execute)
                .unwrap_or(PaletteKeyResult::Closed)
        }
        (KeyCode::Backspace, _) => {
            if let Some(q) = state.command_palette_query_mut() {
                q.pop();
            }
            let filtered = filter_palette_rows(state.command_palette_query());
            let selectable = palette_selectable_indices(&filtered);
            if let Some(idx) = state.palette_index_mut() {
                *idx = (*idx).min(selectable.len().saturating_sub(1));
            }
            PaletteKeyResult::Handled
        }
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            if let Some(q) = state.command_palette_query_mut() {
                q.push(c);
            }
            let filtered = filter_palette_rows(state.command_palette_query());
            let selectable = palette_selectable_indices(&filtered);
            if let Some(idx) = state.palette_index_mut() {
                *idx = (*idx).min(selectable.len().saturating_sub(1));
            }
            PaletteKeyResult::Handled
        }
        _ => PaletteKeyResult::Handled,
    }
}

pub fn render_command_palette(frame: &mut Frame, area: Rect, state: &TuiSessionState) {
    let filtered = filter_palette_rows(state.command_palette_query());
    let selectable = palette_selectable_indices(&filtered);
    let pick_abs = if selectable.is_empty() {
        0
    } else {
        selectable[state
            .palette_index()
            .min(selectable.len().saturating_sub(1))]
    };
    let total_vis = filtered.len().clamp(1, COMMAND_PALETTE_MAX_ROWS);
    let popup_area = centered_rect(
        area,
        COMMAND_PALETTE_WIDTH,
        (total_vis as u16).saturating_add(6),
    );
    let list_scroll = pick_abs.saturating_sub(COMMAND_PALETTE_MAX_ROWS / 2);
    let list_end = (list_scroll + COMMAND_PALETTE_MAX_ROWS).min(filtered.len());
    let query = state.command_palette_query();
    let mut popup_lines = vec![
        Line::from(vec![
            Span::styled(
                "  Search ",
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if query.is_empty() {
                    "type to filter"
                } else {
                    query
                },
                Style::default().fg(theme::TEXT),
            ),
        ]),
        Line::default(),
    ];
    if selectable.is_empty() {
        popup_lines.push(Line::from(Span::styled(
            " No matching commands",
            Style::default().fg(theme::MUTED),
        )));
    } else {
        for &idx in &filtered[list_scroll..list_end] {
            match idx {
                PaletteRow::Section(name) => {
                    popup_lines.push(Line::from(Span::styled(
                        format!("  {name}"),
                        Style::default()
                            .fg(theme::MUTED)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                    )));
                }
                PaletteRow::Entry {
                    label, shortcut, ..
                } => {
                    let global = filtered
                        .iter()
                        .position(|r| std::ptr::eq(*r, idx))
                        .unwrap_or(0);
                    let is_selected = global == pick_abs;
                    let label_style = if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(theme::USER)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme::TEXT)
                    };
                    let shortcut_style = if is_selected {
                        Style::default().fg(Color::Black).bg(theme::USER)
                    } else {
                        Style::default().fg(theme::MUTED)
                    };
                    let pad = 36usize.saturating_sub(label.len()).saturating_sub(2);
                    let mut spans = vec![Span::styled(format!("  {label}"), label_style)];
                    if !shortcut.is_empty() {
                        spans.push(Span::styled(
                            format!("{:>pad$}", shortcut, pad = pad),
                            shortcut_style,
                        ));
                    }
                    popup_lines.push(Line::from(spans));
                }
            }
        }
    }
    popup_lines.push(Line::default());
    popup_lines.push(Line::from(Span::styled(
        " Enter apply · Esc close ",
        Style::default().fg(theme::MUTED),
    )));
    frame.render_widget(ClearWidget, popup_area);
    let popup = Paragraph::new(Text::from(popup_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::BORDER))
                .title(Span::styled(
                    " command palette (ctrl+p) ",
                    Style::default().fg(theme::MUTED),
                )),
        )
        .style(Style::default().bg(theme::SURFACE))
        .wrap(Wrap { trim: false });
    frame.render_widget(popup, popup_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn esc_closes_palette() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.open_command_palette();
        assert_eq!(
            handle_command_palette_key(&mut st, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            PaletteKeyResult::Closed
        );
        assert!(!st.command_palette_open());
    }

    #[test]
    fn typing_filters_query() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.open_command_palette();
        handle_command_palette_key(
            &mut st,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        );
        assert_eq!(st.command_palette_query(), "h");
    }
}
