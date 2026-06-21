//! Command palette popup (Ctrl+P).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── Data ──────────────────────────────────────────────────────────

/// A row in the categorized command palette.
#[derive(Clone, Copy)]
enum PaletteRow {
    Section(&'static str),
    Entry {
        label: &'static str,
        shortcut: &'static str,
    },
}

const PALETTE_CATALOG: &[PaletteRow] = &[
    PaletteRow::Section("Suggested"),
    PaletteRow::Entry {
        label: "Switch model",
        shortcut: "ctrl+x m",
    },
    PaletteRow::Entry {
        label: "Connect provider",
        shortcut: "",
    },
    PaletteRow::Section("Session"),
    PaletteRow::Entry {
        label: "Open editor",
        shortcut: "ctrl+x e",
    },
    PaletteRow::Entry {
        label: "Switch session",
        shortcut: "ctrl+x l",
    },
    PaletteRow::Entry {
        label: "New session",
        shortcut: "ctrl+x n",
    },
    PaletteRow::Entry {
        label: "Compact",
        shortcut: "ctrl+x c",
    },
    PaletteRow::Entry {
        label: "Export session",
        shortcut: "",
    },
    PaletteRow::Section("Prompt"),
    PaletteRow::Entry {
        label: "Skills",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "Agent profile",
        shortcut: "ctrl+x a",
    },
    PaletteRow::Entry {
        label: "Toggle thinking",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "Toggle tool output",
        shortcut: "",
    },
    PaletteRow::Section("Provider"),
    PaletteRow::Entry {
        label: "Connect provider",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "Switch provider",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "API key",
        shortcut: "",
    },
    PaletteRow::Section("System"),
    PaletteRow::Entry {
        label: "View status",
        shortcut: "ctrl+x s",
    },
    PaletteRow::Entry {
        label: "Config",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "Doctor",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "Help",
        shortcut: "ctrl+x h",
    },
    PaletteRow::Entry {
        label: "Permissions",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "Memory",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "Logs",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "MCP servers",
        shortcut: "",
    },
    PaletteRow::Entry {
        label: "Clear screen",
        shortcut: "ctrl+l",
    },
    PaletteRow::Entry {
        label: "Exit",
        shortcut: "ctrl+x q",
    },
];

const COMMAND_PALETTE_WIDTH: u16 = 48;
const COMMAND_PALETTE_MAX_ROWS: usize = 10;

// ── Filter helpers ───────────────────────────────────────────────

fn filter_palette_rows(query: &str) -> Vec<&'static PaletteRow> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return PALETTE_CATALOG.iter().collect();
    }
    let mut result: Vec<&'static PaletteRow> = Vec::new();
    let mut pending_section: Option<&'static PaletteRow> = None;
    for row in PALETTE_CATALOG {
        match row {
            PaletteRow::Section(_) => {
                pending_section = Some(row);
            }
            PaletteRow::Entry { label, shortcut } => {
                if label.to_ascii_lowercase().contains(&needle)
                    || shortcut.to_ascii_lowercase().contains(&needle)
                {
                    if let Some(sec) = pending_section.take() {
                        result.push(sec);
                    }
                    result.push(row);
                }
            }
        }
    }
    result
}

fn palette_selectable_indices(rows: &[&PaletteRow]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| matches!(r, PaletteRow::Entry { .. }).then_some(i))
        .collect()
}

fn palette_command_for_label(label: &str) -> &'static str {
    match label {
        "Switch model" => "/models",
        "Connect provider" => "/connect",
        "Open editor" => "/editor",
        "Switch session" => "/sessions",
        "New session" => "/new",
        "Compact" => "/compact",
        "Export session" => "/export",
        "Skills" => "/skills",
        "Agent profile" => "/agent",
        "Toggle thinking" => "/thinking",
        "Toggle tool output" => "/tool-output",
        "Switch provider" => "/provider",
        "API key" => "/apikey",
        "View status" => "/status",
        "Config" => "/config",
        "Doctor" => "/doctor",
        "Help" => "/help",
        "Permissions" => "/permissions",
        "Memory" => "/memory",
        "Logs" => "/logs",
        "MCP servers" => "/mcp",
        "Clear screen" => "/clear",
        "Exit" => "/exit",
        _ => "",
    }
}

// ── State + Actions ─────────────────────────────────────────────

pub(crate) struct CommandPaletteState {
    query: String,
    index: usize,
}

pub(crate) enum CommandPaletteAction {
    None,
    Close,
    SubmitCommand(String),
}

impl CommandPaletteState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            index: 0,
        }
    }

    pub fn open(&mut self) {
        self.query.clear();
        self.index = 0;
    }

    pub fn close(&mut self) {
        self.query.clear();
        self.index = 0;
    }

    pub fn is_open(&self) -> bool {
        true // mounted = open
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CommandPaletteAction {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                CommandPaletteAction::Close
            }
            (KeyCode::Up, _) => {
                if self.index > 0 {
                    self.index -= 1;
                }
                CommandPaletteAction::None
            }
            (KeyCode::Down, _) => {
                let filtered = filter_palette_rows(&self.query);
                let selectable = palette_selectable_indices(&filtered);
                if !selectable.is_empty() {
                    self.index = (self.index + 1).min(selectable.len().saturating_sub(1));
                }
                CommandPaletteAction::None
            }
            (KeyCode::Enter, _) => {
                let filtered = filter_palette_rows(&self.query);
                let selectable = palette_selectable_indices(&filtered);
                let pick = self.index.min(selectable.len().saturating_sub(1));
                let cmd = if let Some(&abs_idx) = selectable.get(pick)
                    && let PaletteRow::Entry { label, .. } = filtered[abs_idx]
                {
                    palette_command_for_label(label).to_string()
                } else {
                    String::new()
                };
                CommandPaletteAction::SubmitCommand(cmd)
            }
            (KeyCode::Backspace, _) => {
                self.query.pop();
                let filtered = filter_palette_rows(&self.query);
                let selectable = palette_selectable_indices(&filtered);
                self.index = self.index.min(selectable.len().saturating_sub(1));
                CommandPaletteAction::None
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.query.push(c);
                let filtered = filter_palette_rows(&self.query);
                let selectable = palette_selectable_indices(&filtered);
                self.index = self.index.min(selectable.len().saturating_sub(1));
                CommandPaletteAction::None
            }
            _ => CommandPaletteAction::None,
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let filtered = filter_palette_rows(&self.query);
        let selectable = palette_selectable_indices(&filtered);
        let pick_abs = if selectable.is_empty() {
            0
        } else {
            selectable[self.index.min(selectable.len().saturating_sub(1))]
        };
        let total_vis = filtered.len().clamp(1, COMMAND_PALETTE_MAX_ROWS);
        let popup_area = centered_rect(
            area,
            COMMAND_PALETTE_WIDTH,
            (total_vis as u16).saturating_add(6),
        );
        let list_scroll = pick_abs.saturating_sub(COMMAND_PALETTE_MAX_ROWS / 2);
        let list_end = (list_scroll + COMMAND_PALETTE_MAX_ROWS).min(filtered.len());

        let mut popup_lines = vec![
            Line::from(vec![
                Span::styled(
                    "  Search ",
                    Style::default()
                        .fg(theme::MUTED)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if self.query.is_empty() {
                        "type to filter"
                    } else {
                        &self.query
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
                    PaletteRow::Entry { label, shortcut } => {
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
                            spans.push(Span::styled(format!("{shortcut:>pad$}"), shortcut_style));
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
}
