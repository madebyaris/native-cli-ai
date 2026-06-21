//! Permission mode picker popup.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── State + Actions ──────────────────────────────────────────────

pub(crate) struct PermissionPickerState {
    index: usize,
}

pub(crate) enum PermissionPickerAction {
    None,
    Close,
    ApplyPermission(usize),
}

const PERM_LABELS: &[&str] = &[
    "Default",
    "Plan",
    "AcceptEdits",
    "DontAsk",
    "BypassPermissions",
];

impl PermissionPickerState {
    pub fn new() -> Self {
        Self { index: 0 }
    }

    pub fn open(&mut self, current_index: usize) {
        self.index = current_index.min(PERM_LABELS.len() - 1);
    }

    pub fn close(&mut self) {
        self.index = 0;
    }

    pub fn is_open(&self) -> bool {
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PermissionPickerAction {
        const PERM_COUNT: usize = 5;
        match key.code {
            KeyCode::Esc => PermissionPickerAction::Close,
            KeyCode::Up => {
                self.index = self.index.saturating_sub(1);
                PermissionPickerAction::None
            }
            KeyCode::Down => {
                self.index = (self.index + 1).min(PERM_COUNT - 1);
                PermissionPickerAction::None
            }
            KeyCode::Enter => {
                let idx = self.index;
                PermissionPickerAction::ApplyPermission(idx)
            }
            _ => PermissionPickerAction::None,
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let rows = (PERM_LABELS.len() as u16).saturating_add(6).max(8);
        let popup_area = centered_rect(area, 40, rows);

        let mut lines: Vec<Line> = vec![
            Line::from(Span::styled(
                " Permission mode ",
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::default(),
        ];
        for (i, name) in PERM_LABELS.iter().enumerate() {
            let st = if i == self.index {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::USER)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT)
            };
            lines.push(Line::from(Span::styled(format!(" {name}"), st)));
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
                    .title(Span::styled(
                        " permissions ",
                        Style::default().fg(theme::MUTED),
                    )),
            )
            .style(Style::default().bg(theme::SURFACE))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, popup_area);
    }
}
