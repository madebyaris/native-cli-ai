//! Branch picker popup.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::searchable_list::{ClearWidget, centered_rect, theme};

// ── State + Actions ────────────────────────────────────────────────

pub(crate) struct BranchPickerState {
    query: String,
    index: usize,
    branches: Vec<String>,
    current_branch: String,
}

pub(crate) enum BranchPickerAction {
    None,
    Close,
    SwitchBranch(String),
    CreateBranch(String),
}

// ── Helpers ──────────────────────────────────────────────────────

fn branch_filter_text(query: &str) -> &str {
    query.trim().strip_prefix('/').unwrap_or(query.trim())
}

fn filtered_branch_indices(branches: &[String], query: &str) -> Vec<usize> {
    let filter = branch_filter_text(query).to_ascii_lowercase();
    if filter.is_empty() {
        return (0..branches.len()).collect();
    }
    branches
        .iter()
        .enumerate()
        .filter(|(_, branch)| branch.to_ascii_lowercase().contains(&filter))
        .map(|(idx, _)| idx)
        .collect()
}

const BRANCH_PICKER_WIDTH: u16 = 36;
const BRANCH_PICKER_MAX_ROWS: usize = 12;

impl BranchPickerState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            index: 0,
            branches: Vec::new(),
            current_branch: String::new(),
        }
    }

    pub fn open(&mut self, branches: Vec<String>, current_branch: String) {
        self.branches = branches;
        self.current_branch = current_branch;
        self.query.clear();
        self.index = 0;
    }

    pub fn close(&mut self) {
        self.branches.clear();
        self.current_branch.clear();
        self.query.clear();
        self.index = 0;
    }

    pub fn is_open(&self) -> bool {
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> BranchPickerAction {
        let filtered = filtered_branch_indices(&self.branches, &self.query);
        let n = filtered.len();

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => BranchPickerAction::Close,
            (KeyCode::Up, _) if !filtered.is_empty() => {
                self.index = self.index.saturating_sub(1);
                BranchPickerAction::None
            }
            (KeyCode::Down, _) => {
                if n > 0 {
                    self.index = (self.index + 1).min(n - 1);
                }
                BranchPickerAction::None
            }
            (KeyCode::Enter, _) => {
                let raw_query = self.query.trim();
                let branch_name = branch_filter_text(raw_query).trim();

                // /name creates a new branch
                if raw_query.starts_with('/') && !branch_name.is_empty() {
                    return BranchPickerAction::CreateBranch(branch_name.to_string());
                }

                // Exact match by name
                if !branch_name.is_empty()
                    && let Some((idx, _)) = self
                        .branches
                        .iter()
                        .enumerate()
                        .find(|(_, branch)| branch.eq_ignore_ascii_case(branch_name))
                {
                    return BranchPickerAction::SwitchBranch(self.branches[idx].clone());
                }

                // Select from filtered list
                if let Some(&branch_idx) = filtered.get(self.index.min(n.saturating_sub(1))) {
                    BranchPickerAction::SwitchBranch(self.branches[branch_idx].clone())
                } else {
                    BranchPickerAction::None
                }
            }
            (KeyCode::Backspace, _) => {
                self.query.pop();
                let filtered = filtered_branch_indices(&self.branches, &self.query);
                self.index = self.index.min(filtered.len().saturating_sub(1));
                BranchPickerAction::None
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.query.push(c);
                self.index = 0;
                BranchPickerAction::None
            }
            _ => BranchPickerAction::None,
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let filtered = filtered_branch_indices(&self.branches, &self.query);

        let popup_h = (filtered.len().min(BRANCH_PICKER_MAX_ROWS) as u16)
            .saturating_add(6)
            .max(8);
        let popup_area = centered_rect(area, BRANCH_PICKER_WIDTH, popup_h);

        let mut popup_lines = vec![
            Line::from(vec![
                Span::styled(
                    " Branch ",
                    Style::default()
                        .fg(theme::MUTED)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if self.query.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", self.query)
                    },
                    Style::default().fg(theme::TEXT),
                ),
            ]),
            Line::default(),
        ];

        if filtered.is_empty() {
            popup_lines.push(Line::from(Span::styled(
                "  (no branches — type a name to create)",
                Style::default().fg(theme::MUTED),
            )));
        } else {
            let n_show = filtered.len().min(BRANCH_PICKER_MAX_ROWS);
            let list_scroll = self
                .index
                .saturating_sub(n_show.saturating_sub(1))
                .min(filtered.len().saturating_sub(n_show));
            for (i, &branch_idx) in filtered[list_scroll..list_scroll + n_show]
                .iter()
                .enumerate()
            {
                let filtered_idx = list_scroll + i;
                let branch = &self.branches[branch_idx];
                let style = if filtered_idx == self.index {
                    Style::default()
                        .fg(Color::Black)
                        .bg(theme::USER)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::TEXT)
                };
                let mark = if branch.as_str() == self.current_branch {
                    " *"
                } else {
                    ""
                };
                popup_lines.push(Line::from(Span::styled(format!(" {branch}{mark}"), style)));
            }
        }

        popup_lines.push(Line::default());
        popup_lines.push(Line::from(Span::styled(
            " Enter switch  /name new  Esc close",
            Style::default().fg(theme::MUTED),
        )));

        frame.render_widget(ClearWidget, popup_area);
        let popup = Paragraph::new(Text::from(popup_lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(
                        " git branch ",
                        Style::default().fg(theme::MUTED),
                    )),
            )
            .style(Style::default().bg(theme::SURFACE))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, popup_area);
    }
}
