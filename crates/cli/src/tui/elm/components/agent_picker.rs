//! Agent profile picker popup.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::searchable_list::{ClearWidget, centered_rect, theme};

/// Maximum visible agent rows before scrolling kicks in.
const MAX_VISIBLE_ROWS: usize = 12;

// ── State + Actions ──────────────────────────────────────────────

pub(crate) struct AgentPickerState {
    index: usize,
    /// Dynamic agent labels: (display_name, description) pairs.
    /// Populated at open time by the caller.
    labels: Vec<(String, String)>,
}

pub(crate) enum AgentPickerAction {
    None,
    Close,
    SwitchAgent(usize),
}

impl AgentPickerState {
    pub fn new() -> Self {
        Self {
            index: 0,
            labels: Vec::new(),
        }
    }

    pub fn open(&mut self, labels: Vec<(String, String)>, current_index: usize) {
        self.labels = labels;
        let max = self.labels.len().saturating_sub(1);
        self.index = current_index.min(max);
    }

    pub fn close(&mut self) {
        self.index = 0;
    }

    pub fn is_open(&self) -> bool {
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> AgentPickerAction {
        let count = self.labels.len().max(1);
        match key.code {
            KeyCode::Esc => AgentPickerAction::Close,
            KeyCode::Up => {
                self.index = self.index.saturating_sub(1);
                AgentPickerAction::None
            }
            KeyCode::Down => {
                self.index = (self.index + 1).min(count - 1);
                AgentPickerAction::None
            }
            KeyCode::Enter => {
                let idx = self.index;
                AgentPickerAction::SwitchAgent(idx)
            }
            _ => AgentPickerAction::None,
        }
    }

    /// Compute the scroll offset so the selected index stays within the
    /// visible window.  Returns `(scroll_offset, n_show)`.
    fn visible_window(&self) -> (usize, usize) {
        let total = self.labels.len();
        let n_show = total.min(MAX_VISIBLE_ROWS);
        if total <= n_show {
            return (0, n_show);
        }
        // Keep the cursor centered-ish: scroll only when near the edges.
        let scroll = self
            .index
            .saturating_sub(n_show / 2)
            .min(total.saturating_sub(n_show));
        (scroll, n_show)
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let (scroll, n_show) = self.visible_window();
        let rows = (n_show as u16).saturating_add(6).max(8);
        let popup_area = centered_rect(area, 60, rows);

        let mut lines: Vec<Line> = vec![
            Line::from(Span::styled(
                " Agent ",
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::default(),
        ];

        let end = (scroll + n_show).min(self.labels.len());
        for actual_i in scroll..end {
            let (name, desc) = &self.labels[actual_i];
            let is_selected = actual_i == self.index;
            let st = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::USER)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT)
            };
            let desc_st = if is_selected {
                Style::default().fg(Color::Black).bg(theme::USER)
            } else {
                Style::default().fg(theme::MUTED)
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {name:<14}"), st),
                Span::styled(format!(" {desc}"), desc_st),
            ]));
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            " Enter apply · Esc cancel ",
            Style::default().fg(theme::MUTED),
        )));

        frame.render_widget(ClearWidget, popup_area);
        let popup = Paragraph::new(Text::from(lines)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::BORDER))
                .title(Span::styled(" agent ", Style::default().fg(theme::MUTED))),
        );
        frame.render_widget(popup, popup_area);
    }
}
