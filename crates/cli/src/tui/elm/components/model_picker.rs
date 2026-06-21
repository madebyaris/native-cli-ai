//! Model picker popup (Ctrl+X M).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use nca_common::config::ProviderKind;

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── Types ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum ModelPickerAction {
    SwitchProvider(ProviderKind),
    ApplyModel(String),
}

#[derive(Debug, Clone)]
pub(crate) struct ModelPickerEntry {
    pub label: String,
    pub detail: String,
    pub action: ModelPickerAction,
    pub is_header: bool,
}

// ── State + Actions ──────────────────────────────────────────────

pub(crate) struct ModelPickerState {
    search: String,
    index: usize,
    scroll: usize,
    entries: Vec<ModelPickerEntry>,
}

pub(crate) enum ModelPickerPopupAction {
    None,
    Close,
    ApplyModel(String),
    SwitchProvider(ProviderKind),
}

const MODEL_PICKER_MAX_ROWS: usize = 18;

impl ModelPickerState {
    pub fn new() -> Self {
        Self {
            search: String::new(),
            index: 0,
            scroll: 0,
            entries: Vec::new(),
        }
    }

    pub fn open(&mut self, entries: Vec<ModelPickerEntry>) {
        self.entries = entries;
        self.search.clear();
        self.index = 0;
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        self.entries.clear();
        self.search.clear();
        self.index = 0;
        self.scroll = 0;
    }

    pub fn is_open(&self) -> bool {
        true
    }

    fn selectable_count(&self) -> usize {
        let filter = self.search.to_ascii_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                !e.is_header
                    && (filter.is_empty()
                        || e.label.to_ascii_lowercase().contains(&filter)
                        || e.detail.to_ascii_lowercase().contains(&filter))
            })
            .count()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModelPickerPopupAction {
        let selectable_count = self.selectable_count();
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => ModelPickerPopupAction::Close,
            (KeyCode::Up, _) if selectable_count > 0 => {
                self.index = self.index.saturating_sub(1).min(selectable_count - 1);
                ModelPickerPopupAction::None
            }
            (KeyCode::Down, _) if selectable_count > 0 => {
                self.index = (self.index + 1).min(selectable_count - 1);
                ModelPickerPopupAction::None
            }
            (KeyCode::Enter, _) => {
                let filter = self.search.to_ascii_lowercase();
                let selectable: Vec<&ModelPickerEntry> = self
                    .entries
                    .iter()
                    .filter(|e| {
                        !e.is_header
                            && (filter.is_empty()
                                || e.label.to_ascii_lowercase().contains(&filter)
                                || e.detail.to_ascii_lowercase().contains(&filter))
                    })
                    .collect();
                let pick = self.index.min(selectable.len().saturating_sub(1));
                if let Some(entry) = selectable.get(pick) {
                    let action = entry.action.clone();
                    return match action {
                        ModelPickerAction::SwitchProvider(p) => {
                            ModelPickerPopupAction::SwitchProvider(p)
                        }
                        ModelPickerAction::ApplyModel(m) => ModelPickerPopupAction::ApplyModel(m),
                    };
                }
                ModelPickerPopupAction::None
            }
            (KeyCode::Backspace, _) => {
                self.search.pop();
                self.index = 0;
                self.scroll = 0;
                ModelPickerPopupAction::None
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.search.push(c);
                self.index = 0;
                self.scroll = 0;
                ModelPickerPopupAction::None
            }
            _ => ModelPickerPopupAction::None,
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let filter = self.search.to_ascii_lowercase();

        // Pre-compute visible indices
        let vis_indices: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.is_header
                    || filter.is_empty()
                    || e.label.to_ascii_lowercase().contains(&filter)
                    || e.detail.to_ascii_lowercase().contains(&filter)
            })
            .map(|(i, _)| i)
            .collect();
        let selectable_vis: Vec<usize> = vis_indices
            .iter()
            .enumerate()
            .filter(|&(_, &orig)| !self.entries[orig].is_header)
            .map(|(vi, _)| vi)
            .collect();
        let n_sel = selectable_vis.len();
        let pick = if n_sel > 0 {
            self.index.min(n_sel - 1)
        } else {
            0
        };
        let selected_vis_idx = selectable_vis.get(pick).copied().unwrap_or(0);

        let n_visible = vis_indices.len();
        let viewport_rows = n_visible.min(MODEL_PICKER_MAX_ROWS);
        let popup_h = (viewport_rows as u16).saturating_add(7).max(10);
        let popup_area = centered_rect(area, 62, popup_h);

        // Keep selected item visible
        let mut scroll = self.scroll;
        if selected_vis_idx < scroll {
            scroll = selected_vis_idx;
        } else if viewport_rows > 0 && selected_vis_idx >= scroll + viewport_rows {
            scroll = selected_vis_idx.saturating_sub(viewport_rows - 1);
        }
        scroll = scroll.min(n_visible.saturating_sub(viewport_rows));
        let list_start = scroll;
        let list_end = (list_start + viewport_rows).min(n_visible);

        let search_display = if self.search.is_empty() {
            "type to filter…".to_string()
        } else {
            self.search.clone()
        };

        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled(
                    "Search ",
                    Style::default()
                        .fg(theme::MUTED)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(search_display, Style::default().fg(theme::TEXT)),
            ]),
            Line::default(),
        ];

        if vis_indices.is_empty() {
            lines.push(Line::from(Span::styled(
                " No models match",
                Style::default().fg(theme::MUTED),
            )));
        } else {
            if list_start > 0 {
                lines.push(Line::from(Span::styled(
                    format!("  ▲ {} more", list_start),
                    Style::default().fg(theme::MUTED),
                )));
            }
            for (vi, &model_idx) in vis_indices
                .iter()
                .enumerate()
                .skip(list_start)
                .take(list_end.saturating_sub(list_start))
            {
                let entry = &self.entries[model_idx];
                if entry.is_header {
                    lines.push(Line::from(Span::styled(
                        format!(" {}", entry.label),
                        Style::default()
                            .fg(theme::ASSISTANT)
                            .add_modifier(Modifier::BOLD),
                    )));
                } else {
                    let is_sel = selected_vis_idx == vi;
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
                        Span::styled(format!("   {}", entry.label), main_st),
                        Span::styled(format!("  {}", entry.detail), sub_st),
                    ]));
                }
            }
            let remaining_below = n_visible.saturating_sub(list_end);
            if remaining_below > 0 {
                lines.push(Line::from(Span::styled(
                    format!("  ▼ {} more", remaining_below),
                    Style::default().fg(theme::MUTED),
                )));
            }
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            " ↑↓ select · Enter apply · Esc close ",
            Style::default().fg(theme::MUTED),
        )));

        frame.render_widget(ClearWidget, popup_area);
        let popup = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(" models ", Style::default().fg(theme::MUTED))),
            )
            .style(Style::default().bg(theme::SURFACE))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, popup_area);
    }
}
