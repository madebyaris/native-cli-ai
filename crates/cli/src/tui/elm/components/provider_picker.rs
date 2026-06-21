//! Provider picker popup (default provider or API-key target).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use nca_common::config::ProviderKind;

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── State + Actions ──────────────────────────────────────────────

pub(crate) struct ProviderPickerState {
    index: usize,
    for_api_key: bool,
}

pub(crate) enum ProviderPickerAction {
    None,
    Close,
    ApplyDefaultProvider(ProviderKind),
    PromptApiKey(ProviderKind),
}

impl ProviderPickerState {
    pub fn new() -> Self {
        Self {
            index: 0,
            for_api_key: false,
        }
    }

    pub fn open(&mut self, for_api_key: bool) {
        self.for_api_key = for_api_key;
        self.index = 0;
    }

    pub fn close(&mut self) {
        self.index = 0;
    }

    pub fn is_open(&self) -> bool {
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ProviderPickerAction {
        let n = ProviderKind::ALL.len();
        match key.code {
            KeyCode::Esc => ProviderPickerAction::Close,
            KeyCode::Up => {
                self.index = self.index.saturating_sub(1);
                ProviderPickerAction::None
            }
            KeyCode::Down => {
                if n > 0 {
                    self.index = (self.index + 1) % n;
                }
                ProviderPickerAction::None
            }
            KeyCode::Enter => {
                if n == 0 {
                    return ProviderPickerAction::Close;
                }
                let p = ProviderKind::ALL[self.index.min(n - 1)];
                let for_key = self.for_api_key;
                if for_key {
                    ProviderPickerAction::PromptApiKey(p)
                } else {
                    ProviderPickerAction::ApplyDefaultProvider(p)
                }
            }
            _ => ProviderPickerAction::None,
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let names: Vec<&'static str> = ProviderKind::ALL.iter().map(|p| p.display_name()).collect();
        let rows = (names.len() as u16).saturating_add(6).max(8);
        let popup_area = centered_rect(area, 40, rows);

        let mut lines: Vec<Line> = vec![
            Line::from(Span::styled(
                if self.for_api_key {
                    " Select provider for API key "
                } else {
                    " Default LLM provider "
                },
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::default(),
        ];
        for (i, name) in names.iter().enumerate() {
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
            " Enter confirm · Esc cancel ",
            Style::default().fg(theme::MUTED),
        )));

        frame.render_widget(ClearWidget, popup_area);
        let popup = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(
                        " settings ",
                        Style::default().fg(theme::MUTED),
                    )),
            )
            .style(Style::default().bg(theme::SURFACE))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, popup_area);
    }
}
