//! Connect provider modal (`/connect`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use nca_common::config::ProviderKind;

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── Data ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum ConnectRow {
    SectionHeader(String),
    Provider {
        title: String,
        subtitle: String,
        provider: ProviderKind,
    },
}

fn build_connect_rows(search: &str) -> Vec<ConnectRow> {
    let needle = search.trim().to_ascii_lowercase();
    let catalog = &[
        (
            "Popular",
            ProviderKind::MiniMax,
            "MiniMax",
            "Recommended · M2.5 (API key)",
        ),
        (
            "Popular",
            ProviderKind::OpenAi,
            "OpenAI",
            "GPT models (API key)",
        ),
        (
            "Popular",
            ProviderKind::Anthropic,
            "Anthropic",
            "Claude (API key)",
        ),
        (
            "Popular",
            ProviderKind::ZhipuAI,
            "ZhipuAI",
            "GLM-5 Turbo (API key)",
        ),
        (
            "Popular",
            ProviderKind::DeepSeek,
            "DeepSeek",
            "V4 Flash / V4 Pro (API key)",
        ),
        (
            "Other",
            ProviderKind::OpenRouter,
            "OpenRouter",
            "Multi-model routing (API key)",
        ),
    ];

    let mut out = Vec::new();
    let mut last_section = String::new();
    for &(section, kind, title, subtitle) in catalog {
        let title_match = title.to_ascii_lowercase().contains(&needle)
            || subtitle.to_ascii_lowercase().contains(&needle);
        let show = needle.is_empty() || title_match;
        if show {
            if section != last_section {
                out.push(ConnectRow::SectionHeader(section.to_string()));
                last_section = section.to_string();
            }
            out.push(ConnectRow::Provider {
                title: title.to_string(),
                subtitle: subtitle.to_string(),
                provider: kind,
            });
        }
    }
    out
}

fn selectable_row_indices(rows: &[ConnectRow]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| matches!(r, ConnectRow::Provider { .. }).then_some(i))
        .collect()
}

fn provider_at_selection(rows: &[ConnectRow], selection: usize) -> Option<ProviderKind> {
    let sel_indices = selectable_row_indices(rows);
    let &row_idx = sel_indices.get(selection)?;
    if let ConnectRow::Provider { provider, .. } = &rows[row_idx] {
        Some(*provider)
    } else {
        None
    }
}

// ── State + Actions ────────────────────────────────────────────────

pub(crate) struct ConnectModalState {
    search: String,
    index: usize,
}

pub(crate) enum ConnectModalAction {
    None,
    Close,
    ConnectProvider(ProviderKind),
}

impl ConnectModalState {
    pub fn new() -> Self {
        Self {
            search: String::new(),
            index: 0,
        }
    }

    pub fn open(&mut self) {
        self.search.clear();
        self.index = 0;
    }

    pub fn close(&mut self) {
        self.search.clear();
        self.index = 0;
    }

    pub fn is_open(&self) -> bool {
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ConnectModalAction {
        let rows = build_connect_rows(&self.search);
        let n_sel = selectable_row_indices(&rows).len();

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => ConnectModalAction::Close,
            (KeyCode::Up, _) if n_sel > 0 => {
                self.index = self.index.saturating_sub(1).min(n_sel - 1);
                ConnectModalAction::None
            }
            (KeyCode::Down, _) if n_sel > 0 => {
                self.index = (self.index + 1).min(n_sel - 1);
                ConnectModalAction::None
            }
            (KeyCode::Enter, _) => {
                if let Some(p) = provider_at_selection(&rows, self.index) {
                    ConnectModalAction::ConnectProvider(p)
                } else {
                    ConnectModalAction::None
                }
            }
            (KeyCode::Backspace, _) => {
                self.search.pop();
                self.index = 0;
                let rows2 = build_connect_rows(&self.search);
                self.index = self
                    .index
                    .min(selectable_row_indices(&rows2).len().saturating_sub(1));
                ConnectModalAction::None
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.search.push(c);
                self.index = 0;
                let rows2 = build_connect_rows(&self.search);
                self.index = self
                    .index
                    .min(selectable_row_indices(&rows2).len().saturating_sub(1));
                ConnectModalAction::None
            }
            _ => ConnectModalAction::None,
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let rows = build_connect_rows(&self.search);
        let sel_indices = selectable_row_indices(&rows);
        let selected_row = sel_indices.get(self.index).copied();

        let body_lines = rows.len().max(1);
        let popup_h = (body_lines as u16).saturating_add(9).clamp(11, 24);
        let popup_area = centered_rect(area, 58, popup_h);

        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled(
                    "Search ",
                    Style::default()
                        .fg(theme::MUTED)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if self.search.is_empty() {
                        "type to filter…"
                    } else {
                        &self.search
                    },
                    Style::default().fg(theme::TEXT),
                ),
            ]),
            Line::default(),
        ];

        if rows.is_empty() {
            lines.push(Line::from(Span::styled(
                " No providers match",
                Style::default().fg(theme::MUTED),
            )));
        } else {
            for (i, row) in rows.iter().enumerate() {
                match row {
                    ConnectRow::SectionHeader(h) => {
                        lines.push(Line::from(Span::styled(
                            format!(" {h}"),
                            Style::default()
                                .fg(theme::ASSISTANT)
                                .add_modifier(Modifier::BOLD),
                        )));
                    }
                    ConnectRow::Provider {
                        title, subtitle, ..
                    } => {
                        let is_sel = selected_row == Some(i);
                        let main_st = if is_sel {
                            Style::default()
                                .fg(Color::Black)
                                .bg(theme::USER)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme::TEXT)
                        };
                        let sub_st = if is_sel {
                            main_st
                        } else {
                            Style::default().fg(theme::MUTED)
                        };
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {title}"), main_st),
                            Span::styled(format!(" — {subtitle}"), sub_st),
                        ]));
                    }
                }
            }
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            " ↑↓ select · Enter connect · Esc close ",
            Style::default().fg(theme::MUTED),
        )));

        frame.render_widget(ClearWidget, popup_area);
        let title = Line::from(vec![
            Span::styled(" Connect a provider ", Style::default().fg(theme::MUTED)),
            Span::styled(" esc ", Style::default().fg(theme::MUTED)),
        ]);
        let popup = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(title),
            )
            .style(Style::default().bg(theme::SURFACE))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, popup_area);
    }
}
