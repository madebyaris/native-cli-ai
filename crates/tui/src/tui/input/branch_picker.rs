//! Branch picker overlay.

use crate::tui::TuiCmd;
use crate::tui::composer::{branch_picker_enter_command, filtered_branch_indices};
use crate::tui::layout::centered_rect;
use crate::tui::state::TuiSessionState;
use crate::tui::theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear as ClearWidget, Paragraph, Wrap};

#[derive(Debug)]
pub enum BranchPickerKeyResult {
    Handled,
    Closed,
    Command(TuiCmd),
}

pub fn handle_branch_picker_key(
    state: &mut TuiSessionState,
    key: KeyEvent,
) -> BranchPickerKeyResult {
    let branches = state.branch_picker_branches().to_vec();
    let query = state.branch_picker_query().to_string();
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            state.close_branch_picker();
            BranchPickerKeyResult::Closed
        }
        (KeyCode::Up, _) => {
            if let Some(idx) = state.branch_picker_index_mut()
                && *idx > 0
            {
                *idx -= 1;
            }
            BranchPickerKeyResult::Handled
        }
        (KeyCode::Down, _) => {
            let filtered = filtered_branch_indices(&branches, &query);
            if let Some(idx) = state.branch_picker_index_mut() {
                *idx = (*idx + 1).min(filtered.len().saturating_sub(1));
            }
            BranchPickerKeyResult::Handled
        }
        (KeyCode::Enter, _) => {
            let index = state.branch_picker_index();
            let cmd = branch_picker_enter_command(&branches, &query, index);
            state.close_branch_picker();
            if let Some(cmd) = cmd {
                BranchPickerKeyResult::Command(cmd)
            } else {
                BranchPickerKeyResult::Closed
            }
        }
        (KeyCode::Backspace, _) => {
            if let Some(q) = state.branch_picker_query_mut() {
                q.pop();
            }
            let filtered = filtered_branch_indices(&branches, state.branch_picker_query());
            if let Some(idx) = state.branch_picker_index_mut() {
                *idx = (*idx).min(filtered.len().saturating_sub(1));
            }
            BranchPickerKeyResult::Handled
        }
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            if let Some(q) = state.branch_picker_query_mut() {
                q.push(c);
            }
            if let Some(idx) = state.branch_picker_index_mut() {
                *idx = 0;
            }
            BranchPickerKeyResult::Handled
        }
        _ => BranchPickerKeyResult::Handled,
    }
}

pub fn render_branch_picker(frame: &mut Frame, area: Rect, state: &TuiSessionState) {
    let branches = state.branch_picker_branches();
    let query = state.branch_picker_query();
    let filtered = filtered_branch_indices(branches, query);
    let popup_h = (filtered.len().min(12) as u16).saturating_add(6).max(8);
    let popup_area = centered_rect(area, 36, popup_h);
    let mut popup_lines = vec![
        Line::from(vec![
            Span::styled(
                " Branch ",
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if query.is_empty() {
                    String::new()
                } else {
                    format!(": {query}")
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
        let n_show = filtered.len().min(12);
        let list_scroll = state
            .branch_picker_index()
            .saturating_sub(n_show.saturating_sub(1))
            .min(filtered.len().saturating_sub(n_show));
        for (i, branch_idx) in filtered[list_scroll..list_scroll + n_show]
            .iter()
            .enumerate()
        {
            let filtered_idx = list_scroll + i;
            let branch = &branches[*branch_idx];
            let style = if filtered_idx == state.branch_picker_index() {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::USER)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT)
            };
            let mark = if branch.as_str() == state.current_branch {
                " *"
            } else {
                ""
            };
            popup_lines.push(Line::from(Span::styled(format!(" {branch}{mark}"), style)));
        }
    }
    popup_lines.push(Line::default());
    popup_lines.push(Line::from(Span::styled(
        " Enter switch/create · Esc cancel ",
        Style::default().fg(theme::MUTED),
    )));
    frame.render_widget(ClearWidget, popup_area);
    let popup = Paragraph::new(Text::from(popup_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::BORDER))
                .title(Span::styled(" branch ", Style::default().fg(theme::MUTED))),
        )
        .style(Style::default().bg(theme::SURFACE))
        .wrap(Wrap { trim: false });
    frame.render_widget(popup, popup_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn enter_emits_switch_command() {
        let mut st = TuiSessionState::new(
            "s".into(),
            "m".into(),
            "a".into(),
            "default".into(),
            PathBuf::from("/tmp"),
        );
        st.open_branch_picker(vec!["main".into()], "main");
        st.branch_picker_query_mut().unwrap().push_str("main");
        let result =
            handle_branch_picker_key(&mut st, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            result,
            BranchPickerKeyResult::Command(TuiCmd::SwitchBranch(_))
        ));
    }
}
