//! Agent profile picker popup.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── State + Actions ──────────────────────────────────────────────

pub(crate) struct AgentPickerState {
    index: usize,
}

pub(crate) enum AgentPickerAction {
    None,
    Close,
    SwitchAgent(usize),
}

const AGENT_LABELS: &[(&str, &str)] = &[
    ("@build", "Full-access agent for development"),
    ("@plan", "Read-only analysis and planning"),
    ("@review", "Focused code review"),
    ("@fix", "Bug diagnosis and minimal fixes"),
    ("@test", "Testing and validation"),
];

impl AgentPickerState {
    pub fn new() -> Self {
        Self { index: 0 }
    }

    pub fn open(&mut self, current_index: usize) {
        self.index = current_index.min(AGENT_LABELS.len() - 1);
    }

    pub fn close(&mut self) {
        self.index = 0;
    }

    pub fn is_open(&self) -> bool {
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> AgentPickerAction {
        const AGENT_COUNT: usize = 5;
        match key.code {
            KeyCode::Esc => AgentPickerAction::Close,
            KeyCode::Up => {
                self.index = self.index.saturating_sub(1);
                AgentPickerAction::None
            }
            KeyCode::Down => {
                self.index = (self.index + 1).min(AGENT_COUNT - 1);
                AgentPickerAction::None
            }
            KeyCode::Enter => {
                let idx = self.index;
                AgentPickerAction::SwitchAgent(idx)
            }
            _ => AgentPickerAction::None,
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let rows = (AGENT_LABELS.len() as u16).saturating_add(6).max(8);
        let popup_area = centered_rect(area, 52, rows);

        let mut lines: Vec<Line> = vec![
            Line::from(Span::styled(
                " Agent profile ",
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::default(),
        ];
        for (i, (name, desc)) in AGENT_LABELS.iter().enumerate() {
            let st = if i == self.index {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::USER)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT)
            };
            let desc_st = if i == self.index {
                Style::default().fg(Color::Black).bg(theme::USER)
            } else {
                Style::default().fg(theme::MUTED)
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {name:<10}"), st),
                Span::styled(format!(" {desc}"), desc_st),
            ]));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            " Enter apply · Esc cancel ",
            Style::default().fg(theme::MUTED),
        )));

        frame.render_widget(ClearWidget, popup_area);
        let popup = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(" agent ", Style::default().fg(theme::MUTED))),
            )
            .style(Style::default().bg(theme::SURFACE))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, popup_area);
    }
}
