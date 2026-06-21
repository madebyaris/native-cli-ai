//! Info/help/status modal (read-only scrollable popup).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── State + Actions ──────────────────────────────────────────────

pub(crate) struct InfoModalState {
    title: String,
    lines: Vec<String>,
    scroll: usize,
}

pub(crate) enum InfoModalAction {
    None,
    Close,
}

const INFO_MODAL_MAX_VIS: usize = 16;

impl InfoModalState {
    pub fn new() -> Self {
        Self {
            title: String::new(),
            lines: Vec::new(),
            scroll: 0,
        }
    }

    pub fn open(&mut self, title: String, lines: Vec<String>) {
        self.title = title;
        self.lines = lines;
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        self.title.clear();
        self.lines.clear();
        self.scroll = 0;
    }

    pub fn is_open(&self) -> bool {
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> InfoModalAction {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => InfoModalAction::Close,
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.scroll = self.scroll.saturating_sub(1);
                InfoModalAction::None
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                let max_scroll = self.lines.len().saturating_sub(INFO_MODAL_MAX_VIS);
                self.scroll = (self.scroll + 1).min(max_scroll);
                InfoModalAction::None
            }
            (KeyCode::Home, _) => {
                self.scroll = 0;
                InfoModalAction::None
            }
            (KeyCode::End, _) => {
                self.scroll = self.lines.len().saturating_sub(INFO_MODAL_MAX_VIS);
                InfoModalAction::None
            }
            _ => InfoModalAction::None,
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let n_lines = self.lines.len();
        let n_show = n_lines.min(INFO_MODAL_MAX_VIS);
        let max_scroll = n_lines.saturating_sub(n_show);
        self.scroll = self.scroll.min(max_scroll);
        let start = self.scroll;
        let end = (start + n_show).min(n_lines);
        let popup_h = (n_show as u16).saturating_add(6).max(8);
        let popup_area = centered_rect(area, 70, popup_h);

        let mut lines: Vec<Line> = Vec::new();
        for line in &self.lines[start..end] {
            lines.push(Line::from(Span::styled(
                format!(" {line}"),
                Style::default().fg(theme::TEXT),
            )));
        }
        if n_lines > INFO_MODAL_MAX_VIS {
            lines.push(Line::from(Span::styled(
                format!(" ─ {}/{} · ↑↓ scroll", start + 1, n_lines),
                Style::default().fg(theme::MUTED),
            )));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            " Esc close ",
            Style::default().fg(theme::MUTED),
        )));

        frame.render_widget(ClearWidget, popup_area);
        let title = format!(" {} ", self.title);
        let popup = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(title, Style::default().fg(theme::MUTED))),
            )
            .style(Style::default().bg(theme::SURFACE))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, popup_area);
    }
}
