//! Session picker popup.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use nca_common::session::SessionSnapshot;

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── State + Actions ──────────────────────────────────────────────

pub(crate) struct SessionPickerState {
    search: String,
    index: usize,
    scroll: usize,
    entries: Vec<SessionSnapshot>,
    current_session_id: String,
}

pub(crate) enum SessionPickerAction {
    None,
    Close,
    ResumeSession(String),
}

const SESSION_PICKER_MAX_ROWS: usize = 16;

impl SessionPickerState {
    pub fn new() -> Self {
        Self {
            search: String::new(),
            index: 0,
            scroll: 0,
            entries: Vec::new(),
            current_session_id: String::new(),
        }
    }

    pub fn open(&mut self, entries: Vec<SessionSnapshot>, current_session_id: String) {
        self.entries = entries;
        self.current_session_id = current_session_id;
        self.search.clear();
        self.index = 0;
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        self.entries.clear();
        self.current_session_id.clear();
        self.search.clear();
        self.index = 0;
        self.scroll = 0;
    }

    pub fn is_open(&self) -> bool {
        true
    }

    fn filtered_count(&self) -> usize {
        let filter = self.search.to_ascii_lowercase();
        self.entries
            .iter()
            .filter(|s| {
                filter.is_empty()
                    || s.id.to_ascii_lowercase().contains(&filter)
                    || s.session_title
                        .as_ref()
                        .map(|t| t.to_ascii_lowercase().contains(&filter))
                        .unwrap_or(false)
            })
            .count()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SessionPickerAction {
        let count = self.filtered_count();
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => SessionPickerAction::Close,
            (KeyCode::Up, _) => {
                self.index = self.index.saturating_sub(1);
                SessionPickerAction::None
            }
            (KeyCode::Down, _) => {
                if count > 0 {
                    self.index = (self.index + 1).min(count.saturating_sub(1));
                }
                SessionPickerAction::None
            }
            (KeyCode::Enter, _) => {
                let filter = self.search.to_ascii_lowercase();
                let filtered: Vec<&SessionSnapshot> = self
                    .entries
                    .iter()
                    .filter(|s| {
                        filter.is_empty()
                            || s.id.to_ascii_lowercase().contains(&filter)
                            || s.session_title
                                .as_ref()
                                .map(|t| t.to_ascii_lowercase().contains(&filter))
                                .unwrap_or(false)
                    })
                    .collect();
                let pick = self.index.min(filtered.len().saturating_sub(1));
                if let Some(snap) = filtered.get(pick) {
                    let id = snap.id.clone();
                    return SessionPickerAction::ResumeSession(id);
                }
                SessionPickerAction::None
            }
            (KeyCode::Backspace, _) => {
                self.search.pop();
                self.index = 0;
                self.scroll = 0;
                SessionPickerAction::None
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.search.push(c);
                self.index = 0;
                self.scroll = 0;
                SessionPickerAction::None
            }
            _ => SessionPickerAction::None,
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let filter = self.search.to_ascii_lowercase();
        let filtered_indices: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                filter.is_empty()
                    || s.id.to_ascii_lowercase().contains(&filter)
                    || s.session_title
                        .as_ref()
                        .map(|t| t.to_ascii_lowercase().contains(&filter))
                        .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect();

        let n_filtered = filtered_indices.len();
        let viewport_rows = n_filtered.min(SESSION_PICKER_MAX_ROWS);
        let rows = (viewport_rows as u16).saturating_add(8).max(10);
        let popup_area = centered_rect(area, 56, rows);
        let pick = self.index.min(n_filtered.saturating_sub(1));

        // Adjust scroll
        if pick < self.scroll {
            self.scroll = pick;
        } else if viewport_rows > 0 && pick >= self.scroll + viewport_rows {
            self.scroll = pick.saturating_sub(viewport_rows - 1);
        }
        self.scroll = self.scroll.min(n_filtered.saturating_sub(viewport_rows));
        let list_start = self.scroll;
        let list_end = (list_start + viewport_rows).min(n_filtered);

        let search_display = if self.search.is_empty() {
            "type to filter".to_string()
        } else {
            self.search.clone()
        };

        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled(
                    " Search ",
                    Style::default()
                        .fg(theme::MUTED)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(search_display, Style::default().fg(theme::TEXT)),
            ]),
            Line::default(),
        ];

        if filtered_indices.is_empty() {
            lines.push(Line::from(Span::styled(
                " No matching sessions",
                Style::default().fg(theme::MUTED),
            )));
        } else {
            if list_start > 0 {
                lines.push(Line::from(Span::styled(
                    format!("  ▲ {} more", list_start),
                    Style::default().fg(theme::MUTED),
                )));
            }
            for (vis_idx, &filt_idx) in filtered_indices
                .iter()
                .enumerate()
                .skip(list_start)
                .take(list_end.saturating_sub(list_start))
            {
                let snap = &self.entries[filt_idx];
                let is_current = snap.id == self.current_session_id;
                let marker = if is_current { " *" } else { "" };
                let st = if vis_idx == pick {
                    Style::default()
                        .fg(Color::Black)
                        .bg(theme::USER)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::TEXT)
                };
                let display = if let Some(title) = &snap.session_title {
                    format!(" {title}{marker}  [{}]", snap.id)
                } else {
                    format!(" {}{marker}", snap.id)
                };
                lines.push(Line::from(Span::styled(display, st)));
            }
            let remaining_below = n_filtered.saturating_sub(list_end);
            if remaining_below > 0 {
                lines.push(Line::from(Span::styled(
                    format!("  ▼ {} more", remaining_below),
                    Style::default().fg(theme::MUTED),
                )));
            }
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            " Enter resume · Esc close ",
            Style::default().fg(theme::MUTED),
        )));

        frame.render_widget(ClearWidget, popup_area);
        let popup = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(
                        " sessions ",
                        Style::default().fg(theme::MUTED),
                    )),
            )
            .style(Style::default().bg(theme::SURFACE))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, popup_area);
    }
}
