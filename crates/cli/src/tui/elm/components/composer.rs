//! Composer component — multi-line text input with cursor, history, slash commands, and @-mentions.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nca_core::skills::{SkillCatalog, SkillSource};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthChar;

use crate::file_mentions;
use crate::slash_commands::SLASH_COMMANDS;
use crate::tui::app::TuiCmd;

use super::super::msg::Msg;

// ── Theme (from shared module, re-exported via searchable_list) ───
use super::searchable_list::theme;

const COMPOSER_MAX_ROWS: usize = 5;
const SLASH_PANEL_MAX_ROWS: usize = 8;

// ── SlashEntry (duplicated from app.rs for self-containment) ───────
// In Phase 4 cleanup, we'll extract this to a shared location.

/// Entry for the slash panel: either a hardcoded command or a discovered skill.
#[derive(Clone, Debug)]
pub(crate) struct SlashEntry {
    display: String,
    command: String,
}

impl SlashEntry {
    fn command_str(&self) -> &str {
        &self.command
    }

    fn display_text(&self) -> &str {
        &self.display
    }
}

fn load_slash_entries(
    workspace_root: &Path,
    skill_dirs: &[PathBuf],
    plugin_commands: &[(String, Vec<String>)],
) -> Vec<SlashEntry> {
    let mut entries: Vec<SlashEntry> = SLASH_COMMANDS
        .iter()
        .map(|c| SlashEntry {
            display: c.to_string(),
            command: c.to_string(),
        })
        .collect();

    // Add discovered skills
    if let Ok(skills) = SkillCatalog::discover(workspace_root, skill_dirs) {
        for s in skills {
            let tag = match s.source {
                SkillSource::AgentsMd => " (AGENTS.md)",
                SkillSource::FileSystem => " (skill dir)",
            };
            let display = match s.description {
                Some(desc) => format!("/{:<20} — {}{}", s.command, desc, tag),
                None => format!("/{}{}", s.command, tag),
            };
            entries.push(SlashEntry {
                display,
                command: format!("/{}", s.command),
            });
        }
    }

    // Add plugin-contributed slash commands
    for (plugin_name, cmds) in plugin_commands {
        for cmd in cmds {
            entries.push(SlashEntry {
                display: format!("/{cmd} ({plugin_name})"),
                command: format!("/{cmd}"),
            });
        }
    }

    entries.sort_by_key(|a| a.command.to_lowercase());
    entries.dedup_by(|a, b| a.command.eq_ignore_ascii_case(&b.command));
    entries
}

fn filter_slash_entries<'a>(entries: &'a [SlashEntry], buffer: &str) -> Vec<&'a SlashEntry> {
    if !slash_panel_visible(buffer) {
        return Vec::new();
    }
    let needle = buffer.trim_start_matches('/').to_lowercase();
    entries
        .iter()
        .filter(|e| {
            e.command
                .trim_start_matches('/')
                .to_lowercase()
                .starts_with(&needle)
        })
        .collect()
}

fn slash_panel_visible(buffer: &str) -> bool {
    buffer.starts_with('/') && !buffer.contains(' ')
}

fn at_completion_active(buffer: &str, cursor_char_idx: usize) -> bool {
    if slash_panel_visible(buffer) {
        return false;
    }
    let b = cursor_byte_index(buffer, cursor_char_idx);
    file_mentions::at_token_before_cursor(buffer, b).is_some()
}

fn at_completion_matches(
    workspace_files: &[String],
    buffer: &str,
    cursor_char_idx: usize,
) -> Vec<String> {
    if !at_completion_active(buffer, cursor_char_idx) {
        return Vec::new();
    }
    let b = cursor_byte_index(buffer, cursor_char_idx);
    let Some((_, prefix)) = file_mentions::at_token_before_cursor(buffer, b) else {
        return Vec::new();
    };
    file_mentions::filter_paths_prefix(workspace_files, &prefix)
}

fn cursor_byte_index(line: &str, cursor_char_idx: usize) -> usize {
    line.char_indices()
        .nth(cursor_char_idx)
        .map(|(i, _)| i)
        .unwrap_or(line.len())
}

fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(0)
}

fn at_mention_char_ranges(buffer: &str) -> Vec<(usize, usize)> {
    file_mentions::parse_at_mentions(buffer)
        .into_iter()
        .map(|(start, end, _)| {
            let start_char = buffer[..start].chars().count();
            let end_char = buffer[..end].chars().count();
            (start_char, end_char)
        })
        .collect()
}

fn delete_completed_at_mention(buffer: &str, cursor_char_idx: usize) -> Option<(String, usize)> {
    // Find a completed @mention that ends just before cursor
    let mentions = at_mention_char_ranges(buffer);
    for (start_char, end_char) in mentions {
        if end_char == cursor_char_idx {
            let mut chars: Vec<char> = buffer.chars().collect();
            chars.drain(start_char..end_char);
            return Some((chars.into_iter().collect(), start_char));
        }
        // Also check end_char + 1 (mention + trailing space)
        if end_char + 1 == cursor_char_idx && buffer.chars().nth(end_char) == Some(' ') {
            let mut chars: Vec<char> = buffer.chars().collect();
            chars.drain(start_char..=end_char);
            return Some((chars.into_iter().collect(), start_char));
        }
    }
    None
}

fn apply_selected_at_completion(
    workspace_files: &[String],
    buffer: &str,
    cursor_char_idx: usize,
    at_menu_index: usize,
    append_space: bool,
) -> Option<(String, usize)> {
    let at_matches = at_completion_matches(workspace_files, buffer, cursor_char_idx);
    if at_matches.is_empty() || !at_completion_active(buffer, cursor_char_idx) {
        return None;
    }

    let pick = at_menu_index.min(at_matches.len().saturating_sub(1));
    let choice = at_matches.get(pick)?;
    let b = cursor_byte_index(buffer, cursor_char_idx);
    let (at_byte, _prefix) = file_mentions::at_token_before_cursor(buffer, b)?;
    let before = &buffer[..at_byte.saturating_add(1)];
    let after = &buffer[b..];
    let new_buf = format!("{before}{choice}{after}");
    let new_byte = at_byte + 1 + choice.len();
    let new_char = new_buf[..new_byte.min(new_buf.len())].chars().count();
    let (mut new_buf, mut new_cursor) = (new_buf, new_char);
    if append_space {
        let insert_at = cursor_byte_index(&new_buf, new_cursor);
        new_buf.insert(insert_at, ' ');
        new_cursor += 1;
    }
    Some((new_buf, new_cursor))
}

// ── Composer visual row (duplicated from app.rs) ─────────────────

struct ComposerVisualRow {
    prefix: &'static str,
    char_start: usize,
    char_end: usize,
}

fn wrap_char_counts(text: &str, max_width: usize) -> Vec<usize> {
    if max_width == 0 || text.is_empty() {
        return vec![text.chars().count()];
    }
    let mut rows = Vec::new();
    let mut width = 0usize;
    let mut count = 0usize;
    for ch in text.chars() {
        let w = char_width(ch);
        if width + w > max_width && count > 0 {
            rows.push(count);
            width = 0;
            count = 0;
        }
        width += w;
        count += 1;
    }
    rows.push(count);
    rows
}

fn build_composer_visual_rows(buffer: &str, max_cols: usize) -> Vec<ComposerVisualRow> {
    const PREFIX_COLS: usize = 2;
    let buf_cols = max_cols.saturating_sub(PREFIX_COLS).max(1);

    let mut rows = Vec::new();
    let mut char_pos = 0;
    let mut is_first = true;

    for segment in buffer.split('\n') {
        let counts = wrap_char_counts(segment, buf_cols);
        for &count in &counts {
            let prefix = if is_first { "❯ " } else { "  " };
            let start = char_pos;
            rows.push(ComposerVisualRow {
                prefix,
                char_start: start,
                char_end: start + count,
            });
            char_pos = start + count;
            is_first = false;
        }
        char_pos += 1; // +1 for the '\n' itself
    }

    if rows.is_empty() {
        rows.push(ComposerVisualRow {
            prefix: "❯ ",
            char_start: 0,
            char_end: 0,
        });
    }

    rows
}

fn cursor_visual_row_idx(rows: &[ComposerVisualRow], cursor_char_idx: usize) -> usize {
    for (i, row) in rows.iter().enumerate() {
        if cursor_char_idx >= row.char_start && cursor_char_idx <= row.char_end {
            return i;
        }
    }
    let mut best = 0;
    for (i, row) in rows.iter().enumerate() {
        if row.char_end <= cursor_char_idx {
            best = i;
        } else {
            break;
        }
    }
    best
}

fn composer_input_rows(buffer: &str, max_cols: usize) -> usize {
    build_composer_visual_rows(buffer, max_cols).len().max(1)
}

fn push_styled_run(
    spans: &mut Vec<Span<'static>>,
    text: &mut String,
    current_style: &mut Option<Style>,
    style: Style,
    ch: char,
) {
    if current_style.as_ref() != Some(&style) && !text.is_empty() {
        spans.push(Span::styled(
            std::mem::take(text),
            current_style.unwrap_or_default(),
        ));
    }
    *current_style = Some(style);
    text.push(ch);
}

fn style_composer_row(
    row: &ComposerVisualRow,
    buffer_chars: &[char],
    cursor_char_idx: usize,
    has_cursor: bool,
    is_scrolled: bool,
    mention_ranges: &[(usize, usize)],
) -> Line<'static> {
    let mut spans = Vec::new();

    let prefix_text = if is_scrolled { "… " } else { row.prefix };
    let prefix_style = if row.prefix == "❯ " {
        Style::default()
            .fg(theme::USER)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::TEXT)
    };
    spans.push(Span::styled(prefix_text.to_string(), prefix_style));

    let start = row.char_start.min(buffer_chars.len());
    let end = row.char_end.min(buffer_chars.len());
    let mut run = String::new();
    let mut run_style: Option<Style> = None;

    for idx in start..=end {
        let is_cursor_pos = has_cursor && idx == cursor_char_idx;
        let ch = buffer_chars.get(idx).copied().unwrap_or(' ');
        let in_mention =
            idx < buffer_chars.len() && mention_ranges.iter().any(|(s, e)| *s <= idx && idx < *e);

        let style = if is_cursor_pos {
            if in_mention {
                Style::default()
                    .bg(theme::USER)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .bg(theme::MUTED)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            }
        } else if in_mention {
            Style::default()
                .fg(theme::TEXT)
                .bg(theme::MENTION_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT)
        };

        push_styled_run(&mut spans, &mut run, &mut run_style, style, ch);

        if idx == buffer_chars.len() {
            break;
        }
    }

    if !run.is_empty() {
        spans.push(Span::styled(run, run_style.unwrap_or_default()));
    }

    Line::from(spans)
}

fn composer_text_lines(
    buffer: &str,
    cursor_char_idx: usize,
    max_cols: usize,
) -> Vec<Line<'static>> {
    let chars: Vec<char> = buffer.chars().collect();
    let cursor_char_idx = cursor_char_idx.min(chars.len());
    let mention_ranges = at_mention_char_ranges(buffer);

    let rows = build_composer_visual_rows(buffer, max_cols);
    let cursor_row = cursor_visual_row_idx(&rows, cursor_char_idx);

    let total = rows.len();
    let max_rows = COMPOSER_MAX_ROWS;
    let scroll_offset = if total <= max_rows {
        0
    } else {
        cursor_row.saturating_sub(max_rows - 1)
    };

    let mut result = Vec::new();
    for (local_idx, row) in rows.iter().enumerate().skip(scroll_offset).take(max_rows) {
        let has_cursor = local_idx == cursor_row;
        let is_scrolled = scroll_offset > 0 && local_idx == scroll_offset;
        let line = style_composer_row(
            row,
            &chars,
            cursor_char_idx,
            has_cursor,
            is_scrolled,
            &mention_ranges,
        );
        result.push(line);
    }

    result
}

fn composer_chrome_height(
    slash_entries: &[SlashEntry],
    workspace_files: &[String],
    buffer: &str,
    cursor_char_idx: usize,
) -> u16 {
    let slash_filtered = filter_slash_entries(slash_entries, buffer);
    let at_matches = at_completion_matches(workspace_files, buffer, cursor_char_idx);
    let slash_h = if slash_panel_visible(buffer) {
        (slash_filtered.len().min(SLASH_PANEL_MAX_ROWS) as u16).saturating_add(2)
    } else {
        0
    };
    let at_h = if !at_matches.is_empty() {
        (at_matches.len().min(SLASH_PANEL_MAX_ROWS) as u16).saturating_add(2)
    } else {
        0
    };
    slash_h.max(at_h)
}

// ── Composer State ────────────────────────────────────────────────

/// All mutable state owned by the composer.
#[derive(Debug, Default)]
pub(crate) struct ComposerState {
    /// Current text content.
    pub(crate) input_buffer: String,
    /// Cursor position in characters (not bytes).
    pub(crate) cursor_char_idx: usize,
    /// Command history (newest last).
    pub(crate) command_history: Vec<String>,
    /// Position when navigating history (len = below oldest).
    pub(crate) history_index: usize,
    /// Saved draft when entering history navigation.
    pub(crate) history_draft: String,
    /// Selected row in slash panel.
    pub(crate) slash_menu_index: usize,
    /// Selected row in @-mention panel.
    pub(crate) at_menu_index: usize,
    /// Workspace files for @-mention completion.
    pub(crate) workspace_files: Vec<String>,
    /// Slash command entries (commands + skills).
    pub(crate) slash_entries: Vec<SlashEntry>,
    // ── External state (set by NcaModel) ──
    pub(crate) active_approval: bool,
    pub(crate) active_question: bool,
    pub(crate) staged_image_count: usize,
}

impl ComposerState {
    pub(crate) fn load_workspace_files(&mut self, workspace_root: &Path) {
        self.workspace_files = file_mentions::discover_workspace_files(workspace_root);
    }

    pub(crate) fn load_slash_entries(
        &mut self,
        workspace_root: &Path,
        skill_dirs: &[PathBuf],
        plugin_commands: &[(String, Vec<String>)],
    ) {
        self.slash_entries = load_slash_entries(workspace_root, skill_dirs, plugin_commands);
    }

    // ── History management ──

    pub(crate) fn push_history(&mut self, line: &str) {
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        if self.command_history.last().map(|s| s.as_str()) == Some(&trimmed) {
            return;
        }
        self.command_history.push(trimmed);
        if self.command_history.len() > 200 {
            self.command_history
                .drain(..self.command_history.len() - 200);
        }
        self.history_index = self.command_history.len();
        self.history_draft.clear();
    }

    pub(crate) fn history_back(&mut self) -> bool {
        if self.command_history.is_empty() {
            return false;
        }
        let idx = self.history_index;
        if idx == self.command_history.len() {
            self.history_draft = self.input_buffer.clone();
        }
        if idx == 0 {
            return false;
        }
        self.history_index = idx - 1;
        self.input_buffer = self.command_history[self.history_index].clone();
        self.cursor_char_idx = self.input_buffer.chars().count();
        true
    }

    pub(crate) fn history_forward(&mut self) -> bool {
        if self.history_index >= self.command_history.len() {
            return false;
        }
        self.history_index += 1;
        if self.history_index == self.command_history.len() {
            self.input_buffer = std::mem::take(&mut self.history_draft);
        } else {
            self.input_buffer = self.command_history[self.history_index].clone();
        }
        self.cursor_char_idx = self.input_buffer.chars().count();
        true
    }

    pub(crate) fn history_reset(&mut self) {
        if self.history_index != self.command_history.len() {
            self.history_index = self.command_history.len();
            self.history_draft.clear();
        }
    }

    // ── Key event handling ──

    /// Process a key event. Returns `Some(Msg)` if a side-effect command should be emitted.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<Msg> {
        // ── Alt+Enter: insert newline ──
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::ALT) {
            self.history_reset();
            let idx = self.cursor_char_idx;
            let mut cs: Vec<char> = self.input_buffer.chars().collect();
            cs.insert(idx, '\n');
            self.input_buffer = cs.into_iter().collect();
            self.cursor_char_idx += 1;
            return None;
        }

        // ── Tab: slash completion (panel visible) or cycle agent (empty line) ──
        if key.code == KeyCode::Tab && key.modifiers == KeyModifiers::NONE {
            // Slash command panel: complete the selected command without executing it,
            // so the user can review/edit before pressing Enter to run.
            if slash_panel_visible(&self.input_buffer) {
                let filtered = filter_slash_entries(&self.slash_entries, &self.input_buffer);
                if !filtered.is_empty() {
                    let pick = self.slash_menu_index.min(filtered.len().saturating_sub(1));
                    self.input_buffer = filtered[pick].command.clone();
                    self.cursor_char_idx = self.input_buffer.chars().count();
                    self.slash_menu_index = 0;
                }
                return None;
            }
            // Empty input: cycle to the next agent profile (Build→Plan→Review→Fix→Test).
            if self.input_buffer.trim().is_empty() {
                return Some(Msg::Cmd(TuiCmd::CycleAgent));
            }
            return None;
        }

        // ── Enter: submit (with @-completion first) ──
        if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::NONE {
            if let Some((buf, cidx)) = apply_selected_at_completion(
                &self.workspace_files,
                &self.input_buffer,
                self.cursor_char_idx,
                self.at_menu_index,
                true,
            ) {
                self.input_buffer = buf;
                self.cursor_char_idx = cidx;
                return None;
            }
            // Apply selected slash command from panel
            if slash_panel_visible(&self.input_buffer) {
                let filtered = filter_slash_entries(&self.slash_entries, &self.input_buffer);
                if !filtered.is_empty() {
                    let pick = self.slash_menu_index.min(filtered.len().saturating_sub(1));
                    self.input_buffer = filtered[pick].command.clone();
                    self.cursor_char_idx = self.input_buffer.chars().count();
                }
            }
            let line = std::mem::take(&mut self.input_buffer);
            self.cursor_char_idx = 0;
            self.slash_menu_index = 0;
            self.push_history(&line);
            // When a question is active, route through the side channel
            // because run_turn is blocked on ask_question and won't poll cmd_tx.
            if self.active_question {
                return Some(Msg::QuestionSubmit(line));
            }
            // When an approval is active, route through the approval side channel
            // because run_turn is blocked waiting for the approval answer.
            if self.active_approval {
                return Some(Msg::ApprovalSubmit(line));
            }
            return Some(Msg::Cmd(TuiCmd::Submit(line)));
        }

        // ── Approval shortcuts (active only when an approval is pending) ──
        if self.active_approval {
            if key.code == KeyCode::Char('y') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Some(Msg::ApprovalQuickAnswer {
                    approved: true,
                    always_allow: false,
                });
            }
            if key.code == KeyCode::Char('u') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Some(Msg::ApprovalQuickAnswer {
                    approved: true,
                    always_allow: true,
                });
            }
        }

        // ── Ctrl+A / Home: move cursor to start ──
        if (key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL))
            || key.code == KeyCode::Home
        {
            self.cursor_char_idx = 0;
            return None;
        }

        // ── Ctrl+E: move cursor to end ──
        if key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.cursor_char_idx = self.input_buffer.chars().count();
            return None;
        }

        // ── End: move cursor to end (or scroll transcript — handled by NcaModel) ──
        if key.code == KeyCode::End {
            if !self.input_buffer.is_empty() {
                self.cursor_char_idx = self.input_buffer.chars().count();
            }
            // Empty buffer + End → transcript scroll to bottom. NcaModel will handle this.
            return None;
        }

        // ── Left ──
        if key.code == KeyCode::Left {
            self.cursor_char_idx = self.cursor_char_idx.saturating_sub(1);
            return None;
        }

        // ── Right ──
        if key.code == KeyCode::Right {
            let max = self.input_buffer.chars().count();
            self.cursor_char_idx = (self.cursor_char_idx + 1).min(max);
            return None;
        }

        // ── Up: @-completion navigate, slash navigate, or history ──
        if key.code == KeyCode::Up {
            let at_matches = at_completion_matches(
                &self.workspace_files,
                &self.input_buffer,
                self.cursor_char_idx,
            );
            if !at_matches.is_empty()
                && at_completion_active(&self.input_buffer, self.cursor_char_idx)
            {
                self.at_menu_index = self.at_menu_index.saturating_sub(1);
            } else {
                let slash_filtered = filter_slash_entries(&self.slash_entries, &self.input_buffer);
                if !slash_filtered.is_empty() && slash_panel_visible(&self.input_buffer) {
                    self.slash_menu_index = self.slash_menu_index.saturating_sub(1);
                } else {
                    self.history_back();
                }
            }
            return None;
        }

        // ── Down: @-completion navigate, slash navigate, or history ──
        if key.code == KeyCode::Down {
            let at_matches = at_completion_matches(
                &self.workspace_files,
                &self.input_buffer,
                self.cursor_char_idx,
            );
            if !at_matches.is_empty()
                && at_completion_active(&self.input_buffer, self.cursor_char_idx)
            {
                let n = at_matches.len();
                self.at_menu_index = (self.at_menu_index + 1) % n;
            } else {
                let slash_filtered = filter_slash_entries(&self.slash_entries, &self.input_buffer);
                if !slash_filtered.is_empty() && slash_panel_visible(&self.input_buffer) {
                    let n = slash_filtered.len();
                    self.slash_menu_index = (self.slash_menu_index + 1) % n;
                } else {
                    self.history_forward();
                }
            }
            return None;
        }

        // ── Backspace ──
        if key.code == KeyCode::Backspace && self.cursor_char_idx > 0 {
            if let Some((buf, cidx)) =
                delete_completed_at_mention(&self.input_buffer, self.cursor_char_idx)
            {
                self.input_buffer = buf;
                self.cursor_char_idx = cidx;
            } else {
                let idx = self.cursor_char_idx;
                let mut cs: Vec<char> = self.input_buffer.chars().collect();
                cs.remove(idx - 1);
                self.input_buffer = cs.into_iter().collect();
                self.cursor_char_idx -= 1;
            }
            if slash_panel_visible(&self.input_buffer) {
                let f = filter_slash_entries(&self.slash_entries, &self.input_buffer);
                if !f.is_empty() {
                    self.slash_menu_index = self.slash_menu_index.min(f.len().saturating_sub(1));
                } else {
                    self.slash_menu_index = 0;
                }
            }
            return None;
        }

        // ── Char insertion ──
        if let KeyCode::Char(c) = key.code
            && (key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT)
        {
            self.history_reset();
            let idx = self.cursor_char_idx;
            let mut cs: Vec<char> = self.input_buffer.chars().collect();
            cs.insert(idx, c);
            self.input_buffer = cs.into_iter().collect();
            self.cursor_char_idx += 1;
            if slash_panel_visible(&self.input_buffer) {
                let f = filter_slash_entries(&self.slash_entries, &self.input_buffer);
                if !f.is_empty() {
                    self.slash_menu_index = self.slash_menu_index.min(f.len().saturating_sub(1));
                }
            }
        }

        None
    }

    /// Handle pasted text.
    pub(crate) fn handle_paste(&mut self, text: &str) {
        self.history_reset();
        let idx = self.cursor_char_idx;
        let paste_chars: Vec<char> = text.chars().collect();
        let mut cs: Vec<char> = self.input_buffer.chars().collect();
        cs.splice(idx..idx, paste_chars);
        self.cursor_char_idx += text.chars().count();
        self.input_buffer = cs.into_iter().collect();
    }

    /// Compute how many rows the composer content needs (for layout).
    pub(crate) fn content_rows(&self, max_cols: usize) -> usize {
        composer_input_rows(&self.input_buffer, max_cols).min(COMPOSER_MAX_ROWS)
    }

    /// Compute the height of the slash/at-completion panel overlay (0 if none visible).
    pub(crate) fn chrome_height(&self) -> u16 {
        composer_chrome_height(
            &self.slash_entries,
            &self.workspace_files,
            &self.input_buffer,
            self.cursor_char_idx,
        )
    }
}

// ── Composer Component ──────────────────────────────────────────

/// The composer component — multi-line text input with history and completions.
pub(crate) struct Composer {
    state: ComposerState,
}

impl Composer {
    pub(crate) fn new() -> Self {
        Self {
            state: ComposerState::default(),
        }
    }

    pub(crate) fn state(&self) -> &ComposerState {
        &self.state
    }

    pub(crate) fn state_mut(&mut self) -> &mut ComposerState {
        &mut self.state
    }

    /// Handle a key event. Returns `Some(Msg)` for side-effect commands.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<Msg> {
        self.state.handle_key(key)
    }

    /// Handle pasted text.
    pub(crate) fn handle_paste(&mut self, text: &str) {
        self.state.handle_paste(text);
    }

    /// Render the composer and its overlays (slash panel, @-completion panel).
    pub(crate) fn render(&mut self, frame: &mut Frame, area: Rect) {
        let chrome_h = self.state.chrome_height();
        let composer_cols = area.width.saturating_sub(2) as usize; // border
        let input_rows = self.state.content_rows(composer_cols) as u16;
        let input_h = input_rows.saturating_add(1); // +1 for hint line
        let total_h = input_h.saturating_add(chrome_h).saturating_add(2); // +2 border

        let chunks = if chrome_h > 0 {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(input_h.saturating_add(2)), // input box + border
                    Constraint::Length(chrome_h),                  // slash/at panel
                ])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(total_h), Constraint::Min(0)])
                .split(area)
        };

        let inp_r = chunks[0];

        // ── Render slash/at-completion panel ──
        if chrome_h > 0 && chunks.len() > 1 {
            let sr = chunks[1];
            let slash_filtered =
                filter_slash_entries(&self.state.slash_entries, &self.state.input_buffer);
            let at_matches = at_completion_matches(
                &self.state.workspace_files,
                &self.state.input_buffer,
                self.state.cursor_char_idx,
            );

            if slash_panel_visible(&self.state.input_buffer) && !slash_filtered.is_empty() {
                self.render_slash_panel(frame, sr, &slash_filtered);
            } else if !at_matches.is_empty() {
                self.render_at_panel(frame, sr, &at_matches);
            }
        }

        // ── Render input box ──
        let mut input_lines = composer_text_lines(
            &self.state.input_buffer,
            self.state.cursor_char_idx,
            composer_cols,
        );

        // Hint line
        let hint = if self.state.active_approval {
            Line::from(Span::styled(
                "Approval: y/n · Ctrl+Y approve · Ctrl+U always allow",
                Style::default().fg(theme::ERROR),
            ))
        } else if self.state.active_question {
            Line::from(Span::styled(
                "Enter / 0 = suggested · 1–n = option · /auto-answer · End = transcript bottom",
                Style::default().fg(theme::WARN),
            ))
        } else {
            Line::from(Span::styled(
                "Enter send · Alt+Enter newline · Tab agent · Ctrl+V image · /image · Ctrl+P palette · Ctrl+Q exit",
                Style::default().fg(theme::MUTED),
            ))
        };

        let input_title = if self.state.active_approval {
            " approval "
        } else if self.state.active_question {
            " answer "
        } else {
            " message "
        };

        if self.state.staged_image_count > 0 {
            input_lines.push(Line::from(Span::styled(
                format!(
                    "  {} image(s) staged · Enter to send · /image clear",
                    self.state.staged_image_count
                ),
                Style::default().fg(theme::SUCCESS),
            )));
        }
        input_lines.push(hint);

        let input_block = Paragraph::new(Text::from(input_lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(input_title, Style::default().fg(theme::MUTED))),
            )
            .style(Style::default().bg(theme::SURFACE));

        frame.render_widget(input_block, inp_r);
    }

    fn render_slash_panel(&self, frame: &mut Frame, area: Rect, filtered: &[&SlashEntry]) {
        let n_show = filtered.len().min(SLASH_PANEL_MAX_ROWS);
        let max_scroll = filtered.len().saturating_sub(n_show);
        let list_scroll = self
            .state
            .slash_menu_index
            .saturating_sub(n_show.saturating_sub(1))
            .min(max_scroll);
        let mut lines: Vec<Line> = Vec::new();
        for (i, entry) in filtered[list_scroll..list_scroll + n_show]
            .iter()
            .enumerate()
        {
            let global = list_scroll + i;
            let st = if global == self.state.slash_menu_index {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::USER)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT)
            };
            lines.push(Line::from(Span::styled(
                entry.display_text().to_string(),
                st,
            )));
        }
        if filtered.len() > n_show {
            lines.push(Line::from(Span::styled(
                format!(
                    " ─ {}/{} · ↑↓",
                    self.state.slash_menu_index + 1,
                    filtered.len()
                ),
                Style::default().fg(theme::MUTED),
            )));
        }
        let w = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(
                        " commands (↑↓ select, Enter) ",
                        Style::default().fg(theme::MUTED),
                    )),
            )
            .style(Style::default().bg(theme::SURFACE));
        frame.render_widget(w, area);
    }

    fn render_at_panel(&self, frame: &mut Frame, area: Rect, matches: &[String]) {
        let n_show = matches.len().min(SLASH_PANEL_MAX_ROWS);
        let max_scroll = matches.len().saturating_sub(n_show);
        let pick = self
            .state
            .at_menu_index
            .min(matches.len().saturating_sub(1));
        let list_scroll = pick
            .saturating_sub(n_show.saturating_sub(1))
            .min(max_scroll);
        let mut lines: Vec<Line> = Vec::new();
        for (i, path) in matches[list_scroll..list_scroll + n_show]
            .iter()
            .enumerate()
        {
            let global = list_scroll + i;
            let st = if global == pick {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::USER)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT)
            };
            lines.push(Line::from(Span::styled(format!(" {path}"), st)));
        }
        if matches.len() > n_show {
            lines.push(Line::from(Span::styled(
                format!(" ─ {}/{} · ↑↓ Tab", pick + 1, matches.len()),
                Style::default().fg(theme::MUTED),
            )));
        }
        let w = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(
                        " files (@ mention) ",
                        Style::default().fg(theme::MUTED),
                    )),
            )
            .style(Style::default().bg(theme::SURFACE));
        frame.render_widget(w, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn state_with_slash_entries(entries: &[&str]) -> ComposerState {
        ComposerState {
            slash_entries: entries
                .iter()
                .map(|c| SlashEntry {
                    display: (*c).to_string(),
                    command: (*c).to_string(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn tab_on_empty_line_cycles_agent() {
        // WHY: an empty input is the user's signal to switch profiles; Tab must
        // emit the cycle command and must never insert a stray character.
        let mut s = ComposerState::default();
        assert!(s.input_buffer.is_empty());

        let msg = s.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert!(
            matches!(msg, Some(Msg::Cmd(TuiCmd::CycleAgent))),
            "expected CycleAgent on empty-line Tab, got {msg:?}"
        );
        assert_eq!(s.input_buffer, "", "Tab must not mutate an empty buffer");
    }

    #[test]
    fn tab_on_whitespace_only_cycles_agent() {
        // WHY: whitespace-only input is still "no command being typed", so it
        // should behave like an empty line and switch profiles.
        let mut s = ComposerState {
            input_buffer: "   ".into(),
            ..Default::default()
        };

        let msg = s.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert!(
            matches!(msg, Some(Msg::Cmd(TuiCmd::CycleAgent))),
            "expected CycleAgent on whitespace-only Tab, got {msg:?}"
        );
    }

    #[test]
    fn tab_completes_slash_command_without_submitting() {
        // WHY: slash completion must let the user review/edit before running —
        // Tab fills the buffer but must NOT emit Submit (Enter still runs it).
        let mut s = state_with_slash_entries(&["/help", "/clear"]);
        s.input_buffer = "/h".into();
        s.cursor_char_idx = s.input_buffer.chars().count();

        let msg = s.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert!(msg.is_none(), "Tab must not submit, got {msg:?}");
        assert_eq!(s.input_buffer, "/help");
        assert_eq!(s.cursor_char_idx, "/help".chars().count());
    }

    #[test]
    fn tab_on_plain_text_is_a_noop() {
        // WHY: Tab is only meaningful for slash completion or agent switching;
        // plain prose should be left untouched with no side effects.
        let mut s = ComposerState {
            input_buffer: "hello world".into(),
            ..Default::default()
        };
        s.cursor_char_idx = s.input_buffer.chars().count();

        let msg = s.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert!(msg.is_none());
        assert_eq!(s.input_buffer, "hello world");
    }
}
