//! Full-screen session TUI: transcript, streaming assistant, composer.

use std::path::{Path, PathBuf};

use crate::file_mentions;
use crate::slash_commands::SLASH_COMMANDS;
use crate::tui::connect_modal::{
    ConnectRow, build_connect_rows, clamp_selection, provider_at_selection,
    row_index_for_selection, selectable_row_indices,
};
use crate::tui::state::{
    ApprovalRequest, DisplayBlock, ModelPickerAction, ModelPickerEntry, TuiSessionState,
};
use crossterm::{
    cursor::{Hide, MoveToColumn, Show},
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind, poll, read,
    },
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use nca_common::config::ProviderKind;
use nca_common::event::{BusyState, QuestionSelection};
use nca_core::approval::suggest_allow_pattern;
use nca_core::skills::{SkillCatalog, SkillSource};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear as ClearWidget, Paragraph, Wrap},
};
use std::io::{Stdout, stdout};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// Message from TUI to the approval dispatch task.
#[derive(Debug)]
pub enum ApprovalAnswer {
    Verdict { call_id: String, approved: bool },
    AllowPattern { call_id: String, pattern: String },
}

/// Per flattened transcript line: click selects this answer (same indices as `transcript_lines`).
type LineAnswerHit = Option<QuestionSelection>;

#[derive(Debug)]
pub enum TuiCmd {
    Submit(String),
    /// Answer for the current `ask_question` (from question mode or `/auto-answer`).
    QuestionAnswer(nca_common::event::QuestionSelection),
    CycleAgent,
    CancelTurn,
    Exit,
    /// Open the branch picker popup.
    OpenBranchPicker,
    /// Switch to the given branch name.
    SwitchBranch(String),
    /// Create a new branch with the given name and switch to it.
    CreateBranch(String),
    /// Apply workspace default provider (from TUI picker).
    ApplyDefaultProvider(ProviderKind),
    /// Open API key modal for provider; bool indicates whether to connect after save/confirm.
    PromptApiKey(ProviderKind, bool),
    /// Apply a model name (from the model picker).
    ApplyModel(String),
    /// Switch provider (from the model picker).
    ApplyModelProvider(ProviderKind),
    /// Apply permission mode (from the permission picker).
    ApplyPermission(usize),
    /// Switch agent profile (from the agent picker).
    SwitchAgent(usize),
    /// Open external editor via leader key.
    OpenEditor,
    /// Start a new session.
    NewSession,
    /// Run compact.
    RunCompact,
    /// Open model picker (triggered by leader key or command palette).
    OpenModelPicker,
    /// Open status info modal.
    OpenStatus,
    /// Open help info modal.
    OpenHelp,
    /// Open agent picker.
    OpenAgentPicker,
    /// Open permission picker (reserved for future shortcut).
    #[allow(dead_code)]
    OpenPermissionPicker,
    /// Open sessions picker/info.
    OpenSessions,
    /// Cycle to the next recent model (F2 forward, Shift+F2 backward).
    CycleModel(bool),
    /// Validate an API key for onboarding (provider, api_key).
    /// The repl handler looks up base_url from config.
    ValidateApiKey(ProviderKind, String),
    /// Mark onboarding as complete and persist the flag.
    #[allow(dead_code)]
    CompleteOnboarding,
    /// Resume a different session by ID.
    ResumeSession(String),
}

mod theme {
    use ratatui::style::Color;

    pub const BG: Color = Color::Rgb(22, 22, 28);
    pub const SURFACE: Color = Color::Rgb(32, 32, 42);
    pub const BORDER: Color = Color::Rgb(55, 55, 70);
    pub const MENTION_BG: Color = Color::Rgb(48, 62, 94);

    pub const USER: Color = Color::Rgb(56, 189, 248);
    pub const ASSISTANT: Color = Color::Rgb(167, 139, 250);
    pub const TOOL: Color = Color::Rgb(94, 234, 212);
    pub const MUTED: Color = Color::Rgb(120, 120, 140);
    pub const TEXT: Color = Color::Rgb(230, 230, 240);
    pub const SUCCESS: Color = Color::Rgb(74, 222, 128);
    pub const ERROR: Color = Color::Rgb(248, 113, 113);
    pub const WARN: Color = Color::Rgb(251, 191, 36);
}

const SLASH_PANEL_MAX_ROWS: usize = 8;
const MOUSE_SCROLL_LINES: usize = 3;
const SIDEBAR_WIDTH: u16 = 32;
const SIDEBAR_MIN_TOTAL_WIDTH: u16 = 110;
const COMMAND_PALETTE_WIDTH: u16 = 48;
const COMMAND_PALETTE_MAX_ROWS: usize = 10;

fn slash_panel_visible(buffer: &str) -> bool {
    buffer.starts_with('/') && !buffer.contains(' ')
}

fn cursor_byte_index(line: &str, cursor_char_idx: usize) -> usize {
    line.char_indices()
        .nth(cursor_char_idx)
        .map(|(i, _)| i)
        .unwrap_or(line.len())
}

fn at_panel_height(n: usize) -> u16 {
    if n == 0 {
        return 0;
    }
    (n.min(SLASH_PANEL_MAX_ROWS) as u16).saturating_add(2)
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

fn composer_chrome_height(
    slash_entries: &[SlashEntry],
    workspace_files: &[String],
    buffer: &str,
    cursor_char_idx: usize,
) -> u16 {
    let slash_filtered = filter_slash_entries(slash_entries, buffer);
    let at_matches = at_completion_matches(workspace_files, buffer, cursor_char_idx);
    let slash_h = if slash_panel_visible(buffer) {
        slash_panel_height(slash_filtered.len())
    } else {
        0
    };
    let at_h = if !at_matches.is_empty() {
        at_panel_height(at_matches.len())
    } else {
        0
    };
    slash_h.max(at_h)
}

/// Replace `@prefix` before cursor with `@choice` (relative path).
fn apply_at_completion(buffer: &str, cursor_char_idx: usize, choice: &str) -> (String, usize) {
    let b = cursor_byte_index(buffer, cursor_char_idx);
    let Some((at_byte, _prefix)) = file_mentions::at_token_before_cursor(buffer, b) else {
        return (buffer.to_string(), cursor_char_idx);
    };
    let before = &buffer[..at_byte.saturating_add(1)];
    let after = &buffer[b..];
    let new_buf = format!("{before}{choice}{after}");
    let new_byte = at_byte + 1 + choice.len();
    let new_char = new_buf[..new_byte.min(new_buf.len())].chars().count();
    (new_buf, new_char)
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
    let (mut new_buf, mut new_cursor_char_idx) =
        apply_at_completion(buffer, cursor_char_idx, choice);

    if append_space {
        let insert_at = cursor_byte_index(&new_buf, new_cursor_char_idx);
        new_buf.insert(insert_at, ' ');
        new_cursor_char_idx += 1;
    }

    Some((new_buf, new_cursor_char_idx))
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

fn completed_at_mention_range_before_cursor(
    buffer: &str,
    cursor_char_idx: usize,
) -> Option<(usize, usize)> {
    let chars: Vec<char> = buffer.chars().collect();
    for (start_char, end_char) in at_mention_char_ranges(buffer) {
        if end_char == cursor_char_idx {
            return Some((start_char, end_char));
        }
        if end_char < chars.len()
            && end_char + 1 == cursor_char_idx
            && chars.get(end_char) == Some(&' ')
        {
            return Some((start_char, end_char + 1));
        }
    }
    None
}

fn remove_char_range(buffer: &str, start_char_idx: usize, end_char_idx: usize) -> String {
    let mut chars: Vec<char> = buffer.chars().collect();
    chars.drain(start_char_idx..end_char_idx);
    chars.into_iter().collect()
}

fn delete_completed_at_mention(buffer: &str, cursor_char_idx: usize) -> Option<(String, usize)> {
    let (start_char, end_char) = completed_at_mention_range_before_cursor(buffer, cursor_char_idx)?;
    Some((remove_char_range(buffer, start_char, end_char), start_char))
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

/// Display width of a character (CJK = 2, ASCII = 1, zero-width = 0).
fn char_width(ch: char) -> usize {
    unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
}

/// Compute the visible character window so the cursor stays within `max_cols`.
/// Returns `(start_char_idx, end_char_idx)` – half-open range of chars to render.
fn input_visible_window(buffer: &str, cursor_char_idx: usize, max_cols: usize) -> (usize, usize) {
    let chars: Vec<char> = buffer.chars().collect();
    let n = chars.len();
    if n == 0 || max_cols == 0 {
        return (0, 0);
    }
    // Try fitting from the beginning first.
    let mut width = 0usize;
    let mut end = 0usize;
    for (i, &ch) in chars.iter().enumerate() {
        let w = char_width(ch);
        if width + w > max_cols {
            break;
        }
        width += w;
        end = i + 1;
    }
    if end == n || cursor_char_idx < end {
        // Everything fits or cursor is within the visible window.
        return (0, end);
    }
    // Cursor is beyond the window – slide the window so cursor is visible.
    // Walk backwards from cursor to find the longest prefix that fits.
    let mut width = char_width(chars.get(cursor_char_idx).copied().unwrap_or(' '));
    let mut start = cursor_char_idx;
    while start > 0 && width + char_width(chars[start - 1]) <= max_cols {
        start -= 1;
        width += char_width(chars[start]);
    }
    // end: walk forward from cursor to fill remaining space.
    let mut end = cursor_char_idx + 1;
    while end < n && width + char_width(chars[end]) <= max_cols {
        width += char_width(chars[end]);
        end += 1;
    }
    (start, end)
}

fn composer_line(buffer: &str, cursor_char_idx: usize, max_cols: usize) -> Line<'static> {
    // Prompt "❯ " or "… " occupies 2 display columns (1 symbol + 1 space).
    const PROMPT_COLS: usize = 2;
    let buf_cols = max_cols.saturating_sub(PROMPT_COLS);
    let (win_start, win_end) = input_visible_window(buffer, cursor_char_idx, buf_cols);
    let prompt = if win_start > 0 {
        Span::styled("… ", Style::default().fg(theme::MUTED))
    } else {
        Span::styled("❯ ", Style::default().fg(theme::USER).bold())
    };
    let chars: Vec<char> = buffer.chars().collect();
    let mention_ranges = at_mention_char_ranges(buffer);
    let cursor_char_idx = cursor_char_idx.min(chars.len());
    let mut spans = vec![prompt];
    let mut run = String::new();
    let mut run_style: Option<Style> = None;

    for idx in win_start..=win_end {
        if idx == cursor_char_idx {
            let cursor_char = chars.get(idx).copied().unwrap_or(' ');
            let in_mention = idx < chars.len()
                && mention_ranges
                    .iter()
                    .any(|(start, end)| *start <= idx && idx < *end);
            let cursor_style = if in_mention {
                Style::default()
                    .bg(theme::USER)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .bg(theme::MUTED)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            };
            push_styled_run(
                &mut spans,
                &mut run,
                &mut run_style,
                cursor_style,
                cursor_char,
            );
            if idx == chars.len() {
                break;
            }
            continue;
        }

        let Some(ch) = chars.get(idx).copied() else {
            break;
        };
        let in_mention = mention_ranges
            .iter()
            .any(|(start, end)| *start <= idx && idx < *end);
        let style = if in_mention {
            Style::default()
                .fg(theme::TEXT)
                .bg(theme::MENTION_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT)
        };
        push_styled_run(&mut spans, &mut run, &mut run_style, style, ch);
    }

    if !run.is_empty() {
        spans.push(Span::styled(run, run_style.unwrap_or_default()));
    }

    Line::from(spans)
}

/// Entry for the slash panel: either a hardcoded command or a discovered skill.
#[derive(Clone)]
pub enum SlashEntry {
    Command(&'static str),
    Skill {
        command: String,
        description: Option<String>,
        source: SkillSource,
    },
}

impl SlashEntry {
    fn command_str(&self) -> String {
        match self {
            SlashEntry::Command(s) => s.to_string(),
            SlashEntry::Skill { command, .. } => format!("/{command}"),
        }
    }

    fn display_text(&self) -> String {
        match self {
            SlashEntry::Command(s) => s.to_string(),
            SlashEntry::Skill {
                command,
                description,
                source,
            } => {
                let tag = match source {
                    SkillSource::AgentsMd => " (AGENTS.md)",
                    SkillSource::FileSystem => " (skill dir)",
                };
                match description {
                    Some(desc) => format!("/{command:<20} — {desc}{tag}"),
                    None => format!("/{command}{tag}"),
                }
            }
        }
    }
}

/// Collect skills from SkillCatalog for slash panel display.
fn collect_skill_entries(workspace_root: &Path, skill_dirs: &[PathBuf]) -> Vec<SlashEntry> {
    match SkillCatalog::discover(workspace_root, skill_dirs) {
        Ok(skills) => skills
            .into_iter()
            .map(|s| SlashEntry::Skill {
                command: s.command,
                description: s.description,
                source: s.source,
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Load all slash-commands: hardcoded commands + discovered skills.
fn load_slash_entries(workspace_root: &Path, skill_dirs: &[PathBuf]) -> Vec<SlashEntry> {
    let mut entries: Vec<SlashEntry> = SLASH_COMMANDS
        .iter()
        .map(|c| SlashEntry::Command(c))
        .collect();

    // Add discovered skills
    entries.extend(collect_skill_entries(workspace_root, skill_dirs));

    // Sort by command name
    entries.sort_by(|a, b| {
        a.command_str()
            .to_lowercase()
            .cmp(&b.command_str().to_lowercase())
    });
    entries.dedup_by(|a, b| a.command_str().eq_ignore_ascii_case(&b.command_str()));
    entries
}

/// Filter slash entries by buffer prefix.
fn filter_slash_entries<'a>(entries: &'a [SlashEntry], buffer: &str) -> Vec<&'a SlashEntry> {
    if !slash_panel_visible(buffer) {
        return Vec::new();
    }
    let needle = buffer.trim_start_matches('/').to_lowercase();
    entries
        .iter()
        .filter(|e| {
            e.command_str()
                .trim_start_matches('/')
                .to_lowercase()
                .starts_with(&needle)
        })
        .collect()
}

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

fn branch_picker_enter_command(
    branches: &[String],
    query: &str,
    selected_filtered_idx: usize,
) -> Option<TuiCmd> {
    let raw_query = query.trim();
    let branch_name = branch_filter_text(raw_query).trim();
    let filtered = filtered_branch_indices(branches, raw_query);

    if raw_query.starts_with('/') {
        return (!branch_name.is_empty()).then(|| TuiCmd::CreateBranch(branch_name.to_string()));
    }

    if !branch_name.is_empty()
        && let Some((idx, _)) = branches
            .iter()
            .enumerate()
            .find(|(_, branch)| branch.eq_ignore_ascii_case(branch_name))
    {
        return Some(TuiCmd::SwitchBranch(branches[idx].clone()));
    }

    filtered
        .get(selected_filtered_idx)
        .copied()
        .map(|idx| TuiCmd::SwitchBranch(branches[idx].clone()))
}

/// A row in the categorized command palette.
#[derive(Clone)]
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
        _ => "/help",
    }
}

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
                    || palette_command_for_label(label).contains(&needle)
                {
                    if let Some(s) = pending_section.take() {
                        result.push(s);
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

fn slash_panel_height(filtered_len: usize) -> u16 {
    if filtered_len == 0 {
        return 0;
    }
    let rows = filtered_len.min(SLASH_PANEL_MAX_ROWS);
    let footer = if filtered_len > SLASH_PANEL_MAX_ROWS {
        1
    } else {
        0
    };
    // borders (2) + command rows + optional footer
    (rows as u16)
        .saturating_add(footer)
        .saturating_add(2)
        .min(14)
}

fn layout_chunks(area: Rect, slash_h: u16) -> (Rect, Rect, Option<Rect>, Rect) {
    if slash_h > 0 {
        let c = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(4),
                Constraint::Length(2),
                Constraint::Length(slash_h),
                Constraint::Length(3),
            ])
            .split(area);
        (c[0], c[1], Some(c[2]), c[3])
    } else {
        let c = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(4),
                Constraint::Length(2),
                Constraint::Length(3),
            ])
            .split(area);
        (c[0], c[1], None, c[2])
    }
}

fn sidebar_fit(s: &str, max_chars: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max_chars {
        t.to_string()
    } else {
        format!(
            "{}…",
            t.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn layout_with_sidebar(area: Rect) -> (Rect, Option<Rect>) {
    if area.width < SIDEBAR_MIN_TOTAL_WIDTH {
        return (area, None);
    }
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(60), Constraint::Length(SIDEBAR_WIDTH)])
        .split(area);
    (chunks[0], Some(chunks[1]))
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let popup_w = width
        .min(area.width.saturating_sub(2).max(20))
        .min(area.width);
    let popup_h = height
        .min(area.height.saturating_sub(2).max(6))
        .min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(popup_w) / 2,
        area.y + area.height.saturating_sub(popup_h) / 2,
        popup_w,
        popup_h,
    )
}

/// Matches `PermissionMode` as stored via `format!("{:?}", mode)` (e.g. `BypassPermissions`).
fn toolbar_permission_is_bypass(mode: &str) -> bool {
    mode.contains("BypassPermissions")
}

fn escape_cancels_active_turn(state: &TuiSessionState) -> bool {
    matches!(
        state.current_busy_state,
        BusyState::Thinking | BusyState::Streaming | BusyState::ToolRunning
    )
}

fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

/// Run a git command synchronously and return stdout.
fn git_run(args: &[&str], cwd: Option<&Path>) -> Option<String> {
    let cwd = cwd?;
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Get the current git branch name for `workspace`.
pub fn git_current_branch(workspace: &Path) -> Option<String> {
    git_run(&["rev-parse", "--abbrev-ref", "HEAD"], Some(workspace))
}

/// List local git branches for `workspace`. Current branch is marked with `*`.
pub fn git_list_branches(workspace: &Path) -> Vec<String> {
    git_run(&["branch", "--no-color"], Some(workspace))
        .map(|out| {
            out.lines()
                .map(|l| {
                    l.trim_start_matches("* ")
                        .trim_start_matches("+ ")
                        .trim()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Create a new branch `name` and check it out in `workspace`.
pub fn git_create_branch(workspace: &Path, name: &str) -> bool {
    git_run(&["checkout", "-b", name], Some(workspace)).is_some()
}

/// Switch to an existing branch `name` in `workspace`.
pub fn git_switch_branch(workspace: &Path, name: &str) -> bool {
    git_run(&["checkout", name], Some(workspace)).is_some()
}

pub fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().map_err(|e| anyhow::anyhow!("enable_raw_mode: {e}"))?;
    let res: anyhow::Result<Terminal<CrosstermBackend<Stdout>>> = (|| {
        let mut out = stdout();
        execute!(out, EnterAlternateScreen)?;
        execute!(out, EnableMouseCapture)?;
        execute!(out, Hide)?;
        execute!(out, Clear(ClearType::All))?;
        Ok(Terminal::new(CrosstermBackend::new(out))?)
    })();
    if res.is_err() {
        let _ = disable_raw_mode();
    }
    res
}

pub fn restore_terminal() {
    let mut out = stdout();
    let _ = execute!(out, Show);
    let _ = execute!(out, DisableMouseCapture);
    let _ = execute!(out, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

#[inline]
fn push_transcript_line(
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<LineAnswerHit>,
    line: Line<'static>,
    hit: LineAnswerHit,
) {
    lines.push(line);
    hits.push(hit);
}

/// Extract plain text from a range of `Line` items (used for clipboard copy).
/// Takes column-aware selection: `((start_line, start_col), (end_line, end_col))`.
fn plain_text_from_lines(
    lines: &[Line<'_>],
    sel_start: (usize, usize),
    sel_end: (usize, usize),
) -> String {
    let (sl, sc) = sel_start;
    let (el, ec) = sel_end;
    let s = sl.min(lines.len());
    let e = (el + 1).min(lines.len());
    let mut out = String::new();
    for (idx, line) in lines[s..e].iter().enumerate() {
        let global_line = s + idx;
        let full_text: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
        if global_line == sl && global_line == el {
            // Single line: extract substring by column range.
            let start_col = sc.min(ec);
            let end_col = sc.max(ec);
            let truncated = truncate_by_columns(&full_text, end_col);
            let remaining = truncate_by_columns_skip(&truncated, start_col);
            out.push_str(&remaining);
        } else if global_line == sl {
            // First line: skip columns before start_col.
            out.push_str(&truncate_by_columns_skip(&full_text, sc));
        } else if global_line == el {
            // Last line: truncate at end_col.
            out.push_str(&truncate_by_columns(&full_text, ec));
        } else {
            out.push_str(&full_text);
        }
        out.push('\n');
    }
    out
}

/// Keep only the first `max_cols` display columns of text.
fn truncate_by_columns(text: &str, max_cols: usize) -> String {
    let mut col = 0usize;
    let mut result = String::new();
    for c in text.chars() {
        if col >= max_cols {
            break;
        }
        let w = if c == '\t' {
            4
        } else {
            unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
        };
        result.push(c);
        col += w;
    }
    result
}

/// Skip the first `skip_cols` display columns of text, return the rest.
fn truncate_by_columns_skip(text: &str, skip_cols: usize) -> String {
    let mut col = 0usize;
    let mut skipping = true;
    let mut result = String::new();
    for c in text.chars() {
        if skipping {
            let w = if c == '\t' {
                4
            } else {
                unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
            };
            col += w;
            if col > skip_cols {
                result.push(c);
                skipping = false;
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Build scrollable transcript lines + optional mouse/click targets per line.
fn transcript_lines_and_hits(
    state: &TuiSessionState,
    width: u16,
) -> (Vec<Line<'static>>, Vec<LineAnswerHit>) {
    let w = width.max(20) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut hits: Vec<LineAnswerHit> = Vec::new();

    for block in &state.blocks {
        match block {
            DisplayBlock::User(content) => {
                push_transcript_line(
                    &mut lines,
                    &mut hits,
                    Line::from(vec![Span::styled(
                        " YOU ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(theme::USER)
                            .add_modifier(Modifier::BOLD),
                    )]),
                    None,
                );
                push_transcript_line(&mut lines, &mut hits, Line::default(), None);
                for text_line in wrap_text(content, w) {
                    push_transcript_line(
                        &mut lines,
                        &mut hits,
                        Line::from(Span::styled(text_line, Style::default().fg(theme::TEXT))),
                        None,
                    );
                }
                push_transcript_line(&mut lines, &mut hits, Line::default(), None);
            }
            DisplayBlock::Assistant(content) => {
                push_transcript_line(
                    &mut lines,
                    &mut hits,
                    Line::from(vec![Span::styled(
                        " nca ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(theme::ASSISTANT)
                            .add_modifier(Modifier::BOLD),
                    )]),
                    None,
                );
                push_transcript_line(&mut lines, &mut hits, Line::default(), None);
                for text_line in wrap_text(content, w) {
                    push_transcript_line(&mut lines, &mut hits, parse_md_line(&text_line), None);
                }
                push_transcript_line(&mut lines, &mut hits, Line::default(), None);
            }
            DisplayBlock::ToolRunning { name, .. } => {
                push_transcript_line(
                    &mut lines,
                    &mut hits,
                    Line::from(vec![
                        Span::styled(" ⚡ ", Style::default().fg(theme::TOOL)),
                        Span::styled(
                            format!("{name} "),
                            Style::default()
                                .fg(theme::TOOL)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled("…", Style::default().fg(theme::MUTED)),
                    ]),
                    None,
                );
            }
            DisplayBlock::ApprovalPending(req) => {
                render_approval_block(&mut lines, &mut hits, req, w);
            }
            DisplayBlock::ApprovalResolved { tool, approved } => {
                let (label, style) = if *approved {
                    (
                        " approved ",
                        Style::default().fg(Color::Black).bg(theme::SUCCESS),
                    )
                } else {
                    (
                        " denied ",
                        Style::default().fg(Color::Black).bg(theme::ERROR),
                    )
                };
                push_transcript_line(
                    &mut lines,
                    &mut hits,
                    Line::from(vec![
                        Span::styled(label, style.add_modifier(Modifier::BOLD)),
                        Span::styled(format!(" {tool}"), Style::default().fg(theme::TEXT)),
                    ]),
                    None,
                );
                push_transcript_line(&mut lines, &mut hits, Line::default(), None);
            }
            DisplayBlock::ToolDone { name, ok, detail } => {
                let (icon, st) = if *ok {
                    ("✓", Style::default().fg(theme::SUCCESS))
                } else {
                    ("✗", Style::default().fg(theme::ERROR))
                };
                push_transcript_line(
                    &mut lines,
                    &mut hits,
                    Line::from(vec![
                        Span::styled(format!(" {icon} "), st),
                        Span::styled(
                            name.to_string(),
                            Style::default()
                                .fg(theme::TOOL)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" — {}", truncate_chars(detail, 100)),
                            Style::default().fg(theme::MUTED),
                        ),
                    ]),
                    None,
                );
            }
            DisplayBlock::System(s) => {
                push_transcript_line(
                    &mut lines,
                    &mut hits,
                    Line::from(Span::styled(
                        format!(" ‣ {s}"),
                        Style::default().fg(theme::WARN),
                    )),
                    None,
                );
            }
            DisplayBlock::Question(q) => {
                push_transcript_line(
                    &mut lines,
                    &mut hits,
                    Line::from(vec![
                        Span::styled(
                            " ? ",
                            Style::default().fg(Color::Black).bg(theme::WARN).bold(),
                        ),
                        Span::styled(
                            " question ",
                            Style::default()
                                .fg(theme::WARN)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    None,
                );
                push_transcript_line(&mut lines, &mut hits, Line::default(), None);
                for text_line in wrap_text(&q.prompt, w) {
                    push_transcript_line(
                        &mut lines,
                        &mut hits,
                        Line::from(Span::styled(text_line, Style::default().fg(theme::TEXT))),
                        None,
                    );
                }
                // When the modal is open, skip inline options — the popup handles selection.
                if !state.question_modal_open {
                    push_transcript_line(
                        &mut lines,
                        &mut hits,
                        Line::from(vec![
                            Span::styled(
                                format!("  [0] suggested: {} ", q.suggested_answer),
                                Style::default()
                                    .fg(theme::SUCCESS)
                                    .add_modifier(Modifier::UNDERLINED),
                            ),
                            Span::styled("(click)", Style::default().fg(theme::MUTED)),
                        ]),
                        Some(QuestionSelection::Suggested),
                    );
                    for (i, o) in q.options.iter().enumerate() {
                        push_transcript_line(
                            &mut lines,
                            &mut hits,
                            Line::from(vec![
                                Span::styled(
                                    format!("  [{}] ({}) {} ", i + 1, o.id, o.label),
                                    Style::default()
                                        .fg(theme::TEXT)
                                        .add_modifier(Modifier::UNDERLINED),
                                ),
                                Span::styled("(click)", Style::default().fg(theme::MUTED)),
                            ]),
                            Some(QuestionSelection::Option {
                                option_id: o.id.clone(),
                            }),
                        );
                    }
                    if q.allow_custom {
                        push_transcript_line(
                            &mut lines,
                            &mut hits,
                            Line::from(Span::styled(
                                "  [c] type your own answer below, then Enter",
                                Style::default().fg(theme::MUTED),
                            )),
                            None,
                        );
                    }
                    push_transcript_line(
                        &mut lines,
                        &mut hits,
                        Line::from(Span::styled(
                            "  Tip: /auto-answer or Enter on empty = suggested · click an option above",
                            Style::default().fg(theme::MUTED),
                        )),
                        None,
                    );
                }
                push_transcript_line(&mut lines, &mut hits, Line::default(), None);
            }
            DisplayBlock::ErrorLine(s) => {
                push_transcript_line(
                    &mut lines,
                    &mut hits,
                    Line::from(Span::styled(
                        format!(" ✗ {s}"),
                        Style::default().fg(theme::ERROR),
                    )),
                    None,
                );
            }
        }
    }

    if let Some(stream) = &state.streaming_assistant
        && !stream.is_empty()
    {
        push_transcript_line(
            &mut lines,
            &mut hits,
            Line::from(vec![
                Span::styled(
                    " nca ",
                    Style::default().fg(Color::Black).bg(theme::ASSISTANT),
                ),
                Span::styled(" streaming", Style::default().fg(theme::MUTED)),
            ]),
            None,
        );
        push_transcript_line(&mut lines, &mut hits, Line::default(), None);
        for text_line in wrap_text(stream, w) {
            push_transcript_line(&mut lines, &mut hits, parse_md_line(&text_line), None);
        }
    }

    if lines.is_empty() {
        push_transcript_line(
            &mut lines,
            &mut hits,
            Line::from(vec![
                Span::styled(
                    "nca",
                    Style::default()
                        .fg(theme::ASSISTANT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" — session ready", Style::default().fg(theme::MUTED)),
            ]),
            None,
        );
        push_transcript_line(&mut lines, &mut hits, Line::default(), None);
        push_transcript_line(
            &mut lines,
            &mut hits,
            Line::from(Span::styled(
                "Tab  agent   Ctrl+V  image   Ctrl+P  commands   !cmd  shell   @path  search   /  inline   wheel  scroll\n\n                drag to select  ·  release to copy to clipboard",
                Style::default().fg(theme::MUTED),
            )),
            None,
        );
    }

    (lines, hits)
}

fn transcript_lines(state: &TuiSessionState, width: u16) -> Vec<Line<'static>> {
    transcript_lines_and_hits(state, width).0
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

fn wrap_text(s: &str, width: usize) -> Vec<String> {
    if width < 8 {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    for paragraph in s.split('\n') {
        if paragraph.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut line_w = 0usize;
        for word in paragraph.split_whitespace() {
            let word_w: usize = word.chars().map(char_width).sum();
            if line.is_empty() {
                line = word.to_string();
                line_w = word_w;
            } else if line_w + 1 + word_w <= width {
                line.push(' ');
                line.push_str(word);
                line_w += 1 + word_w;
            } else {
                out.push(line);
                line = word.to_string();
                line_w = word_w;
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    // Second pass: split any line that still exceeds width (CJK text without spaces).
    let mut final_out = Vec::new();
    for l in out {
        if l.chars().map(char_width).sum::<usize>() <= width {
            final_out.push(l);
        } else {
            let mut cur = String::new();
            let mut cur_w = 0usize;
            for ch in l.chars() {
                let w = char_width(ch);
                if cur_w + w > width {
                    final_out.push(cur);
                    cur = String::new();
                    cur_w = 0;
                }
                cur.push(ch);
                cur_w += w;
            }
            if !cur.is_empty() {
                final_out.push(cur);
            }
        }
    }
    if final_out.is_empty() && !s.is_empty() {
        final_out.push(s.to_string());
    }
    final_out
}

fn wrap_preformatted_line(line: &str, width: usize) -> Vec<String> {
    if width < 4 || line.is_empty() {
        return vec![line.to_string()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;
    for ch in line.chars() {
        let w = char_width(ch);
        if current_w + w > width {
            out.push(current);
            current = String::new();
            current_w = 0;
        }
        current.push(ch);
        current_w += w;
    }
    if out.is_empty() || !current.is_empty() {
        out.push(current);
    }
    out
}

fn push_wrapped_plain_lines(
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<LineAnswerHit>,
    text: &str,
    width: usize,
    style: Style,
) {
    for source_line in text.lines() {
        let wrapped = wrap_preformatted_line(source_line, width);
        for line in wrapped {
            push_transcript_line(lines, hits, Line::from(Span::styled(line, style)), None);
        }
        if source_line.is_empty() {
            push_transcript_line(lines, hits, Line::default(), None);
        }
    }
}

fn render_approval_block(
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<LineAnswerHit>,
    req: &ApprovalRequest,
    width: usize,
) {
    push_transcript_line(
        lines,
        hits,
        Line::from(vec![
            Span::styled(
                " ? ",
                Style::default().fg(Color::Black).bg(theme::WARN).bold(),
            ),
            Span::styled(
                format!(" approval required: {}", req.tool),
                Style::default()
                    .fg(theme::WARN)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        None,
    );
    push_transcript_line(lines, hits, Line::default(), None);
    for text_line in wrap_text(&req.description, width) {
        push_transcript_line(
            lines,
            hits,
            Line::from(Span::styled(text_line, Style::default().fg(theme::TEXT))),
            None,
        );
    }
    push_transcript_line(lines, hits, Line::default(), None);
    push_transcript_line(
        lines,
        hits,
        Line::from(Span::styled(
            " Input ",
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::BOLD),
        )),
        None,
    );
    push_wrapped_plain_lines(
        lines,
        hits,
        &req.input,
        width,
        Style::default().fg(theme::MUTED),
    );
    push_transcript_line(
        lines,
        hits,
        Line::from(Span::styled(
            " Reply: y/n · Ctrl+Y approve · Ctrl+N deny · Ctrl+U always allow · /approve · /deny",
            Style::default().fg(theme::MUTED),
        )),
        None,
    );
    push_transcript_line(lines, hits, Line::default(), None);
}

/// Parse user approval input (flexible: punctuation, synonyms, `/approve` style).
fn parse_approval_verdict(line: &str) -> Option<bool> {
    let mut s = line.trim().to_lowercase();
    while matches!(
        s.chars().last(),
        Some('.' | '!' | '?' | ',' | ';' | ':' | '"' | '\'')
    ) {
        s.pop();
    }
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Slash commands (handled before this in caller for passthrough; bare forms here too)
    match s {
        "/approve" | "/y" | "/yes" | "/ok" => return Some(true),
        "/deny" | "/n" | "/no" => return Some(false),
        _ => {}
    }
    let word = s.split_whitespace().next()?;
    match word {
        "y" | "yes" | "ok" | "okay" | "approve" | "approved" | "allow" | "1" | "true" => Some(true),
        "n" | "no" | "deny" | "denied" | "reject" | "rejected" | "decline" | "declined" | "0"
        | "false" => Some(false),
        _ => None,
    }
}

fn parse_md_line(line: &str) -> Line<'static> {
    if line.starts_with("```") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme::MUTED),
        ));
    }
    let mut spans: Vec<Span> = Vec::new();
    let mut rest = line.to_string();
    while !rest.is_empty() {
        if let Some(pos) = rest.find("**") {
            if pos > 0 {
                spans.push(Span::styled(
                    rest[..pos].to_string(),
                    Style::default().fg(theme::TEXT),
                ));
            }
            rest = rest[pos + 2..].to_string();
            if let Some(end) = rest.find("**") {
                spans.push(Span::styled(
                    rest[..end].to_string(),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ));
                rest = rest[end + 2..].to_string();
            } else {
                spans.push(Span::raw("**"));
                break;
            }
        } else {
            spans.push(Span::styled(rest, Style::default().fg(theme::TEXT)));
            break;
        }
    }
    Line::from(spans)
}

fn parse_tui_question_answer(
    raw: &str,
    q: &nca_common::event::InteractiveQuestionPayload,
) -> Option<QuestionSelection> {
    let t = raw.trim();
    if t.is_empty() || t == "0" || t.eq_ignore_ascii_case("s") {
        return Some(QuestionSelection::Suggested);
    }
    if let Ok(n) = t.parse::<usize>()
        && n >= 1
        && n <= q.options.len()
    {
        return Some(QuestionSelection::Option {
            option_id: q.options[n - 1].id.clone(),
        });
    }
    if q.allow_custom && !t.is_empty() {
        return Some(QuestionSelection::Custom {
            text: t.to_string(),
        });
    }
    None
}

/// `question_answer_tx`: when `Some`, answers are sent there so they unblock `ask_question` while
/// the async loop is stuck in `run_turn` (that task does not poll `cmd_rx` until the turn ends).
pub fn run_blocking(
    state: Arc<Mutex<TuiSessionState>>,
    cmd_tx: UnboundedSender<TuiCmd>,
    question_answer_tx: Option<UnboundedSender<(String, QuestionSelection)>>,
    approval_answer_tx: Option<UnboundedSender<ApprovalAnswer>>,
    show_run_banner: bool,
    cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> anyhow::Result<()> {
    let mut terminal = setup_terminal()?;

    // Load slash entries once: hardcoded commands + discovered skills
    let skill_dirs = vec![PathBuf::from(".nca/skills")];
    let workspace_root = {
        let g = state.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        g.workspace_root.clone()
    };
    let slash_entries = load_slash_entries(&workspace_root, &skill_dirs);
    let workspace_files = file_mentions::discover_workspace_files(&workspace_root);

    if show_run_banner && let Ok(mut g) = state.lock() {
        g.blocks.push(DisplayBlock::System(
            "Interactive run — type a message, Tab cycles agent profile, Ctrl+P opens commands."
                .into(),
        ));
    }

    loop {
        {
            let mut g = state.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
            if g.should_exit {
                break;
            }

            let slash_filtered = filter_slash_entries(&slash_entries, &g.input_buffer);
            let at_matches =
                at_completion_matches(&workspace_files, &g.input_buffer, g.cursor_char_idx);
            let chrome_h = composer_chrome_height(
                &slash_entries,
                &workspace_files,
                &g.input_buffer,
                g.cursor_char_idx,
            );

            terminal.draw(|frame| {
                let area = frame.area();
                let (main_area, sidebar_opt) = layout_with_sidebar(area);
                let (tr, st_r, slash_opt, inp_r) = layout_chunks(main_area, chrome_h);

                let transcript_h = tr.height.saturating_sub(2) as usize;
                let inner_w = tr.width.saturating_sub(2);
                let (lines, _hits) = transcript_lines_and_hits(&g, inner_w);
                let total = lines.len();
                let max_scroll = total.saturating_sub(transcript_h);
                if g.transcript_follow_tail {
                    g.scroll_lines = max_scroll;
                } else {
                    g.scroll_lines = g.scroll_lines.min(max_scroll);
                }
                let start = g.scroll_lines;
                let end = (start + transcript_h).min(total);
                let sel = g.transcript_selection; // ((line, col), (line, col)) in global coords
                let visible: Vec<Line> = if start < end {
                    lines[start..end]
                        .iter()
                        .enumerate()
                        .map(|(i, line)| {
                            let global = start + i;
                            let in_selection = sel
                                .map(|((sl, sc), (el, ec))| {
                                    if global < sl || global > el {
                                        false
                                    } else if global == sl && global == el {
                                        // Single line selection: column range matters.
                                        let lo = sc.min(ec);
                                        let hi = sc.max(ec);
                                        lo <= hi // always true, but column check for spans
                                    } else {
                                        true
                                    }
                                })
                                .unwrap_or(false);
                            if in_selection {
                                let ((sl, sc), (el, ec)) = sel.unwrap();
                                let (lo_line, lo_col) = if sl < el || (sl == el && sc <= ec) {
                                    (sl, sc)
                                } else {
                                    (el, ec)
                                };
                                let (hi_line, hi_col) = if sl < el || (sl == el && sc <= ec) {
                                    (el, ec)
                                } else {
                                    (sl, sc)
                                };
                                // Clamp columns to line width.
                                let line_width: usize = line.spans.iter().map(|s| s.width()).sum();
                                let sel_start_col = if global == lo_line { lo_col.min(line_width) } else { 0 };
                                let sel_end_col = if global == hi_line { hi_col.min(line_width) } else { line_width };

                                // Empty column range → no visible highlight on this line.
                                if sel_start_col >= sel_end_col {
                                    return line.clone();
                                }

                                // Build highlighted spans with partial selection support.
                                let mut highlighted_spans: Vec<Span<'static>> = Vec::new();
                                let mut col_acc: usize = 0; // running column accumulator (NOT mutated inside closures)
                                for sp in &line.spans {
                                    let span_width = sp.width();
                                    let span_start = col_acc;
                                    let span_end = col_acc + span_width;

                                    if span_end <= sel_start_col || span_start >= sel_end_col {
                                        // Span is entirely outside selection.
                                        highlighted_spans.push(sp.clone());
                                    } else if span_start >= sel_start_col && span_end <= sel_end_col {
                                        // Span is entirely inside selection.
                                        highlighted_spans.push(Span::styled(
                                            sp.content.clone(),
                                            sp.style.bg(theme::USER).fg(Color::Black),
                                        ));
                                    } else {
                                        // Span partially overlaps selection — split it.
                                        let content = sp.content.as_ref();
                                        if span_start < sel_start_col && span_end > sel_end_col {
                                            // Selection starts AND ends mid-span: three-way split.
                                            // left (outside) | middle (inside) | right (outside)
                                            let mut tmp_col = span_start;
                                            let left_chars = content.chars().take_while(|c| {
                                                let w = char_width(*c);
                                                if tmp_col < sel_start_col {
                                                    tmp_col += w;
                                                    true
                                                } else {
                                                    false
                                                }
                                            }).count();
                                            highlighted_spans.push(Span::styled(
                                                content.chars().take(left_chars).collect::<String>(),
                                                sp.style,
                                            ));
                                            let mid_chars = content.chars().skip(left_chars).take_while(|c| {
                                                let w = char_width(*c);
                                                if tmp_col < sel_end_col {
                                                    tmp_col += w;
                                                    true
                                                } else {
                                                    false
                                                }
                                            }).count();
                                            let mid: String = content.chars().skip(left_chars).take(mid_chars).collect();
                                            if !mid.is_empty() {
                                                highlighted_spans.push(Span::styled(
                                                    mid,
                                                    sp.style.bg(theme::USER).fg(Color::Black),
                                                ));
                                            }
                                            let right: String = content.chars().skip(left_chars + mid_chars).collect();
                                            if !right.is_empty() {
                                                highlighted_spans.push(Span::styled(
                                                    right,
                                                    sp.style,
                                                ));
                                            }
                                        } else if span_start < sel_start_col {
                                            // Selection starts mid-span, extends to span end.
                                            let mut tmp_col = span_start;
                                            let left_chars = content.chars().take_while(|c| {
                                                let w = char_width(*c);
                                                if tmp_col < sel_start_col {
                                                    tmp_col += w;
                                                    true
                                                } else {
                                                    false
                                                }
                                            }).count();
                                            highlighted_spans.push(Span::styled(
                                                content.chars().take(left_chars).collect::<String>(),
                                                sp.style,
                                            ));
                                            let remaining: String = content.chars().skip(left_chars).collect();
                                            highlighted_spans.push(Span::styled(
                                                remaining,
                                                sp.style.bg(theme::USER).fg(Color::Black),
                                            ));
                                        } else {
                                            // Selection ends mid-span: left=inside, right=outside.
                                            let mut tmp_col = span_start;
                                            let inside_chars = content.chars().take_while(|c| {
                                                let w = char_width(*c);
                                                if tmp_col < sel_end_col {
                                                    tmp_col += w;
                                                    true
                                                } else {
                                                    false
                                                }
                                            }).count();
                                            let inside: String = content.chars().take(inside_chars).collect();
                                            let outside: String = content.chars().skip(inside_chars).collect();
                                            if !inside.is_empty() {
                                                highlighted_spans.push(Span::styled(
                                                    inside,
                                                    sp.style.bg(theme::USER).fg(Color::Black),
                                                ));
                                            }
                                            if !outside.is_empty() {
                                                highlighted_spans.push(Span::styled(
                                                    outside,
                                                    sp.style,
                                                ));
                                            }
                                        }
                                    }
                                    col_acc += span_width;
                                }
                                Line::from(highlighted_spans)
                            } else {
                                line.clone()
                            }
                        })
                        .collect()
                } else {
                    vec![]
                };

                let title = format!(
                    " transcript — {} lines (↑↓ wheel · End bottom) ",
                    total
                );
                let main = Paragraph::new(Text::from(visible))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(theme::BORDER))
                            .title(Span::styled(title, Style::default().fg(theme::MUTED))),
                    )
                    .wrap(Wrap { trim: false })
                    .style(Style::default().bg(theme::BG));

                frame.render_widget(main, tr);

                if let Some(sidebar) = sidebar_opt {
                    let sections = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(12),
                            Constraint::Length(8),
                            Constraint::Min(10),
                        ])
                        .split(sidebar);

                    let ws_line = if g.workspace_display.is_empty() {
                        "—".to_string()
                    } else {
                        sidebar_fit(&g.workspace_display, 26)
                    };
                    let session_lines = vec![
                        Line::from(Span::styled(
                            "workspace",
                            Style::default().fg(theme::MUTED),
                        )),
                        Line::from(ws_line),
                        Line::default(),
                        Line::from(format!("session {}", &g.session_id[..8.min(g.session_id.len())])),
                        Line::from(format!("model   {}", g.model)),
                        Line::from(format!("agent   {}", g.agent_profile)),
                        Line::from(format!("mode    {}", g.permission_mode)),
                        Line::from(format!(
                            "status  {}",
                            if g.busy { "busy" } else { "idle" }
                        )),
                        Line::from(format!("blocks  {}", g.blocks.len())),
                        Line::from(format!("lines   {total}")),
                    ];
                    let session_block = Paragraph::new(Text::from(session_lines))
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(theme::BORDER))
                                .title(Span::styled(
                                    " context ",
                                    Style::default().fg(theme::MUTED),
                                )),
                        )
                        .style(Style::default().bg(theme::SURFACE))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(session_block, sections[0]);

                    let usage_lines = vec![
                        Line::from(format!("input   {}", g.input_tokens)),
                        Line::from(format!("output  {}", g.output_tokens)),
                        Line::from(format!("total   {}", g.input_tokens + g.output_tokens)),
                        Line::from(format!("cost    ${:.4}", g.cost_usd)),
                        Line::default(),
                        Line::from({
                            let pct = g.context_usage_percent;
                            let color = if pct >= 90 { theme::ERROR } else if pct >= 70 { theme::WARN } else { theme::SUCCESS };
                            Span::styled(format!("ctx  {}%", pct), Style::default().fg(color))
                        }),
                        if g.context_window > 0 {
                            Line::from(Span::styled(
                                format!("win  {}", g.context_window),
                                Style::default().fg(theme::MUTED),
                            ))
                        } else {
                            Line::default()
                        },
                        Line::from(if g.active_approval.is_some() {
                            "pending approval"
                        } else if g.active_question.is_some() {
                            "pending question"
                        } else {
                            "no pending prompt"
                        }),
                    ];
                    let usage_block = Paragraph::new(Text::from(usage_lines))
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(theme::BORDER))
                                .title(Span::styled(
                                    " usage ",
                                    Style::default().fg(theme::MUTED),
                                )),
                        )
                        .style(Style::default().bg(theme::SURFACE))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(usage_block, sections[1]);

                    let mut todo_lines: Vec<Line> = vec![Line::from(Span::styled(
                        "sub-agents",
                        Style::default()
                            .fg(theme::MUTED)
                            .add_modifier(Modifier::BOLD),
                    ))];
                    if g.subagents.is_empty() {
                        todo_lines.push(Line::from(Span::styled(
                            "none (spawn shows here)",
                            Style::default().fg(theme::MUTED),
                        )));
                    } else {
                        for row in g.subagents.iter().take(8) {
                            let dot = if row.running { "●" } else { "○" };
                            let id8 = sidebar_fit(&row.id, 8);
                            let ph = sidebar_fit(&row.phase, 11);
                            todo_lines.push(Line::from(vec![
                                Span::styled(
                                    format!("{dot} "),
                                    Style::default().fg(if row.running {
                                        theme::WARN
                                    } else {
                                        theme::MUTED
                                    }),
                                ),
                                Span::styled(format!("{id8} "), Style::default().fg(theme::TEXT)),
                                Span::styled(ph, Style::default().fg(theme::TOOL)),
                            ]));
                            if !row.detail.is_empty() {
                                todo_lines.push(Line::from(Span::styled(
                                    format!("  {}", sidebar_fit(&row.detail, 26)),
                                    Style::default().fg(theme::MUTED),
                                )));
                            }
                            if let Some(ref skill_name) = row.skill {
                                todo_lines.push(Line::from(Span::styled(
                                    format!("  [{}]", sidebar_fit(skill_name, 24)),
                                    Style::default().fg(theme::WARN),
                                )));
                            }
                            if !row.task.is_empty() && row.task != "(sub-agent)" {
                                todo_lines.push(Line::from(Span::styled(
                                    format!("  {}", sidebar_fit(&row.task, 26)),
                                    Style::default().fg(theme::TEXT),
                                )));
                            }
                        }
                    }
                    todo_lines.push(Line::default());
                    todo_lines.push(Line::from(Span::styled(
                        "dev",
                        Style::default()
                            .fg(theme::MUTED)
                            .add_modifier(Modifier::BOLD),
                    )));
                    todo_lines.push(Line::from(Span::styled(
                        ".nca/sessions",
                        Style::default().fg(theme::USER),
                    )));
                    todo_lines.push(Line::from(Span::styled(
                        "docs/research/",
                        Style::default().fg(theme::USER),
                    )));
                    todo_lines.push(Line::from(Span::styled(
                        "Ctrl+P commands",
                        Style::default().fg(theme::MUTED),
                    )));
                    let todo_block = Paragraph::new(Text::from(todo_lines))
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(theme::BORDER))
                                .title(Span::styled(
                                    " sidebar ",
                                    Style::default().fg(theme::MUTED),
                                )),
                        )
                        .style(Style::default().bg(theme::SURFACE))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(todo_block, sections[2]);
                }

                let elapsed = g.started.elapsed().as_secs();
                let indicator_text = crate::tui::busy_indicator::render_indicator(
                    g.current_busy_state,
                    g.busy_state_since,
                );
                let indicator_color =
                    crate::tui::busy_indicator::color_for_state(g.current_busy_state);
                let busy = Span::styled(indicator_text, Style::default().fg(indicator_color));
                let approval_hint = if g.active_approval.is_some() {
                    Span::styled(" !approve ", Style::default().fg(theme::ERROR))
                } else {
                    Span::raw("")
                };
                let q_hint = if g.active_question.is_some() {
                    Span::styled(" ?answer ", Style::default().fg(theme::WARN))
                } else {
                    Span::raw("")
                };
                // Session / tokens / cost live in the sidebar; keep the bar short and obvious about bypass.
                let perm_span = if toolbar_permission_is_bypass(&g.permission_mode) {
                    Span::styled(
                        " BYPASS — tools run without approval ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(theme::ERROR)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    Span::styled(
                        format!(" perm:{} ", g.permission_mode),
                        Style::default().fg(theme::MUTED),
                    )
                };
                let time_span = Span::styled(
                    format!("{:02}:{:02}", elapsed / 60, elapsed % 60),
                    Style::default().fg(theme::MUTED),
                );

                let cancel_hint_text = " Esc cancel ";
                let cancel_hint = escape_cancels_active_turn(&g).then(|| {
                    Span::styled(
                        cancel_hint_text,
                        Style::default()
                            .fg(Color::Black)
                            .bg(theme::WARN)
                            .add_modifier(Modifier::BOLD),
                    )
                });
                let status_rect = if cancel_hint.is_some() && st_r.width > cancel_hint_text.len() as u16 {
                    Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([
                            Constraint::Min(0),
                            Constraint::Length(cancel_hint_text.len() as u16),
                        ])
                        .split(st_r)[0]
                } else {
                    st_r
                };

                // Compute the character-cell x-offset before any borrow of `g` escapes into `status_spans`.
                let branch_char_offset = 4 + g.model.len() + 4 + g.agent_profile.len() + 4;
                let branch_text = if g.current_branch.is_empty() {
                    String::new()
                } else {
                    format!("⎇ {}", g.current_branch)
                };
                let branch_span_style = Style::default()
                    .fg(theme::TOOL)
                    .add_modifier(Modifier::UNDERLINED);

                // Store the branch chip bounds for click hit-testing.
                if status_rect.width > branch_char_offset as u16 && !branch_text.is_empty() {
                    let chip_len = branch_text.len() as u16;
                    g.branch_chip_bounds = Some(Rect::new(
                        status_rect.x + branch_char_offset as u16,
                        status_rect.y,
                        chip_len.min(status_rect.width - branch_char_offset as u16),
                        1,
                    ));
                } else {
                    g.branch_chip_bounds = None;
                }

                let mut status_spans = vec![
                    busy,
                    approval_hint,
                    q_hint,
                    Span::raw(" │ "),
                    Span::styled(&g.model, Style::default().fg(theme::USER)),
                    Span::raw(" │ "),
                    Span::styled(&g.agent_profile, Style::default().fg(theme::ASSISTANT)),
                    Span::raw(" │ "),
                    // branch_text borrow ends before next mutable use of `g` below
                    Span::styled(branch_text, branch_span_style),
                    Span::raw(" │ "),
                    perm_span,
                ];
                // Sidebar is hidden on narrow terminals — put session/tokens/cost back on the bar.
                if sidebar_opt.is_none() {
                    status_spans.push(Span::raw(" │ "));
                    status_spans.push(Span::styled(
                        g.session_id[..8.min(g.session_id.len())].to_string(),
                        Style::default().fg(theme::MUTED),
                    ));
                    status_spans.extend([
                        Span::raw(" │ in:"),
                        Span::styled(
                            format!("{}", g.input_tokens),
                            Style::default().fg(theme::TEXT),
                        ),
                        Span::raw(" out:"),
                        Span::styled(
                            format!("{}", g.output_tokens),
                            Style::default().fg(theme::TEXT),
                        ),
                        Span::raw(" │ $"),
                        Span::styled(
                            format!("{:.4}", g.cost_usd),
                            Style::default().fg(theme::SUCCESS),
                        ),
                    ]);
                }
                // Context usage percentage (always on status bar).
                let ctx_pct_color = if g.context_usage_percent >= 90 {
                    theme::ERROR
                } else if g.context_usage_percent >= 70 {
                    theme::WARN
                } else {
                    theme::MUTED
                };
                status_spans.push(Span::raw(" │ ctx:"));
                status_spans.push(Span::styled(
                    format!("{}%", g.context_usage_percent),
                    Style::default().fg(ctx_pct_color),
                ));
                status_spans.push(Span::raw(" │ "));
                status_spans.push(time_span);
                let status = Line::from(status_spans);
                let bar = Paragraph::new(status).style(Style::default().bg(theme::SURFACE));
                frame.render_widget(bar, status_rect);
                if let Some(cancel_hint) = cancel_hint {
                    let hint_width = cancel_hint_text.len() as u16;
                    if st_r.width > hint_width {
                        let hint_rect = Rect::new(
                            st_r.x + st_r.width.saturating_sub(hint_width),
                            st_r.y,
                            hint_width,
                            1,
                        );
                        let hint_bar = Paragraph::new(Line::from(cancel_hint))
                            .style(Style::default().bg(theme::SURFACE));
                        frame.render_widget(hint_bar, hint_rect);
                    }
                }

                if let Some(sr) = slash_opt {
                    if slash_panel_visible(&g.input_buffer) && !slash_filtered.is_empty() {
                        let n_show = slash_filtered.len().min(SLASH_PANEL_MAX_ROWS);
                        let max_scroll = slash_filtered.len().saturating_sub(n_show);
                        let list_scroll = g
                            .slash_menu_index
                            .saturating_sub(n_show.saturating_sub(1))
                            .min(max_scroll);
                        let mut slash_lines: Vec<Line> = Vec::new();
                        for (i, entry) in slash_filtered[list_scroll..list_scroll + n_show]
                            .iter()
                            .enumerate()
                        {
                            let global = list_scroll + i;
                            let st = if global == g.slash_menu_index {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(theme::USER)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(theme::TEXT)
                            };
                            slash_lines.push(Line::from(Span::styled(entry.display_text(), st)));
                        }
                        if slash_filtered.len() > n_show {
                            slash_lines.push(Line::from(Span::styled(
                                format!(
                                    " ─ {}/{} · ↑↓",
                                    g.slash_menu_index + 1,
                                    slash_filtered.len()
                                ),
                                Style::default().fg(theme::MUTED),
                            )));
                        }
                        let slash_w = Paragraph::new(Text::from(slash_lines))
                            .block(
                                Block::default()
                                    .borders(Borders::ALL)
                                    .border_style(Style::default().fg(theme::BORDER))
                                    .title(Span::styled(
                                        " commands (↑↓ Tab complete) ",
                                        Style::default().fg(theme::MUTED),
                                    )),
                            )
                            .style(Style::default().bg(theme::SURFACE));
                        frame.render_widget(slash_w, sr);
                    } else if !at_matches.is_empty() {
                        let n_show = at_matches.len().min(SLASH_PANEL_MAX_ROWS);
                        let max_scroll = at_matches.len().saturating_sub(n_show);
                        let pick = g.at_menu_index.min(at_matches.len().saturating_sub(1));
                        let list_scroll =
                            pick.saturating_sub(n_show.saturating_sub(1)).min(max_scroll);
                        let mut lines: Vec<Line> = Vec::new();
                        for (i, path) in at_matches[list_scroll..list_scroll + n_show]
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
                            // Show path without @ prefix since @ is already in the buffer
                            lines.push(Line::from(Span::styled(format!(" {path}"), st)));
                        }
                        if at_matches.len() > n_show {
                            lines.push(Line::from(Span::styled(
                                format!(" ─ {}/{} · ↑↓ Tab", pick + 1, at_matches.len()),
                                Style::default().fg(theme::MUTED),
                            )));
                        }
                        let at_w = Paragraph::new(Text::from(lines))
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
                        frame.render_widget(at_w, sr);
                    }
                }

                let input_line = composer_line(
                    &g.input_buffer,
                    g.cursor_char_idx,
                    inp_r.width.saturating_sub(2) as usize, // border
                );

                let hint = if g.active_approval.is_some() {
                    Line::from(Span::styled(
                        "Approval: y/n · Ctrl+Y approve · Ctrl+N deny · Ctrl+U always allow · /approve · /deny · other /commands still work",
                        Style::default().fg(theme::ERROR),
                    ))
                } else if g.active_question.is_some() && !g.question_modal_open {
                    Line::from(Span::styled(
                        "Enter / 0 = suggested · 1–n = option · click underlined line · /auto-answer · End = transcript bottom (empty input)",
                        Style::default().fg(theme::WARN),
                    ))
                } else if g.input_buffer.is_empty() {
                    Line::from(Span::styled(
                        "Enter send · Tab agent · Ctrl+V image · /image · Ctrl+P palette · Ctrl+Q exit · Ctrl+L clear · drag select → copy",
                        Style::default().fg(theme::MUTED),
                    ))
                } else {
                    Line::default()
                };

                let input_title = if g.active_approval.is_some() {
                    " approval "
                } else if g.active_question.is_some() {
                    " answer "
                } else {
                    " message "
                };
                let mut input_lines = vec![input_line];
                if !g.staged_image_attachments.is_empty() {
                    input_lines.push(Line::from(Span::styled(
                        format!(
                            "  {} image(s) staged · Enter to send · /image clear",
                            g.staged_image_attachments.len()
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

                if g.command_palette_open {
                    let filtered = filter_palette_rows(&g.command_palette_query);
                    let selectable = palette_selectable_indices(&filtered);
                    let pick_abs = if selectable.is_empty() {
                        0
                    } else {
                        selectable[g.palette_index.min(selectable.len().saturating_sub(1))]
                    };
                    let total_vis = filtered.len().clamp(1, COMMAND_PALETTE_MAX_ROWS);
                    let popup_area = centered_rect(area, COMMAND_PALETTE_WIDTH, (total_vis as u16).saturating_add(6));
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
                                if g.command_palette_query.is_empty() {
                                    "type to filter"
                                } else {
                                    g.command_palette_query.as_str()
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
                                        Style::default().fg(theme::MUTED).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                                    )));
                                }
                                PaletteRow::Entry { label, shortcut } => {
                                    let global = filtered.iter().position(|r| std::ptr::eq(*r, idx)).unwrap_or(0);
                                    let is_selected = global == pick_abs;
                                    let label_style = if is_selected {
                                        Style::default().fg(Color::Black).bg(theme::USER).add_modifier(Modifier::BOLD)
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
                                        spans.push(Span::styled(format!("{:>pad$}", shortcut, pad = pad), shortcut_style));
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

                // Branch picker popup.
                if g.branch_picker_open {
                    let branches = &g.branch_picker_branches;
                    let filtered = filtered_branch_indices(branches, &g.branch_picker_query);

                    let popup_h = (filtered.len().min(12) as u16).saturating_add(6).max(8);
                    let popup_area = centered_rect(area, 36, popup_h);

                    let mut popup_lines = vec![
                        Line::from(vec![
                            Span::styled(" Branch ", Style::default().fg(theme::MUTED).add_modifier(Modifier::BOLD)),
                            Span::styled(
                                if g.branch_picker_query.is_empty() {
                                    "".to_string()
                                } else {
                                    format!(": {}", g.branch_picker_query)
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
                        let list_scroll = g
                            .branch_picker_index
                            .saturating_sub(n_show.saturating_sub(1))
                            .min(filtered.len().saturating_sub(n_show));
                        for (i, branch_idx) in filtered[list_scroll..list_scroll + n_show].iter().enumerate() {
                            let filtered_idx = list_scroll + i;
                            let branch = &branches[*branch_idx];
                            let style = if filtered_idx == g.branch_picker_index {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(theme::USER)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(theme::TEXT)
                            };
                            let mark = if branch.as_str() == g.current_branch { " *" } else { "" };
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
                                .title(Span::styled(" git branch ", Style::default().fg(theme::MUTED))),
                        )
                        .style(Style::default().bg(theme::SURFACE))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(popup, popup_area);
                }

                // LLM provider picker (default provider or API-key target).
                if g.provider_picker_open {
                    let names: Vec<&'static str> = ProviderKind::ALL
                        .iter()
                        .map(|p| p.display_name())
                        .collect();
                    let rows = (names.len() as u16).saturating_add(6).max(8);
                    let popup_area = centered_rect(area, 40, rows);
                    let mut lines: Vec<Line> = vec![
                        Line::from(Span::styled(
                            if g.provider_picker_for_api_key {
                                " Select provider for API key "
                            } else {
                                " Default LLM provider "
                            },
                            Style::default().fg(theme::MUTED).add_modifier(Modifier::BOLD),
                        )),
                        Line::default(),
                    ];
                    for (i, name) in names.iter().enumerate() {
                        let st = if i == g.provider_picker_index {
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
                                .title(Span::styled(" settings ", Style::default().fg(theme::MUTED))),
                        )
                        .style(Style::default().bg(theme::SURFACE))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(popup, popup_area);
                }

                if g.permission_picker_open {
                    const PERM_LABELS: &[&str] = &["Default", "Plan", "AcceptEdits", "DontAsk", "BypassPermissions"];
                    let rows = (PERM_LABELS.len() as u16).saturating_add(6).max(8);
                    let popup_area = centered_rect(area, 40, rows);
                    let mut lines: Vec<Line> = vec![
                        Line::from(Span::styled(
                            " Permission mode ",
                            Style::default().fg(theme::MUTED).add_modifier(Modifier::BOLD),
                        )),
                        Line::default(),
                    ];
                    for (i, name) in PERM_LABELS.iter().enumerate() {
                        let st = if i == g.permission_picker_index {
                            Style::default().fg(Color::Black).bg(theme::USER).add_modifier(Modifier::BOLD)
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
                                .title(Span::styled(" permissions ", Style::default().fg(theme::MUTED))),
                        )
                        .style(Style::default().bg(theme::SURFACE))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(popup, popup_area);
                }

                if g.agent_picker_open {
                    const AGENT_LABELS: &[(&str, &str)] = &[
                        ("@build", "Full-access agent for development"),
                        ("@plan", "Read-only analysis and planning"),
                        ("@review", "Focused code review"),
                        ("@fix", "Bug diagnosis and minimal fixes"),
                        ("@test", "Testing and validation"),
                    ];
                    let rows = (AGENT_LABELS.len() as u16).saturating_add(6).max(8);
                    let popup_area = centered_rect(area, 52, rows);
                    let mut lines: Vec<Line> = vec![
                        Line::from(Span::styled(
                            " Agent profile ",
                            Style::default().fg(theme::MUTED).add_modifier(Modifier::BOLD),
                        )),
                        Line::default(),
                    ];
                    for (i, (name, desc)) in AGENT_LABELS.iter().enumerate() {
                        let st = if i == g.agent_picker_index {
                            Style::default().fg(Color::Black).bg(theme::USER).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme::TEXT)
                        };
                        let desc_st = if i == g.agent_picker_index {
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

                // Question modal popup (arrow-key option picker).
                if g.question_modal_open
                    && let Some(ref q) = g.active_question
                {
                        let has_chat_option = q.allow_custom;
                        let total_items = 1 + q.options.len() + if has_chat_option { 1 } else { 0 };
                        // +4 for: title line, blank, blank before footer, footer
                        let rows = (total_items as u16).saturating_add(6).max(8);
                        let popup_w = 60u16.min(area.width.saturating_sub(4));
                        let popup_area = centered_rect(area, popup_w, rows);

                        let mut lines: Vec<Line> = vec![
                            Line::from(Span::styled(
                                format!(" {} ", q.prompt),
                                Style::default()
                                    .fg(theme::ASSISTANT)
                                    .add_modifier(Modifier::BOLD),
                            )),
                            Line::default(),
                        ];

                        // Suggested answer (index 0)
                        let suggested_label = format!(" Suggested: {} ", q.suggested_answer);
                        if g.question_modal_index == 0 {
                            lines.push(Line::from(Span::styled(
                                format!(" ► {}", suggested_label.trim()),
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(theme::USER)
                                    .add_modifier(Modifier::BOLD),
                            )));
                        } else {
                            lines.push(Line::from(Span::styled(
                                format!("   {}", suggested_label.trim()),
                                Style::default().fg(theme::TEXT),
                            )));
                        }

                        // Options (index 1..n)
                        for (i, o) in q.options.iter().enumerate() {
                            let item_idx = i + 1;
                            let label = format!("{} ", o.label);
                            if g.question_modal_index == item_idx {
                                lines.push(Line::from(Span::styled(
                                    format!(" ► {}", label.trim()),
                                    Style::default()
                                        .fg(Color::Black)
                                        .bg(theme::USER)
                                        .add_modifier(Modifier::BOLD),
                                )));
                            } else {
                                lines.push(Line::from(Span::styled(
                                    format!("   {}", label.trim()),
                                    Style::default().fg(theme::TEXT),
                                )));
                            }
                        }

                        // "Chat about this" (last item, only if allow_custom)
                        if has_chat_option {
                            let chat_idx = 1 + q.options.len();
                            if g.question_modal_index == chat_idx {
                                lines.push(Line::from(Span::styled(
                                    " ► Chat about this",
                                    Style::default()
                                        .fg(Color::Black)
                                        .bg(theme::USER)
                                        .add_modifier(Modifier::BOLD),
                                )));
                            } else {
                                lines.push(Line::from(Span::styled(
                                    "   Chat about this",
                                    Style::default()
                                        .fg(theme::MUTED)
                                        .add_modifier(Modifier::ITALIC),
                                )));
                            }
                        }

                        // Footer
                        lines.push(Line::default());
                        let footer_text = if has_chat_option {
                            " ↑↓ select · Enter confirm · Esc chat "
                        } else {
                            " ↑↓ select · Enter confirm "
                        };
                        lines.push(Line::from(Span::styled(
                            footer_text,
                            Style::default().fg(theme::MUTED),
                        )));

                        frame.render_widget(ClearWidget, popup_area);
                        let popup = Paragraph::new(Text::from(lines))
                            .block(
                                Block::default()
                                    .borders(Borders::ALL)
                                    .border_style(Style::default().fg(theme::BORDER))
                                    .title(Span::styled(
                                        " question ",
                                        Style::default().fg(theme::WARN),
                                    )),
                            )
                            .style(Style::default().bg(theme::SURFACE))
                            .wrap(Wrap { trim: false });
                        frame.render_widget(popup, popup_area);
                }

                if g.session_picker_open {
                    let filter = g.session_picker_search.to_ascii_lowercase();
                    let filtered_indices: Vec<usize> = g.session_picker_entries.iter().enumerate()
                        .filter(|(_, s)| {
                            filter.is_empty()
                                || s.id.to_ascii_lowercase().contains(&filter)
                                || s.session_title
                                    .as_ref()
                                    .map(|t| t.to_ascii_lowercase().contains(&filter))
                                    .unwrap_or(false)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    const SESSION_PICKER_MAX_ROWS: usize = 16;
                    let n_filtered = filtered_indices.len();
                    let viewport_rows = n_filtered.min(SESSION_PICKER_MAX_ROWS);
                    let rows = (viewport_rows as u16).saturating_add(8).max(10);
                    let popup_area = centered_rect(area, 56, rows);
                    let pick = g.session_picker_index.min(n_filtered.saturating_sub(1));

                    if pick < g.session_picker_scroll {
                        g.session_picker_scroll = pick;
                    } else if viewport_rows > 0 && pick >= g.session_picker_scroll + viewport_rows {
                        g.session_picker_scroll = pick.saturating_sub(viewport_rows - 1);
                    }
                    g.session_picker_scroll = g.session_picker_scroll.min(n_filtered.saturating_sub(viewport_rows));
                    let list_start = g.session_picker_scroll;
                    let list_end = (list_start + viewport_rows).min(n_filtered);

                    let search_display = if g.session_picker_search.is_empty() {
                        "type to filter".to_string()
                    } else {
                        g.session_picker_search.clone()
                    };
                    let mut lines: Vec<Line> = vec![
                        Line::from(vec![
                            Span::styled(" Search ", Style::default().fg(theme::MUTED).add_modifier(Modifier::BOLD)),
                            Span::styled(search_display, Style::default().fg(theme::TEXT)),
                        ]),
                        Line::default(),
                    ];
                    if filtered_indices.is_empty() {
                        lines.push(Line::from(Span::styled(" No matching sessions", Style::default().fg(theme::MUTED))));
                    } else {
                        if list_start > 0 {
                            lines.push(Line::from(Span::styled(
                                format!("  ▲ {} more", list_start),
                                Style::default().fg(theme::MUTED),
                            )));
                        }
                        let current_session_id = g.session_id.clone();
                        for (vis_idx, &filt_idx) in filtered_indices
                            .iter()
                            .enumerate()
                            .skip(list_start)
                            .take(list_end.saturating_sub(list_start))
                        {
                            let snap = &g.session_picker_entries[filt_idx];
                            let is_current = snap.id == current_session_id;
                            let marker = if is_current { " *" } else { "" };
                            let st = if vis_idx == pick {
                                Style::default().fg(Color::Black).bg(theme::USER).add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(theme::TEXT)
                            };
                            let display = if let Some(title) = &snap.session_title {
                                format!(" {title}{marker}  [{}]", snap.id)
                            } else {
                                format!(" {}{marker}", snap.id)
                            };
                            lines.push(Line::from(Span::styled(display, st)));
                        }
                        let remaining_below = n_filtered.saturating_sub(list_end);
                        if remaining_below > 0 {
                            lines.push(Line::from(Span::styled(
                                format!("  ▼ {} more", remaining_below),
                                Style::default().fg(theme::MUTED),
                            )));
                        }
                    }
                    lines.push(Line::default());
                    lines.push(Line::from(Span::styled(
                        " Enter resume · Esc close ",
                        Style::default().fg(theme::MUTED),
                    )));
                    frame.render_widget(ClearWidget, popup_area);
                    let popup = Paragraph::new(Text::from(lines))
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(theme::BORDER))
                                .title(Span::styled(" sessions ", Style::default().fg(theme::MUTED))),
                        )
                        .style(Style::default().bg(theme::SURFACE))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(popup, popup_area);
                }

                if g.api_key_modal_open {
                    let provider = g
                        .api_key_target_provider
                        .map(|p| p.display_name())
                        .unwrap_or("provider");
                    let popup_area = centered_rect(area, 66, 12);
                    let headline = if g.api_key_connect_after_save {
                        " Connect provider "
                    } else {
                        " API key "
                    };
                    let hint = if g.api_key_target_has_existing {
                        " Press Enter to keep current key, or paste a new key to replace it. "
                    } else {
                        " Paste API key, then press Enter. "
                    };
                    let masked = if g.api_key_input.is_empty() {
                        String::new()
                    } else {
                        "*".repeat(g.api_key_input.chars().count())
                    };
                    let validation_line = if g.onboarding_mode {
                        match &g.validation_status {
                            Some(crate::tui::state::OnboardingValidation::Validating) => {
                                Some(Line::from(Span::styled(
                                    " Validating...",
                                    Style::default().fg(Color::Yellow),
                                )))
                            }
                            Some(crate::tui::state::OnboardingValidation::Failed(msg)) => {
                                Some(Line::from(Span::styled(
                                    format!(" {}", msg),
                                    Style::default().fg(Color::Red),
                                )))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let mut lines = vec![
                        Line::from(vec![
                            Span::styled(
                                format!(" Provider: {provider}"),
                                Style::default()
                                    .fg(theme::TEXT)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]),
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
                    if let Some(vline) = validation_line {
                        lines.push(vline);
                    }
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

                // Generic info modal (read-only scrollable popup).
                if g.info_modal_open {
                    let max_vis = 16usize;
                    let n_lines = g.info_modal_lines.len();
                    let popup_h = (n_lines.min(max_vis) as u16).saturating_add(6).max(8);
                    let popup_area = centered_rect(area, 70, popup_h);
                    let n_show = n_lines.min(max_vis);
                    let max_scroll = n_lines.saturating_sub(n_show);
                    g.info_modal_scroll = g.info_modal_scroll.min(max_scroll);
                    let start = g.info_modal_scroll;
                    let end = (start + n_show).min(n_lines);
                    let mut lines: Vec<Line> = Vec::new();
                    for line in &g.info_modal_lines[start..end] {
                        lines.push(Line::from(Span::styled(
                            format!(" {line}"),
                            Style::default().fg(theme::TEXT),
                        )));
                    }
                    if n_lines > max_vis {
                        lines.push(Line::from(Span::styled(
                            format!(" ─ {}/{} · ↑↓ scroll", start + 1, n_lines),
                            Style::default().fg(theme::MUTED),
                        )));
                    }
                    lines.push(Line::default());
                    lines.push(Line::from(Span::styled(
                        " Esc close ",
                        Style::default().fg(theme::MUTED),
                    )));
                    frame.render_widget(ClearWidget, popup_area);
                    let title = format!(" {} ", g.info_modal_title);
                    let popup = Paragraph::new(Text::from(lines))
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(theme::BORDER))
                                .title(Span::styled(title, Style::default().fg(theme::MUTED))),
                        )
                        .style(Style::default().bg(theme::SURFACE))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(popup, popup_area);
                }

                // Model picker popup.
                if g.model_picker_open {
                    let filter = g.model_picker_search.to_ascii_lowercase();

                    // Pre-compute indices for visible/selectable items and scroll
                    // without holding an immutable borrow on `g` that conflicts
                    // with the scroll update.
                    let vis_indices: Vec<usize> = g
                        .model_picker_entries
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
                        .filter(|&(_, &orig)| !g.model_picker_entries[orig].is_header)
                        .map(|(vi, _)| vi)
                        .collect();
                    let n_sel = selectable_vis.len();
                    let pick = if n_sel > 0 {
                        g.model_picker_index.min(n_sel - 1)
                    } else {
                        0
                    };
                    let selected_vis_idx = selectable_vis.get(pick).copied().unwrap_or(0);

                    const MODEL_PICKER_MAX_ROWS: usize = 18;
                    let n_visible = vis_indices.len();
                    let viewport_rows = n_visible.min(MODEL_PICKER_MAX_ROWS);
                    let popup_h = (viewport_rows as u16).saturating_add(7).max(10);
                    let popup_area = centered_rect(area, 62, popup_h);

                    // Keep the selected item visible within the viewport.
                    if selected_vis_idx < g.model_picker_scroll {
                        g.model_picker_scroll = selected_vis_idx;
                    } else if viewport_rows > 0 && selected_vis_idx >= g.model_picker_scroll + viewport_rows {
                        g.model_picker_scroll = selected_vis_idx.saturating_sub(viewport_rows - 1);
                    }
                    g.model_picker_scroll = g.model_picker_scroll.min(n_visible.saturating_sub(viewport_rows));
                    let list_start = g.model_picker_scroll;
                    let list_end = (list_start + viewport_rows).min(n_visible);

                    let search_display = if g.model_picker_search.is_empty() {
                        "type to filter…".to_string()
                    } else {
                        g.model_picker_search.clone()
                    };
                    let mut lines: Vec<Line> = vec![
                        Line::from(vec![
                            Span::styled(
                                "Search ",
                                Style::default()
                                    .fg(theme::MUTED)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                search_display,
                                Style::default().fg(theme::TEXT),
                            ),
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
                            let entry = &g.model_picker_entries[model_idx];
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
                                .title(Span::styled(
                                    " models ",
                                    Style::default().fg(theme::MUTED),
                                )),
                        )
                        .style(Style::default().bg(theme::SURFACE))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(popup, popup_area);
                }

                // OpenCode-style "Connect a provider" (`/connect`).
                if g.connect_modal_open {
                    let rows = build_connect_rows(&g.connect_search);
                    let sel = clamp_selection(g.connect_menu_index, &rows);
                    let selected_row = row_index_for_selection(&rows, sel);
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
                                if g.connect_search.is_empty() {
                                    "type to filter…"
                                } else {
                                    g.connect_search.as_str()
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
                                    title,
                                    subtitle,
                                    ..
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
                        Span::styled(
                            " Connect a provider ",
                            Style::default().fg(theme::MUTED),
                        ),
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
            })?;
        }

        if poll(Duration::from_millis(40))? {
            let ev = read()?;
            let mut g = state.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;

            match ev {
                Event::Mouse(_) if g.command_palette_open => continue,
                Event::Mouse(_) if g.info_modal_open => continue,
                Event::Mouse(_) if g.model_picker_open => continue,
                Event::Mouse(_) if g.connect_modal_open => continue,
                Event::Mouse(_) if g.api_key_modal_open => continue,
                Event::Mouse(_) if g.provider_picker_open => continue,
                Event::Mouse(_) if g.permission_picker_open => continue,
                Event::Mouse(_) if g.agent_picker_open => continue,
                Event::Mouse(_) if g.session_picker_open => continue,
                Event::Mouse(_) if g.question_modal_open => continue,
                Event::Mouse(m) => {
                    let sz = terminal.size()?;
                    let area = Rect::new(0, 0, sz.width, sz.height);
                    let (main_area, _) = layout_with_sidebar(area);
                    let slash_filtered = filter_slash_entries(&slash_entries, &g.input_buffer);
                    let at_matches =
                        at_completion_matches(&workspace_files, &g.input_buffer, g.cursor_char_idx);
                    let sh = composer_chrome_height(
                        &slash_entries,
                        &workspace_files,
                        &g.input_buffer,
                        g.cursor_char_idx,
                    );
                    let (tr, _, slash_r, _) = layout_chunks(main_area, sh);

                    if rect_contains(tr, m.column, m.row) {
                        let inner_w = tr.width.saturating_sub(2);
                        let (lines, hits) = transcript_lines_and_hits(&g, inner_w);
                        let total = lines.len();
                        let th = tr.height.saturating_sub(2) as usize;
                        let max_scroll = total.saturating_sub(th);
                        let inner_top = tr.y.saturating_add(1);
                        let row_in_area = if m.row >= inner_top {
                            Some((m.row - inner_top) as usize)
                        } else {
                            None
                        };
                        let gline = row_in_area.filter(|r| *r < th).map(|r| g.scroll_lines + r);
                        // Column within the text area (0-indexed, excluding border).
                        let gcol = m.column.saturating_sub(tr.x).saturating_sub(1) as usize;

                        match m.kind {
                            MouseEventKind::ScrollUp => {
                                g.transcript_selection = None;
                                g.transcript_dragging = false;
                                g.transcript_follow_tail = false;
                                g.scroll_lines = g.scroll_lines.saturating_sub(MOUSE_SCROLL_LINES);
                            }
                            MouseEventKind::ScrollDown => {
                                g.transcript_selection = None;
                                g.transcript_dragging = false;
                                g.scroll_lines =
                                    (g.scroll_lines + MOUSE_SCROLL_LINES).min(max_scroll);
                                if g.scroll_lines >= max_scroll {
                                    g.transcript_follow_tail = true;
                                }
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                if let Some(gl) = gline {
                                    let picked = if gl < hits.len() {
                                        hits[gl].clone().zip(
                                            g.active_question
                                                .as_ref()
                                                .map(|q| q.question_id.clone()),
                                        )
                                    } else {
                                        None
                                    };
                                    if let Some((sel, qid)) = picked {
                                        // Clear any text selection when clicking a question option.
                                        g.transcript_selection = None;
                                        g.transcript_dragging = false;
                                        drop(g);
                                        if let Some(ref tx) = question_answer_tx {
                                            let _ = tx.send((qid, sel));
                                        } else {
                                            let _ = cmd_tx.send(TuiCmd::QuestionAnswer(sel));
                                        }
                                        continue;
                                    }
                                    // No question hit — start a new text selection or extend existing.
                                    if m.modifiers.contains(KeyModifiers::SHIFT) {
                                        // Shift+click: extend existing selection.
                                        if let Some(((anchor_line, _), _)) = g.transcript_selection
                                        {
                                            let start = (anchor_line.min(gl), 0);
                                            let end = (anchor_line.max(gl), gcol);
                                            g.transcript_selection =
                                                Some((start.min(end), start.max(end)));
                                        } else {
                                            g.transcript_drag_anchor = Some((gl, gcol));
                                        }
                                    } else {
                                        // Plain click: set selection at click position (immediate highlight).
                                        let click_pos = (gl, gcol);
                                        g.transcript_selection = Some((click_pos, click_pos));
                                        g.transcript_drag_anchor = Some((gl, gcol));
                                    }
                                    g.transcript_dragging = true;
                                }
                            }
                            MouseEventKind::Drag(MouseButton::Left) if g.transcript_dragging => {
                                let Some(gl) = gline else { continue };
                                if let Some((anchor_line, anchor_col)) = g.transcript_drag_anchor {
                                    if g.transcript_selection.is_some() {
                                        // Drag already active — update selection normally.
                                        let start = (
                                            anchor_line.min(gl),
                                            if anchor_line <= gl { anchor_col } else { gcol },
                                        );
                                        let end = (
                                            anchor_line.max(gl),
                                            if anchor_line <= gl { gcol } else { anchor_col },
                                        );
                                        g.transcript_selection = Some((start, end));
                                    } else {
                                        // First drag attempt after Down.
                                        // Only activate selection if the mouse is close to the
                                        // anchor — filters out spurious large jumps that many
                                        // terminals send on plain click.
                                        let line_dist = gl.abs_diff(anchor_line);
                                        let col_dist = gcol.abs_diff(anchor_col);
                                        if line_dist <= 1 && col_dist <= 3 {
                                            let start = (
                                                anchor_line.min(gl),
                                                if anchor_line <= gl { anchor_col } else { gcol },
                                            );
                                            let end = (
                                                anchor_line.max(gl),
                                                if anchor_line <= gl { gcol } else { anchor_col },
                                            );
                                            g.transcript_selection = Some((start, end));
                                        }
                                    }
                                } else if let Some(((anchor_line, _), _)) = g.transcript_selection {
                                    let start = (anchor_line.min(gl), 0);
                                    let end = (anchor_line.max(gl), gcol);
                                    g.transcript_selection = Some((start.min(end), start.max(end)));
                                }
                            }
                            MouseEventKind::Up(MouseButton::Left) => {
                                g.transcript_dragging = false;
                                g.transcript_drag_anchor = None;
                                // OpenCode-style: auto-copy selected text to clipboard on mouse-up.
                                if let Some((sel_start, sel_end)) = g.transcript_selection {
                                    let (sl, sc) = sel_start;
                                    let (el, ec) = sel_end;
                                    if sl < el || (sl == el && sc != ec) {
                                        let text =
                                            plain_text_from_lines(&lines, sel_start, sel_end);
                                        let n = text.trim_end_matches('\n').chars().count();
                                        match crate::image_attach::copy_text_to_clipboard(&text) {
                                            Ok(()) => {
                                                g.blocks.push(DisplayBlock::System(format!(
                                                    "Copied {n} chars to clipboard"
                                                )));
                                            }
                                            Err(e) => {
                                                g.blocks.push(DisplayBlock::System(format!(
                                                    "Clipboard failed: {e}"
                                                )));
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    if let Some(sr) = slash_r
                        && rect_contains(sr, m.column, m.row)
                        && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
                    {
                        let inner_y = m.row.saturating_sub(sr.y).saturating_sub(1);
                        if slash_panel_visible(&g.input_buffer) && !slash_filtered.is_empty() {
                            let n_show = slash_filtered.len().min(SLASH_PANEL_MAX_ROWS);
                            let max_scroll = slash_filtered.len().saturating_sub(n_show);
                            let list_scroll = g
                                .slash_menu_index
                                .saturating_sub(n_show.saturating_sub(1))
                                .min(max_scroll);
                            if (inner_y as usize) < n_show {
                                let idx = list_scroll + inner_y as usize;
                                if idx < slash_filtered.len() {
                                    g.input_buffer = slash_filtered[idx].command_str();
                                    g.cursor_char_idx = g.input_buffer.chars().count();
                                    g.slash_menu_index = idx;
                                }
                            }
                        } else if !at_matches.is_empty() {
                            let n_show = at_matches.len().min(SLASH_PANEL_MAX_ROWS);
                            let max_scroll = at_matches.len().saturating_sub(n_show);
                            let pick = g.at_menu_index.min(at_matches.len().saturating_sub(1));
                            let list_scroll = pick
                                .saturating_sub(n_show.saturating_sub(1))
                                .min(max_scroll);
                            if (inner_y as usize) < n_show {
                                let idx = list_scroll + inner_y as usize;
                                if let Some(choice) = at_matches.get(idx) {
                                    let cur = g.cursor_char_idx;
                                    let (buf, cidx) =
                                        apply_at_completion(&g.input_buffer, cur, choice);
                                    g.input_buffer = buf;
                                    g.cursor_char_idx = cidx;
                                }
                            }
                        }
                    }

                    // Check click on branch chip in status bar.
                    if let Some(bounds) = g.branch_chip_bounds
                        && rect_contains(bounds, m.column, m.row)
                        && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
                    {
                        let _ = cmd_tx.send(TuiCmd::OpenBranchPicker);
                    }
                }
                Event::Key(key) => {
                    if g.command_palette_open {
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                                g.command_palette_open = false;
                                g.command_palette_query.clear();
                                g.palette_index = 0;
                            }
                            (KeyCode::Up, _) if g.palette_index > 0 => {
                                g.palette_index -= 1;
                            }
                            (KeyCode::Down, _) => {
                                let filtered = filter_palette_rows(&g.command_palette_query);
                                let selectable = palette_selectable_indices(&filtered);
                                if !selectable.is_empty() {
                                    g.palette_index = (g.palette_index + 1)
                                        .min(selectable.len().saturating_sub(1));
                                }
                            }
                            (KeyCode::Enter, _) => {
                                let filtered = filter_palette_rows(&g.command_palette_query);
                                let selectable = palette_selectable_indices(&filtered);
                                let pick = g.palette_index.min(selectable.len().saturating_sub(1));
                                if let Some(&abs_idx) = selectable.get(pick)
                                    && let PaletteRow::Entry { label, .. } = filtered[abs_idx]
                                {
                                    let cmd = palette_command_for_label(label);
                                    g.input_buffer = cmd.to_string();
                                    g.cursor_char_idx = g.input_buffer.chars().count();
                                }
                                g.command_palette_open = false;
                                g.command_palette_query.clear();
                                g.palette_index = 0;
                            }
                            (KeyCode::Backspace, _) => {
                                g.command_palette_query.pop();
                                let filtered = filter_palette_rows(&g.command_palette_query);
                                let selectable = palette_selectable_indices(&filtered);
                                g.palette_index =
                                    g.palette_index.min(selectable.len().saturating_sub(1));
                            }
                            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                g.command_palette_query.push(c);
                                let filtered = filter_palette_rows(&g.command_palette_query);
                                let selectable = palette_selectable_indices(&filtered);
                                g.palette_index =
                                    g.palette_index.min(selectable.len().saturating_sub(1));
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Info modal (read-only scrollable popup).
                    if g.info_modal_open {
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                                g.close_info_modal();
                            }
                            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                                g.info_modal_scroll = g.info_modal_scroll.saturating_sub(1);
                            }
                            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                                let max_vis = 16usize;
                                let max_scroll = g.info_modal_lines.len().saturating_sub(max_vis);
                                g.info_modal_scroll = (g.info_modal_scroll + 1).min(max_scroll);
                            }
                            (KeyCode::Home, _) => {
                                g.info_modal_scroll = 0;
                            }
                            (KeyCode::End, _) => {
                                let max_vis = 16usize;
                                g.info_modal_scroll =
                                    g.info_modal_lines.len().saturating_sub(max_vis);
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Model picker popup.
                    if g.model_picker_open {
                        let filter = g.model_picker_search.to_ascii_lowercase();
                        let selectable_count = g
                            .model_picker_entries
                            .iter()
                            .filter(|e| {
                                !e.is_header
                                    && (filter.is_empty()
                                        || e.label.to_ascii_lowercase().contains(&filter)
                                        || e.detail.to_ascii_lowercase().contains(&filter))
                            })
                            .count();
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) => {
                                g.close_model_picker();
                            }
                            (KeyCode::Up, _) if selectable_count > 0 => {
                                g.model_picker_index = g
                                    .model_picker_index
                                    .saturating_sub(1)
                                    .min(selectable_count - 1);
                            }
                            (KeyCode::Down, _) if selectable_count > 0 => {
                                g.model_picker_index =
                                    (g.model_picker_index + 1).min(selectable_count - 1);
                            }
                            (KeyCode::Enter, _) => {
                                let selectable: Vec<&ModelPickerEntry> = g
                                    .model_picker_entries
                                    .iter()
                                    .filter(|e| {
                                        !e.is_header
                                            && (filter.is_empty()
                                                || e.label.to_ascii_lowercase().contains(&filter)
                                                || e.detail.to_ascii_lowercase().contains(&filter))
                                    })
                                    .collect();
                                let pick =
                                    g.model_picker_index.min(selectable.len().saturating_sub(1));
                                if let Some(entry) = selectable.get(pick) {
                                    let action = entry.action.clone();
                                    g.close_model_picker();
                                    drop(g);
                                    match action {
                                        ModelPickerAction::SwitchProvider(p) => {
                                            let _ = cmd_tx.send(TuiCmd::ApplyModelProvider(p));
                                        }
                                        ModelPickerAction::ApplyModel(m) => {
                                            let _ = cmd_tx.send(TuiCmd::ApplyModel(m));
                                        }
                                    }
                                }
                            }
                            (KeyCode::Backspace, _) => {
                                g.model_picker_search.pop();
                                g.model_picker_index = 0;
                                g.model_picker_scroll = 0;
                            }
                            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                g.model_picker_search.push(c);
                                g.model_picker_index = 0;
                                g.model_picker_scroll = 0;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Connect provider (OpenCode-style `/connect`).
                    if g.connect_modal_open {
                        let rows = build_connect_rows(&g.connect_search);
                        let n_sel = selectable_row_indices(&rows).len();
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) if !g.onboarding_mode => {
                                g.close_connect_modal();
                            }
                            (KeyCode::Up, _) if n_sel > 0 => {
                                g.connect_menu_index =
                                    g.connect_menu_index.saturating_sub(1).min(n_sel - 1);
                            }
                            (KeyCode::Down, _) if n_sel > 0 => {
                                g.connect_menu_index = (g.connect_menu_index + 1).min(n_sel - 1);
                            }
                            (KeyCode::Enter, _) => {
                                if let Some(p) = provider_at_selection(&rows, g.connect_menu_index)
                                {
                                    g.close_connect_modal();
                                    drop(g);
                                    let _ = cmd_tx.send(TuiCmd::PromptApiKey(p, true));
                                }
                            }
                            (KeyCode::Backspace, _) => {
                                g.connect_search.pop();
                                g.connect_menu_index = 0;
                                g.connect_modal_scroll = 0;
                                let rows2 = build_connect_rows(&g.connect_search);
                                g.connect_menu_index =
                                    clamp_selection(g.connect_menu_index, &rows2);
                            }
                            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                g.connect_search.push(c);
                                g.connect_menu_index = 0;
                                g.connect_modal_scroll = 0;
                                let rows2 = build_connect_rows(&g.connect_search);
                                g.connect_menu_index =
                                    clamp_selection(g.connect_menu_index, &rows2);
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if g.api_key_modal_open {
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) => {
                                g.close_api_key_modal();
                                if g.onboarding_mode {
                                    // Go back to connect modal instead of closing entirely
                                    g.open_connect_modal();
                                }
                            }
                            (KeyCode::Enter, _) => {
                                if g.onboarding_mode {
                                    // Block input while validation is in flight
                                    if matches!(
                                        g.validation_status,
                                        Some(crate::tui::state::OnboardingValidation::Validating)
                                    ) {
                                        // Already validating — ignore
                                    } else if let Some(provider) = g.api_key_target_provider {
                                        let key = g.api_key_input.trim().to_string();
                                        if key.is_empty() {
                                            // Don't submit empty keys during onboarding
                                        } else {
                                            g.validation_status = Some(
                                                crate::tui::state::OnboardingValidation::Validating,
                                            );
                                            drop(g);
                                            let _ =
                                                cmd_tx.send(TuiCmd::ValidateApiKey(provider, key));
                                        }
                                    }
                                } else {
                                    drop(g);
                                    let _ = cmd_tx.send(TuiCmd::Submit(String::new()));
                                }
                            }
                            (KeyCode::Backspace, _) => {
                                g.api_key_input.pop();
                                if g.onboarding_mode {
                                    g.validation_status = None;
                                }
                            }
                            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                g.api_key_input.push(c);
                                if g.onboarding_mode {
                                    g.validation_status = None; // Clear stale error on new input
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Provider picker (settings).
                    if g.provider_picker_open {
                        let n = ProviderKind::ALL.len();
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) => {
                                g.close_provider_picker();
                            }
                            (KeyCode::Up, _) => {
                                g.provider_picker_index = g.provider_picker_index.saturating_sub(1);
                            }
                            (KeyCode::Down, _) if n > 0 => {
                                g.provider_picker_index = (g.provider_picker_index + 1) % n;
                            }
                            (KeyCode::Enter, _) => {
                                if n == 0 {
                                    g.close_provider_picker();
                                    continue;
                                }
                                let p = ProviderKind::ALL[g.provider_picker_index.min(n - 1)];
                                let for_key = g.provider_picker_for_api_key;
                                g.close_provider_picker();
                                if for_key {
                                    drop(g);
                                    let _ = cmd_tx.send(TuiCmd::PromptApiKey(p, false));
                                } else {
                                    drop(g);
                                    let _ = cmd_tx.send(TuiCmd::ApplyDefaultProvider(p));
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Branch picker keyboard handling.
                    if g.branch_picker_open {
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) => {
                                g.close_branch_picker();
                            }
                            (KeyCode::Up, _)
                                if !filtered_branch_indices(
                                    &g.branch_picker_branches,
                                    &g.branch_picker_query,
                                )
                                .is_empty() =>
                            {
                                g.branch_picker_index = g.branch_picker_index.saturating_sub(1);
                            }
                            (KeyCode::Down, _) => {
                                let n = filtered_branch_indices(
                                    &g.branch_picker_branches,
                                    &g.branch_picker_query,
                                )
                                .len();
                                if n > 0 {
                                    g.branch_picker_index = (g.branch_picker_index + 1).min(n - 1);
                                }
                            }
                            (KeyCode::Enter, _) => {
                                let cmd = branch_picker_enter_command(
                                    &g.branch_picker_branches,
                                    &g.branch_picker_query,
                                    g.branch_picker_index,
                                );
                                g.close_branch_picker();
                                if let Some(c) = cmd {
                                    drop(g);
                                    let _ = cmd_tx.send(c);
                                }
                            }
                            (KeyCode::Backspace, _) => {
                                g.branch_picker_query.pop();
                                let filtered = filtered_branch_indices(
                                    &g.branch_picker_branches,
                                    &g.branch_picker_query,
                                );
                                g.branch_picker_index =
                                    g.branch_picker_index.min(filtered.len().saturating_sub(1));
                            }
                            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                g.branch_picker_query.push(c);
                                g.branch_picker_index = 0;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Question modal keyboard handling.
                    if g.question_modal_open {
                        if let Some(ref q) = g.active_question.clone() {
                            // Total items: 1 (suggested) + options.len() + (1 if allow_custom for "Chat about this")
                            let total = 1 + q.options.len() + if q.allow_custom { 1 } else { 0 };
                            match (key.code, key.modifiers) {
                                (KeyCode::Esc, _) if q.allow_custom => {
                                    // Fall back to inline text input
                                    g.close_question_modal();
                                }
                                // If !allow_custom, Esc is a no-op
                                (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                                    g.question_modal_index =
                                        g.question_modal_index.saturating_sub(1);
                                }
                                (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                                    g.question_modal_index =
                                        (g.question_modal_index + 1).min(total - 1);
                                }
                                (KeyCode::Enter, _) => {
                                    let idx = g.question_modal_index;
                                    let sel = if idx == 0 {
                                        // Suggested answer
                                        Some(QuestionSelection::Suggested)
                                    } else if idx <= q.options.len() {
                                        // Regular option (1-based → 0-based)
                                        Some(QuestionSelection::Option {
                                            option_id: q.options[idx - 1].id.clone(),
                                        })
                                    } else {
                                        // "Chat about this" — fall back to inline text input
                                        None
                                    };

                                    if let Some(sel) = sel {
                                        let qid = q.question_id.clone();
                                        g.close_question_modal();
                                        g.active_question = None;
                                        drop(g);
                                        if let Some(ref tx) = question_answer_tx {
                                            let _ = tx.send((qid, sel));
                                        } else {
                                            let _ = cmd_tx.send(TuiCmd::QuestionAnswer(sel));
                                        }
                                    } else {
                                        // "Chat about this" — close modal, keep active_question
                                        g.close_question_modal();
                                    }
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }

                    // Permission picker keyboard handling.
                    if g.permission_picker_open {
                        const PERM_COUNT: usize = 5;
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) => {
                                g.close_permission_picker();
                            }
                            (KeyCode::Up, _) => {
                                g.permission_picker_index =
                                    g.permission_picker_index.saturating_sub(1);
                            }
                            (KeyCode::Down, _) => {
                                g.permission_picker_index =
                                    (g.permission_picker_index + 1).min(PERM_COUNT - 1);
                            }
                            (KeyCode::Enter, _) => {
                                let idx = g.permission_picker_index;
                                g.close_permission_picker();
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::ApplyPermission(idx));
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Agent profile picker keyboard handling.
                    if g.agent_picker_open {
                        const AGENT_COUNT: usize = 5;
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) => {
                                g.close_agent_picker();
                            }
                            (KeyCode::Up, _) => {
                                g.agent_picker_index = g.agent_picker_index.saturating_sub(1);
                            }
                            (KeyCode::Down, _) => {
                                g.agent_picker_index =
                                    (g.agent_picker_index + 1).min(AGENT_COUNT - 1);
                            }
                            (KeyCode::Enter, _) => {
                                let idx = g.agent_picker_index;
                                g.close_agent_picker();
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::SwitchAgent(idx));
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Session picker keyboard handling.
                    if g.session_picker_open {
                        let filter = g.session_picker_search.to_ascii_lowercase();
                        let count = g
                            .session_picker_entries
                            .iter()
                            .filter(|s| {
                                filter.is_empty()
                                    || s.id.to_ascii_lowercase().contains(&filter)
                                    || s.session_title
                                        .as_ref()
                                        .map(|t| t.to_ascii_lowercase().contains(&filter))
                                        .unwrap_or(false)
                            })
                            .count();
                        match (key.code, key.modifiers) {
                            (KeyCode::Esc, _) => {
                                g.close_session_picker();
                            }
                            (KeyCode::Up, _) => {
                                g.session_picker_index = g.session_picker_index.saturating_sub(1);
                            }
                            (KeyCode::Down, _) if count > 0 => {
                                g.session_picker_index =
                                    (g.session_picker_index + 1).min(count.saturating_sub(1));
                            }
                            (KeyCode::Enter, _) => {
                                let filtered: Vec<&nca_common::session::SessionSnapshot> = g
                                    .session_picker_entries
                                    .iter()
                                    .filter(|s| {
                                        filter.is_empty()
                                            || s.id.to_ascii_lowercase().contains(&filter)
                                            || s.session_title
                                                .as_ref()
                                                .map(|t| t.to_ascii_lowercase().contains(&filter))
                                                .unwrap_or(false)
                                    })
                                    .collect();
                                let pick =
                                    g.session_picker_index.min(filtered.len().saturating_sub(1));
                                if let Some(snap) = filtered.get(pick) {
                                    let id = snap.id.clone();
                                    g.close_session_picker();
                                    drop(g);
                                    let _ = cmd_tx.send(TuiCmd::ResumeSession(id));
                                }
                            }
                            (KeyCode::Backspace, _) => {
                                g.session_picker_search.pop();
                                g.session_picker_index = 0;
                                g.session_picker_scroll = 0;
                            }
                            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                g.session_picker_search.push(c);
                                g.session_picker_index = 0;
                                g.session_picker_scroll = 0;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Ctrl+X leader key dispatch.
                    if g.leader_pending {
                        g.leader_pending = false;
                        match key.code {
                            KeyCode::Char('m') | KeyCode::Char('M') => {
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::OpenModelPicker);
                            }
                            KeyCode::Char('e') | KeyCode::Char('E') => {
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::OpenEditor);
                            }
                            KeyCode::Char('l') | KeyCode::Char('L') => {
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::OpenSessions);
                            }
                            KeyCode::Char('n') | KeyCode::Char('N') => {
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::NewSession);
                            }
                            KeyCode::Char('c') | KeyCode::Char('C') => {
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::RunCompact);
                            }
                            KeyCode::Char('s') | KeyCode::Char('S') => {
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::OpenStatus);
                            }
                            KeyCode::Char('a') | KeyCode::Char('A') => {
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::OpenAgentPicker);
                            }
                            KeyCode::Char('h') | KeyCode::Char('H') => {
                                drop(g);
                                let _ = cmd_tx.send(TuiCmd::OpenHelp);
                            }
                            KeyCode::Char('q') | KeyCode::Char('Q') => {
                                g.should_exit = true;
                                let _ = cmd_tx.send(TuiCmd::Exit);
                                break;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match (key.code, key.modifiers) {
                        (KeyCode::Esc, _) if escape_cancels_active_turn(&g) => {
                            if let Some(ref flag) = cancel_flag {
                                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            g.blocks
                                .push(DisplayBlock::System("Cancelling current run...".into()));
                            let _ = cmd_tx.send(TuiCmd::CancelTurn);
                        }
                        (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                            g.should_exit = true;
                            let _ = cmd_tx.send(TuiCmd::Exit);
                            break;
                        }
                        (KeyCode::Insert, KeyModifiers::CONTROL | KeyModifiers::SHIFT) => {
                            // Ctrl+Shift+Insert: copy selected transcript text to clipboard.
                            if let Some((sel_start, sel_end)) = g.transcript_selection {
                                let sz = terminal.size().ok();
                                if let Some(sz) = sz {
                                    let area = Rect::new(0, 0, sz.width, sz.height);
                                    let (main_area, _) = layout_with_sidebar(area);
                                    let inner_w = main_area.width.saturating_sub(2);
                                    let lines = transcript_lines(&g, inner_w);
                                    let text = plain_text_from_lines(&lines, sel_start, sel_end);
                                    let n = text.trim_end_matches('\n').chars().count();
                                    match crate::image_attach::copy_text_to_clipboard(&text) {
                                        Ok(()) => {
                                            g.blocks.push(DisplayBlock::System(format!(
                                                "Copied {n} chars to clipboard"
                                            )));
                                        }
                                        Err(e) => {
                                            g.push_error(format!("clipboard copy failed: {e}"));
                                        }
                                    }
                                }
                            } else {
                                g.blocks.push(DisplayBlock::System(
                                    "No text selected — drag in transcript to select, then Ctrl+Shift+C to copy".into(),
                                ));
                            }
                        }
                        (KeyCode::Char('c'), KeyModifiers::CONTROL)
                            if !key.modifiers.contains(KeyModifiers::SHIFT) =>
                        {
                            // Ctrl+C: cancel current turn.
                            if let Some(ref flag) = cancel_flag {
                                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            let _ = cmd_tx.send(TuiCmd::CancelTurn);
                        }
                        (KeyCode::Char('c'), KeyModifiers::CONTROL | KeyModifiers::SHIFT) => {
                            // Ctrl+Shift+C: copy selected transcript text to clipboard.
                            if let Some((sel_start, sel_end)) = g.transcript_selection {
                                let sz = terminal.size().ok();
                                if let Some(sz) = sz {
                                    let area = Rect::new(0, 0, sz.width, sz.height);
                                    let (main_area, _) = layout_with_sidebar(area);
                                    let inner_w = main_area.width.saturating_sub(2);
                                    let lines = transcript_lines(&g, inner_w);
                                    let text = plain_text_from_lines(&lines, sel_start, sel_end);
                                    let n = text.trim_end_matches('\n').chars().count();
                                    match crate::image_attach::copy_text_to_clipboard(&text) {
                                        Ok(()) => {
                                            g.blocks.push(DisplayBlock::System(format!(
                                                "Copied {n} chars to clipboard"
                                            )));
                                        }
                                        Err(e) => {
                                            g.push_error(format!("clipboard copy failed: {e}"));
                                        }
                                    }
                                }
                            } else {
                                g.blocks.push(DisplayBlock::System(
                                    "No text selected — drag in transcript to select, then Ctrl+Shift+C to copy".into(),
                                ));
                            }
                        }
                        (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                            g.blocks.clear();
                            g.streaming_assistant = None;
                            g.scroll_lines = 0;
                            g.transcript_follow_tail = true;
                        }
                        (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                            g.command_palette_open = true;
                            g.command_palette_query.clear();
                            g.palette_index = 0;
                        }
                        (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
                            g.leader_pending = true;
                        }
                        (KeyCode::Char('v'), KeyModifiers::CONTROL) => {
                            if g.active_approval.is_some() || g.active_question.is_some() {
                                continue;
                            }
                            let ws = g.workspace_root.clone();
                            let sid = g.session_id.clone();
                            match crate::image_attach::paste_clipboard_image(&ws, &sid) {
                                Ok(att) => {
                                    let label = att.path.clone();
                                    g.staged_image_attachments.push(att);
                                    g.blocks.push(DisplayBlock::System(format!(
                                        "[image] staged {label} — Enter to send"
                                    )));
                                }
                                Err(e) => g.push_error(format!("[image] {e}")),
                            }
                        }
                        (KeyCode::Tab, _) => {
                            if let Some((buf, cidx)) = apply_selected_at_completion(
                                &workspace_files,
                                &g.input_buffer,
                                g.cursor_char_idx,
                                g.at_menu_index,
                                false,
                            ) {
                                g.input_buffer = buf;
                                g.cursor_char_idx = cidx;
                            } else {
                                let slash_filtered =
                                    filter_slash_entries(&slash_entries, &g.input_buffer);
                                if !slash_filtered.is_empty()
                                    && slash_panel_visible(&g.input_buffer)
                                {
                                    let pick = g.slash_menu_index % slash_filtered.len();
                                    g.input_buffer = slash_filtered[pick].command_str();
                                    g.cursor_char_idx = g.input_buffer.chars().count();
                                } else {
                                    drop(g);
                                    let _ = cmd_tx.send(TuiCmd::CycleAgent);
                                }
                            }
                        }
                        (KeyCode::F(2), KeyModifiers::NONE) => {
                            drop(g);
                            let _ = cmd_tx.send(TuiCmd::CycleModel(true));
                        }
                        (KeyCode::F(2), KeyModifiers::SHIFT) => {
                            drop(g);
                            let _ = cmd_tx.send(TuiCmd::CycleModel(false));
                        }
                        (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                            if let Some(req) = g.active_approval.clone() {
                                let call_id = req.call_id.clone();
                                g.input_buffer.clear();
                                g.cursor_char_idx = 0;
                                drop(g);
                                if let Some(ref tx) = approval_answer_tx {
                                    let _ = tx.send(ApprovalAnswer::Verdict {
                                        call_id,
                                        approved: true,
                                    });
                                }
                                continue;
                            }
                        }
                        (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                            if let Some(req) = g.active_approval.clone() {
                                let call_id = req.call_id.clone();
                                g.input_buffer.clear();
                                g.cursor_char_idx = 0;
                                drop(g);
                                if let Some(ref tx) = approval_answer_tx {
                                    let _ = tx.send(ApprovalAnswer::Verdict {
                                        call_id,
                                        approved: false,
                                    });
                                }
                                continue;
                            }
                        }
                        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                            if let Some(req) = g.active_approval.clone() {
                                let input_json: serde_json::Value =
                                    serde_json::from_str(&req.input).unwrap_or_default();
                                let pattern = suggest_allow_pattern(&req.tool, &input_json);
                                let call_id = req.call_id.clone();
                                g.input_buffer.clear();
                                g.cursor_char_idx = 0;
                                g.blocks.push(DisplayBlock::System(format!(
                                    "Always allowing: {pattern}"
                                )));
                                drop(g);
                                if let Some(ref tx) = approval_answer_tx {
                                    let _ =
                                        tx.send(ApprovalAnswer::AllowPattern { call_id, pattern });
                                }
                                continue;
                            }
                        }
                        (KeyCode::Enter, _) => {
                            if let Some((buf, cidx)) = apply_selected_at_completion(
                                &workspace_files,
                                &g.input_buffer,
                                g.cursor_char_idx,
                                g.at_menu_index,
                                true,
                            ) {
                                g.input_buffer = buf;
                                g.cursor_char_idx = cidx;
                                continue;
                            }
                            let line = std::mem::take(&mut g.input_buffer);
                            g.cursor_char_idx = 0;
                            g.slash_menu_index = 0;
                            let active_approval = g.active_approval.clone();
                            let active_q = g.active_question.clone();
                            if let Some(req) = active_approval {
                                let t = line.trim();
                                if t.is_empty() {
                                    g.blocks.push(DisplayBlock::System(
                                        "Empty line — type y or n (or yes/no, ok, deny). Ctrl+Y = approve, Ctrl+N = deny."
                                            .into(),
                                    ));
                                    continue;
                                }
                                if t.starts_with('/') {
                                    let lower = t.to_lowercase();
                                    let slash_verdict = match lower.as_str() {
                                        "/approve" | "/y" | "/yes" | "/ok" => Some(true),
                                        "/deny" | "/n" | "/no" => Some(false),
                                        _ => None,
                                    };
                                    if let Some(approved) = slash_verdict {
                                        let call_id = req.call_id.clone();
                                        drop(g);
                                        if let Some(ref tx) = approval_answer_tx {
                                            let _ = tx.send(ApprovalAnswer::Verdict {
                                                call_id,
                                                approved,
                                            });
                                        } else {
                                            let _ = cmd_tx.send(TuiCmd::CancelTurn);
                                        }
                                        continue;
                                    }
                                    drop(g);
                                    let _ = cmd_tx.send(TuiCmd::Submit(line));
                                    continue;
                                }
                                if let Some(approved) = parse_approval_verdict(t) {
                                    let call_id = req.call_id.clone();
                                    drop(g);
                                    if let Some(ref tx) = approval_answer_tx {
                                        let _ =
                                            tx.send(ApprovalAnswer::Verdict { call_id, approved });
                                    } else {
                                        let _ = cmd_tx.send(TuiCmd::CancelTurn);
                                    }
                                    continue;
                                }
                                g.blocks.push(DisplayBlock::System(
                                    "Could not parse approval — try y, n, yes, no, ok, deny, or Ctrl+Y / Ctrl+N."
                                        .into(),
                                ));
                                continue;
                            }
                            if let Some(ref q) = active_q {
                                let t = line.trim();
                                // `/auto-answer` must go through the side channel: `run_turn` is often
                                // blocked on this question, so `cmd_rx` is not polled for Submit.
                                if t == "/auto-answer" {
                                    let qid = q.question_id.clone();
                                    drop(g);
                                    if let Some(ref tx) = question_answer_tx {
                                        let _ = tx.send((qid, QuestionSelection::Suggested));
                                    } else {
                                        let _ = cmd_tx.send(TuiCmd::QuestionAnswer(
                                            QuestionSelection::Suggested,
                                        ));
                                    }
                                    continue;
                                }
                                if t.starts_with('/') {
                                    drop(g);
                                    let _ = cmd_tx.send(TuiCmd::Submit(line));
                                    continue;
                                }
                                if let Some(sel) = parse_tui_question_answer(&line, q) {
                                    let qid = q.question_id.clone();
                                    drop(g);
                                    if let Some(ref tx) = question_answer_tx {
                                        let _ = tx.send((qid, sel));
                                    } else {
                                        let _ = cmd_tx.send(TuiCmd::QuestionAnswer(sel));
                                    }
                                    continue;
                                }
                                g.blocks.push(DisplayBlock::System(
                                    "Invalid answer: use Enter/0 for suggested, 1–n for an option, or custom text."
                                        .into(),
                                ));
                                continue;
                            }
                            drop(g);
                            let _ = cmd_tx.send(TuiCmd::Submit(line));
                        }
                        (KeyCode::Char('a'), KeyModifiers::CONTROL) | (KeyCode::Home, _) => {
                            g.cursor_char_idx = 0;
                        }
                        (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                            g.cursor_char_idx = g.input_buffer.chars().count();
                        }
                        (KeyCode::End, _) => {
                            if !g.input_buffer.is_empty() {
                                g.cursor_char_idx = g.input_buffer.chars().count();
                            } else {
                                let sz = terminal.size().ok();
                                if let Some(sz) = sz {
                                    let area = Rect::new(0, 0, sz.width, sz.height);
                                    let (main_area, _) = layout_with_sidebar(area);
                                    let sh = composer_chrome_height(
                                        &slash_entries,
                                        &workspace_files,
                                        &g.input_buffer,
                                        g.cursor_char_idx,
                                    );
                                    let (tr, _, _, _) = layout_chunks(main_area, sh);
                                    let total =
                                        transcript_lines(&g, tr.width.saturating_sub(2)).len();
                                    let th = tr.height.saturating_sub(2) as usize;
                                    let max_scroll = total.saturating_sub(th);
                                    g.transcript_follow_tail = true;
                                    g.scroll_lines = max_scroll;
                                }
                            }
                        }
                        (KeyCode::Left, _) => {
                            g.cursor_char_idx = g.cursor_char_idx.saturating_sub(1);
                        }
                        (KeyCode::Right, _) => {
                            let max = g.input_buffer.chars().count();
                            g.cursor_char_idx = (g.cursor_char_idx + 1).min(max);
                        }
                        (KeyCode::Up, _) => {
                            let at_matches = at_completion_matches(
                                &workspace_files,
                                &g.input_buffer,
                                g.cursor_char_idx,
                            );
                            if !at_matches.is_empty()
                                && at_completion_active(&g.input_buffer, g.cursor_char_idx)
                            {
                                g.at_menu_index = g.at_menu_index.saturating_sub(1);
                            } else {
                                let slash_filtered =
                                    filter_slash_entries(&slash_entries, &g.input_buffer);
                                if !slash_filtered.is_empty()
                                    && slash_panel_visible(&g.input_buffer)
                                {
                                    g.slash_menu_index = g.slash_menu_index.saturating_sub(1);
                                } else {
                                    g.transcript_follow_tail = false;
                                    g.scroll_lines = g.scroll_lines.saturating_sub(1);
                                }
                            }
                        }
                        (KeyCode::Down, _) => {
                            let at_matches = at_completion_matches(
                                &workspace_files,
                                &g.input_buffer,
                                g.cursor_char_idx,
                            );
                            if !at_matches.is_empty()
                                && at_completion_active(&g.input_buffer, g.cursor_char_idx)
                            {
                                let n = at_matches.len();
                                g.at_menu_index = (g.at_menu_index + 1) % n;
                            } else {
                                let slash_filtered =
                                    filter_slash_entries(&slash_entries, &g.input_buffer);
                                if !slash_filtered.is_empty()
                                    && slash_panel_visible(&g.input_buffer)
                                {
                                    let n = slash_filtered.len();
                                    g.slash_menu_index = (g.slash_menu_index + 1) % n;
                                } else {
                                    let sz = terminal.size().ok();
                                    if let Some(sz) = sz {
                                        let area = Rect::new(0, 0, sz.width, sz.height);
                                        let (main_area, _) = layout_with_sidebar(area);
                                        let sh = composer_chrome_height(
                                            &slash_entries,
                                            &workspace_files,
                                            &g.input_buffer,
                                            g.cursor_char_idx,
                                        );
                                        let (tr, _, _, _) = layout_chunks(main_area, sh);
                                        let lines =
                                            transcript_lines(&g, tr.width.saturating_sub(2));
                                        let total = lines.len();
                                        let th = tr.height.saturating_sub(2) as usize;
                                        let max_scroll = total.saturating_sub(th);
                                        g.scroll_lines = (g.scroll_lines + 1).min(max_scroll);
                                        if g.scroll_lines >= max_scroll {
                                            g.transcript_follow_tail = true;
                                        }
                                    }
                                }
                            }
                        }
                        (KeyCode::Backspace, _) if g.cursor_char_idx > 0 => {
                            if let Some((buf, cidx)) =
                                delete_completed_at_mention(&g.input_buffer, g.cursor_char_idx)
                            {
                                g.input_buffer = buf;
                                g.cursor_char_idx = cidx;
                            } else {
                                let idx = g.cursor_char_idx;
                                let mut cs: Vec<char> = g.input_buffer.chars().collect();
                                cs.remove(idx - 1);
                                g.input_buffer = cs.into_iter().collect();
                                g.cursor_char_idx -= 1;
                            }
                            if slash_panel_visible(&g.input_buffer) {
                                let f = filter_slash_entries(&slash_entries, &g.input_buffer);
                                if !f.is_empty() {
                                    g.slash_menu_index =
                                        g.slash_menu_index.min(f.len().saturating_sub(1));
                                } else {
                                    g.slash_menu_index = 0;
                                }
                            }
                        }
                        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                            let idx = g.cursor_char_idx;
                            let mut cs: Vec<char> = g.input_buffer.chars().collect();
                            cs.insert(idx, c);
                            g.input_buffer = cs.into_iter().collect();
                            g.cursor_char_idx += 1;
                            if slash_panel_visible(&g.input_buffer) {
                                let f = filter_slash_entries(&slash_entries, &g.input_buffer);
                                if !f.is_empty() {
                                    g.slash_menu_index =
                                        g.slash_menu_index.min(f.len().saturating_sub(1));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    restore_terminal();
    let _ = execute!(stdout(), MoveToColumn(0));
    Ok(())
}

#[cfg(test)]
mod approval_parse_tests {
    use super::{
        TuiCmd, apply_selected_at_completion, branch_picker_enter_command,
        completed_at_mention_range_before_cursor, composer_line, delete_completed_at_mention,
        escape_cancels_active_turn, filtered_branch_indices, parse_approval_verdict,
    };
    use crate::tui::state::TuiSessionState;
    use nca_common::event::BusyState;
    use std::path::PathBuf;

    #[test]
    fn parses_yes_with_punctuation_and_synonyms() {
        assert_eq!(parse_approval_verdict("yes"), Some(true));
        assert_eq!(parse_approval_verdict("Yes."), Some(true));
        assert_eq!(parse_approval_verdict("  OK! "), Some(true));
        assert_eq!(parse_approval_verdict("approve"), Some(true));
        assert_eq!(parse_approval_verdict("/approve"), Some(true));
        assert_eq!(parse_approval_verdict("/y"), Some(true));
    }

    #[test]
    fn parses_no_and_deny() {
        assert_eq!(parse_approval_verdict("n"), Some(false));
        assert_eq!(parse_approval_verdict("no."), Some(false));
        assert_eq!(parse_approval_verdict("deny"), Some(false));
        assert_eq!(parse_approval_verdict("/deny"), Some(false));
    }

    #[test]
    fn rejects_unknown() {
        assert_eq!(parse_approval_verdict("maybe"), None);
        assert_eq!(parse_approval_verdict("nope"), None);
        assert_eq!(parse_approval_verdict(""), None);
    }

    #[test]
    fn branch_picker_switches_exact_match_from_typed_query() {
        let branches = vec![
            "interactive-question".into(),
            "main".into(),
            "self-autoresearch".into(),
        ];
        let cmd = branch_picker_enter_command(&branches, "main", 0);
        assert!(matches!(cmd, Some(TuiCmd::SwitchBranch(name)) if name == "main"));
    }

    #[test]
    fn branch_picker_creates_only_with_slash_prefix() {
        let branches = vec!["main".into()];
        let cmd = branch_picker_enter_command(&branches, "/feature-x", 0);
        assert!(matches!(cmd, Some(TuiCmd::CreateBranch(name)) if name == "feature-x"));
    }

    #[test]
    fn branch_picker_filters_case_insensitively() {
        let branches = vec!["Main".into(), "feature/login".into()];
        assert_eq!(filtered_branch_indices(&branches, "main"), vec![0]);
        assert_eq!(filtered_branch_indices(&branches, "LOGIN"), vec![1]);
    }

    #[test]
    fn branch_picker_switches_selected_filtered_branch_by_name() {
        let branches = vec!["alpha".into(), "main".into(), "main-fix".into()];
        let cmd = branch_picker_enter_command(&branches, "mai", 1);
        assert!(matches!(cmd, Some(TuiCmd::SwitchBranch(name)) if name == "main-fix"));
    }

    #[test]
    fn enter_accepts_selected_at_mention_without_submitting() {
        let workspace_files = vec![
            "crates/cli/src/file_mentions.rs".into(),
            "crates/cli/src/tui/app.rs".into(),
        ];
        let buffer = "check @crates/cli/src/t";
        let cursor_char_idx = buffer.chars().count();

        let (next_buffer, next_cursor_char_idx) =
            apply_selected_at_completion(&workspace_files, buffer, cursor_char_idx, 0, true)
                .expect("active mention should be selectable");

        assert_eq!(next_buffer, "check @crates/cli/src/tui/app.rs ");
        assert_eq!(next_cursor_char_idx, next_buffer.chars().count());
    }

    #[test]
    fn backspace_deletes_completed_at_mention_and_space() {
        let buffer = "check @crates/cli/src/tui/app.rs ";
        let cursor_char_idx = buffer.chars().count();

        let (next_buffer, next_cursor_char_idx) =
            delete_completed_at_mention(buffer, cursor_char_idx)
                .expect("completed mention should delete as one token");

        assert_eq!(next_buffer, "check ");
        assert_eq!(next_cursor_char_idx, "check ".chars().count());
    }

    #[test]
    fn mention_range_includes_inserted_trailing_space() {
        let buffer = "check @crates/cli/src/tui/app.rs ";
        let cursor_char_idx = buffer.chars().count();

        assert_eq!(
            completed_at_mention_range_before_cursor(buffer, cursor_char_idx),
            Some((6, buffer.chars().count()))
        );
    }

    #[test]
    fn composer_line_styles_completed_mentions() {
        let line = composer_line("see @README.md ", 15, 80);
        let mention_span = line
            .spans
            .iter()
            .find(|span| span.content.contains("@README.md"))
            .expect("mention span should exist");

        assert_eq!(mention_span.style.bg, Some(super::theme::MENTION_BG));
    }

    #[test]
    fn escape_only_cancels_active_turn_states() {
        let mut state = TuiSessionState::new(
            "session".into(),
            "model".into(),
            "@build".into(),
            "AcceptEdits".into(),
            PathBuf::from("."),
        );
        assert!(!escape_cancels_active_turn(&state));

        state.set_busy_state(BusyState::Thinking);
        assert!(escape_cancels_active_turn(&state));

        state.set_busy_state(BusyState::Streaming);
        assert!(escape_cancels_active_turn(&state));

        state.set_busy_state(BusyState::ToolRunning);
        assert!(escape_cancels_active_turn(&state));

        state.set_busy_state(BusyState::ApprovalPending);
        assert!(!escape_cancels_active_turn(&state));
    }
}
