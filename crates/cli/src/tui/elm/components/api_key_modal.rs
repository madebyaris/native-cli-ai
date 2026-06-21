//! API key entry modal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use nca_common::config::ProviderKind;

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── State + Actions ──────────────────────────────────────────────

pub(crate) struct ApiKeyModalState {
    target_provider: Option<ProviderKind>,
    input: String,
    has_existing: bool,
    connect_after_save: bool,
}

pub(crate) enum ApiKeyModalAction {
    None,
    Close,
    Confirm(String),
}

impl ApiKeyModalState {
    pub fn new() -> Self {
        Self {
            target_provider: None,
            input: String::new(),
            has_existing: false,
            connect_after_save: false,
        }
    }

    pub fn open(&mut self, provider: ProviderKind, has_existing: bool, connect_after_save: bool) {
        self.target_provider = Some(provider);
        self.has_existing = has_existing;
        self.connect_after_save = connect_after_save;
        self.input.clear();
    }

    pub fn close(&mut self) {
        self.target_provider = None;
        self.input.clear();
    }

    pub fn is_open(&self) -> bool {
        true
    }

    /// Return the target provider (for action routing).
    pub fn provider(&self) -> Option<ProviderKind> {
        self.target_provider
    }

    /// Return whether to connect after saving the key.
    pub fn connect_after(&self) -> bool {
        self.connect_after_save
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ApiKeyModalAction {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => ApiKeyModalAction::Close,
            (KeyCode::Enter, _) => {
                let key = self.input.trim().to_string();
                ApiKeyModalAction::Confirm(key)
            }
            (KeyCode::Backspace, _) => {
                self.input.pop();
                ApiKeyModalAction::None
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.input.push(c);
                ApiKeyModalAction::None
            }
            _ => ApiKeyModalAction::None,
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let provider = self
            .target_provider
            .map(|p| p.display_name())
            .unwrap_or("provider");
        let popup_area = centered_rect(area, 66, 12);

        let headline = if self.connect_after_save {
            " Connect provider "
        } else {
            " API key "
        };
        let hint = if self.has_existing {
            " Press Enter to keep current key, or paste a new key to replace it. "
        } else {
            " Paste API key, then press Enter. "
        };
        let masked = if self.input.is_empty() {
            String::new()
        } else {
            "*".repeat(self.input.chars().count())
        };

        let lines = vec![
            Line::from(vec![Span::styled(
                format!(" Provider: {provider}"),
                Style::default()
                    .fg(theme::TEXT)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::default(),
            Line::from(vec![
                Span::styled(" API key ", Style::default().fg(theme::MUTED)),
                Span::styled(masked, Style::default().fg(theme::USER)),
            ]),
            Line::default(),
            Line::from(Span::styled(hint, Style::default().fg(theme::MUTED))),
            Line::from(Span::styled(
                " Enter confirm · Esc cancel ",
                Style::default().fg(theme::MUTED),
            )),
        ];

        frame.render_widget(ClearWidget, popup_area);
        let popup = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(headline, Style::default().fg(theme::MUTED))),
            )
            .style(Style::default().bg(theme::SURFACE))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, popup_area);
    }
}
