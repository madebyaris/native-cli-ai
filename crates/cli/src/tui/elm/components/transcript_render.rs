//! Transcript rendering helpers — pure functions that emit `Line`s from blocks,
//! streaming text, and handle text-selection highlighting.
//!
//! Split from `transcript.rs` to keep state management and rendering separate.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::format::format_duration;
use crate::tui::state::DisplayBlock;

use super::searchable_list::theme;
use super::transcript::{LineAnswerHit, TranscriptHit, TranscriptState};

// ── Text helpers ──────────────────────────────────────────────────

pub(super) fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(1)
}

pub(super) fn wrap_text(s: &str, width: usize) -> Vec<String> {
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

pub(super) fn wrap_preformatted(text: &str, _width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        out.push(line.to_string());
    }
    out
}

pub(super) fn parse_md_line(line: &str) -> Line<'static> {
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

pub(super) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

/// Compute the display width (columns) of a string, accounting for CJK and tab.
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            if c == '\t' {
                4
            } else {
                unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
            }
        })
        .sum()
}

/// Truncate `s` so that its display width fits within `max_cols`.
fn truncate_to_width(s: &str, max_cols: usize) -> String {
    if display_width(s) <= max_cols {
        return s.to_string();
    }
    let mut result = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = if c == '\t' {
            4
        } else {
            unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
        };
        if w + cw > max_cols.saturating_sub(1) {
            result.push('…');
            break;
        }
        result.push(c);
        w += cw;
    }
    result
}

// ── Line counting ────────────────────────────────────────────────

pub(super) fn block_line_count(block: &DisplayBlock, width: usize) -> usize {
    let w = width.max(20);
    match block {
        DisplayBlock::User(content) => 2 + wrap_text(content, w).len() + 1,
        DisplayBlock::Assistant(content) => 2 + wrap_text(content, w).len() + 1,
        DisplayBlock::Thinking {
            content,
            expanded,
            duration_ms: _,
        } => {
            let all = wrap_text(content, w);
            let total = all.len();
            let preview = 3usize;
            let show = if *expanded || total <= preview {
                total
            } else {
                preview
            };
            let mut n = 1 + show + 1;
            if total > preview {
                n += 1;
            }
            n
        }
        DisplayBlock::ToolRunning { input, .. } => {
            let cmd = serde_json::from_str::<serde_json::Value>(input)
                .ok()
                .and_then(|v| v.get("command").and_then(|v| v.as_str().map(String::from)));
            match cmd {
                Some(c) if !c.is_empty() => 1 + wrap_text(&c, w.saturating_sub(3)).len(),
                _ => 1,
            }
        }
        DisplayBlock::ApprovalPending(req) => {
            2 + wrap_text(&req.description, w).len()
                + 1
                + 1
                + wrap_preformatted_lines_count(&req.input, w)
                + 1
                + 1
        }
        DisplayBlock::ApprovalResolved { .. } => 2,
        DisplayBlock::ToolDone {
            full_output,
            expanded,
            input,
            ..
        } => {
            let all = wrap_text(full_output, w);
            let total = all.len();
            let preview = 3usize;
            let show = if *expanded || total <= preview {
                total
            } else {
                preview
            };
            let mut n = 1 + show;
            if total > preview {
                n += 1;
            }
            let cmd = serde_json::from_str::<serde_json::Value>(input)
                .ok()
                .and_then(|v| v.get("command").and_then(|v| v.as_str().map(String::from)))
                .unwrap_or_default();
            if !cmd.is_empty() {
                n += wrap_text(&cmd, w.saturating_sub(3)).len();
            }
            n + 1
        }
        DisplayBlock::System(s) => wrap_text(s, w).len(),
        DisplayBlock::Question(q) => {
            let mut n = 2 + wrap_text(&q.prompt, w).len() + 1 + q.options.len() + 2;
            if q.allow_custom {
                n += 1;
            }
            n
        }
        DisplayBlock::ErrorLine(_s) => 1, // truncated to width in render
        DisplayBlock::TurnInfo { .. } => 1,
    }
}

fn wrap_preformatted_lines_count(text: &str, width: usize) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut n = 0usize;
    for source_line in text.lines() {
        if source_line.is_empty() {
            n += 1;
        } else {
            let mut w = 0usize;
            let mut has_content = false;
            for ch in source_line.chars() {
                let cw = char_width(ch);
                if w + cw > width {
                    n += 1;
                    w = 0;
                }
                w += cw;
                has_content = true;
            }
            if has_content {
                n += 1;
            }
        }
    }
    n
}

// ── Column truncation helpers ─────────────────────────────────────

pub(super) fn truncate_by_columns(text: &str, max_cols: usize) -> String {
    let mut col = 0usize;
    let mut result = String::new();
    for c in text.chars() {
        if col >= max_cols {
            break;
        }
        let w = if c == '\t' { 4 } else { char_width(c) };
        result.push(c);
        col += w;
    }
    result
}

pub(super) fn truncate_by_columns_skip(text: &str, skip_cols: usize) -> String {
    let mut col = 0usize;
    let mut skipping = true;
    let mut result = String::new();
    for c in text.chars() {
        if skipping {
            let w = if c == '\t' { 4 } else { char_width(c) };
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

pub(super) fn plain_text_from_lines(
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
            let start_col = sc.min(ec);
            let end_col = sc.max(ec);
            let truncated = truncate_by_columns(&full_text, end_col);
            let remaining = truncate_by_columns_skip(&truncated, start_col);
            out.push_str(&remaining);
        } else if global_line == sl {
            out.push_str(&truncate_by_columns_skip(&full_text, sc));
        } else if global_line == el {
            out.push_str(&truncate_by_columns(&full_text, ec));
        } else {
            out.push_str(&full_text);
        }
        if idx < lines[s..e].len() - 1 {
            out.push('\n');
        }
    }
    out
}

// ── Selection highlight ──────────────────────────────────────────

pub(super) fn apply_selection_highlight(
    lines: Vec<Line<'static>>,
    line_offset: usize,
    sel: Option<((usize, usize), (usize, usize))>,
) -> Vec<Line<'static>> {
    let Some(sel) = sel else {
        return lines;
    };
    let ((sl, sc), (el, ec)) = sel;
    let (lo_line, lo_col, hi_line, hi_col) = if sl < el || (sl == el && sc <= ec) {
        (sl, sc, el, ec)
    } else {
        (el, ec, sl, sc)
    };

    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let global = line_offset + i;
            if global < lo_line || global > hi_line {
                return line;
            }
            let line_width: usize = line.spans.iter().map(|s| s.width()).sum();
            let sel_start_col = if global == lo_line {
                lo_col.min(line_width)
            } else {
                0
            };
            let sel_end_col = if global == hi_line {
                hi_col.min(line_width)
            } else {
                line_width
            };
            if sel_start_col >= sel_end_col {
                return line;
            }

            let mut highlighted_spans: Vec<Span<'static>> = Vec::new();
            let mut col_acc: usize = 0;
            for sp in line.spans {
                let span_width = sp.width();
                let span_start = col_acc;
                let span_end = col_acc + span_width;

                if span_end <= sel_start_col || span_start >= sel_end_col {
                    highlighted_spans.push(sp);
                } else if span_start >= sel_start_col && span_end <= sel_end_col {
                    highlighted_spans.push(Span::styled(
                        sp.content,
                        sp.style.bg(theme::USER).fg(Color::Black),
                    ));
                } else {
                    let content = sp.content.as_ref();
                    if span_start < sel_start_col && span_end > sel_end_col {
                        // Three-way split
                        let mut tmp_col = span_start;
                        let left_chars = content
                            .chars()
                            .take_while(|c| {
                                let w = char_width(*c);
                                if tmp_col < sel_start_col {
                                    tmp_col += w;
                                    true
                                } else {
                                    false
                                }
                            })
                            .count();
                        highlighted_spans.push(Span::styled(
                            content.chars().take(left_chars).collect::<String>(),
                            sp.style,
                        ));
                        let mid_chars = content
                            .chars()
                            .skip(left_chars)
                            .take_while(|c| {
                                let w = char_width(*c);
                                if tmp_col < sel_end_col {
                                    tmp_col += w;
                                    true
                                } else {
                                    false
                                }
                            })
                            .count();
                        let mid: String =
                            content.chars().skip(left_chars).take(mid_chars).collect();
                        if !mid.is_empty() {
                            highlighted_spans
                                .push(Span::styled(mid, sp.style.bg(theme::USER).fg(Color::Black)));
                        }
                        let right: String = content.chars().skip(left_chars + mid_chars).collect();
                        if !right.is_empty() {
                            highlighted_spans.push(Span::styled(right, sp.style));
                        }
                    } else if span_start < sel_start_col {
                        let mut tmp_col = span_start;
                        let left_chars = content
                            .chars()
                            .take_while(|c| {
                                let w = char_width(*c);
                                if tmp_col < sel_start_col {
                                    tmp_col += w;
                                    true
                                } else {
                                    false
                                }
                            })
                            .count();
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
                        let mut tmp_col = span_start;
                        let inside_chars = content
                            .chars()
                            .take_while(|c| {
                                let w = char_width(*c);
                                if tmp_col < sel_end_col {
                                    tmp_col += w;
                                    true
                                } else {
                                    false
                                }
                            })
                            .count();
                        let inside: String = content.chars().take(inside_chars).collect();
                        let outside: String = content.chars().skip(inside_chars).collect();
                        if !inside.is_empty() {
                            highlighted_spans.push(Span::styled(
                                inside,
                                sp.style.bg(theme::USER).fg(Color::Black),
                            ));
                        }
                        if !outside.is_empty() {
                            highlighted_spans.push(Span::styled(outside, sp.style));
                        }
                    }
                }
                col_acc += span_width;
            }
            Line::from(highlighted_spans)
        })
        .collect()
}

// ── emit_* functions (virtualized rendering) ─────────────────────

/// Emit lines for a single block, with skip/take virtualization.
pub(super) fn emit_block_lines(
    block: &DisplayBlock,
    bi: usize,
    w: usize,
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<LineAnswerHit>,
    skip: usize,
    max_lines: usize,
) {
    let mut emitted = 0usize;
    let mut skipped = 0usize;
    let mut push = |line: Line<'static>, hit: LineAnswerHit| {
        if skipped < skip {
            skipped += 1;
            return;
        }
        if emitted >= max_lines {
            return;
        }
        lines.push(line);
        hits.push(hit);
        emitted += 1;
    };
    match block {
        DisplayBlock::User(content) => {
            push(
                Line::from(vec![Span::styled(
                    " YOU ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(theme::USER)
                        .add_modifier(Modifier::BOLD),
                )]),
                None,
            );
            push(Line::default(), None);
            for tl in wrap_text(content, w) {
                push(
                    Line::from(Span::styled(tl, Style::default().fg(theme::TEXT))),
                    None,
                );
            }
            push(Line::default(), None);
        }
        DisplayBlock::Assistant(content) => {
            push(
                Line::from(vec![Span::styled(
                    " nca ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(theme::ASSISTANT)
                        .add_modifier(Modifier::BOLD),
                )]),
                None,
            );
            push(Line::default(), None);
            for tl in wrap_text(content, w) {
                push(parse_md_line(&tl), None);
            }
            push(Line::default(), None);
        }
        DisplayBlock::ToolRunning { name, input, .. } => {
            let name_budget = w.saturating_sub(5); // " ⚡ " + " …"
            push(
                Line::from(vec![
                    Span::styled(" ⚡ ", Style::default().fg(theme::TOOL)),
                    Span::styled(
                        format!("{} ", truncate_to_width(name, name_budget)),
                        Style::default()
                            .fg(theme::TOOL)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("…", Style::default().fg(theme::MUTED)),
                ]),
                None,
            );
            let cmd = serde_json::from_str::<serde_json::Value>(input)
                .ok()
                .and_then(|v| v.get("command").and_then(|v| v.as_str().map(String::from)))
                .unwrap_or_default();
            if !cmd.is_empty() {
                for tl in wrap_text(&cmd, w.saturating_sub(3)) {
                    push(
                        Line::from(Span::styled(
                            format!("   {tl}"),
                            Style::default().fg(theme::MUTED),
                        )),
                        None,
                    );
                }
            }
        }
        DisplayBlock::ApprovalPending(req) => {
            push(
                Line::from(vec![Span::styled(
                    " 🔒 ",
                    Style::default()
                        .fg(theme::WARN)
                        .add_modifier(Modifier::BOLD),
                )]),
                None,
            );
            push(Line::default(), None);
            for tl in wrap_text(&req.description, w) {
                push(
                    Line::from(Span::styled(tl, Style::default().fg(theme::TEXT))),
                    None,
                );
            }
            push(Line::default(), None);
            push(
                Line::from(Span::styled("  input:", Style::default().fg(theme::MUTED))),
                None,
            );
            for tl in wrap_preformatted(&req.input, w) {
                push(
                    Line::from(Span::styled(
                        format!("    {tl}"),
                        Style::default().fg(theme::MUTED),
                    )),
                    None,
                );
            }
            push(Line::default(), None);
            push(
                Line::from(vec![
                    Span::styled("  y/yes ", Style::default().fg(theme::SUCCESS)),
                    Span::styled("approve · ", Style::default().fg(theme::MUTED)),
                    Span::styled("n/no ", Style::default().fg(theme::ERROR)),
                    Span::styled("deny", Style::default().fg(theme::MUTED)),
                ]),
                None,
            );
            push(Line::default(), None);
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
            let label_width = display_width(label);
            let tool_budget = w.saturating_sub(label_width + 1);
            push(
                Line::from(vec![
                    Span::styled(label, style.add_modifier(Modifier::BOLD)),
                    Span::styled(
                        format!(" {}", truncate_to_width(tool, tool_budget)),
                        Style::default().fg(theme::TEXT),
                    ),
                ]),
                None,
            );
            push(Line::default(), None);
        }
        DisplayBlock::ToolDone {
            name,
            input,
            ok: _,
            detail,
            full_output,
            expanded,
            duration_ms,
        } => {
            let icon = "✓";
            let st = Style::default().fg(theme::SUCCESS);
            let all_l: Vec<String> = wrap_text(full_output, w);
            let total = all_l.len();
            let preview = 3usize;
            let is_exp = *expanded;
            let show = if is_exp || total <= preview {
                total
            } else {
                preview
            };
            let prefix_width = 3 + name.chars().count() + 3; // " icon " + name + " — "
            let detail_budget = w.saturating_sub(prefix_width);
            push(
                Line::from(vec![
                    Span::styled(format!(" {icon} "), st),
                    Span::styled(
                        name.to_string(),
                        Style::default()
                            .fg(theme::TOOL)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" — {}", truncate_to_width(detail, detail_budget)),
                        Style::default().fg(theme::MUTED),
                    ),
                    Span::styled(
                        format!(" · {}", format_duration(*duration_ms)),
                        Style::default().fg(theme::MUTED),
                    ),
                ]),
                None,
            );
            let cmd = serde_json::from_str::<serde_json::Value>(input)
                .ok()
                .and_then(|v| v.get("command").and_then(|v| v.as_str().map(String::from)))
                .unwrap_or_default();
            if !cmd.is_empty() {
                for tl in wrap_text(&cmd, w.saturating_sub(3)) {
                    push(
                        Line::from(Span::styled(
                            format!("   {tl}"),
                            Style::default().fg(theme::MUTED),
                        )),
                        None,
                    );
                }
            }
            for tl in &all_l[..show] {
                push(
                    Line::from(Span::styled(tl.clone(), Style::default().fg(theme::MUTED))),
                    None,
                );
            }
            if total > preview {
                let label: String = if is_exp {
                    format!(" ▾ hide {name} output ")
                } else {
                    format!(" ▸ show {name} output ({}/{}) ", show, total)
                };
                push(
                    Line::from(vec![
                        Span::styled(label, Style::default().fg(theme::TOOL)),
                        Span::styled("(click)", Style::default().fg(theme::MUTED)),
                    ]),
                    Some(TranscriptHit::ToggleToolOutput(bi)),
                );
            }
            push(Line::default(), None);
        }
        DisplayBlock::System(s) => {
            for text_line in wrap_text(s, w) {
                push(
                    Line::from(Span::styled(text_line, Style::default().fg(theme::WARN))),
                    None,
                );
            }
        }
        DisplayBlock::Question(q) => {
            push(
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
            push(Line::default(), None);
            for tl in wrap_text(&q.prompt, w) {
                push(
                    Line::from(Span::styled(tl, Style::default().fg(theme::TEXT))),
                    None,
                );
            }
            push(
                Line::from(vec![Span::styled(
                    {
                        let prefix = "  [0] suggested: ".to_string();
                        let suffix = " (click)".to_string();
                        let budget =
                            w.saturating_sub(display_width(&prefix) + display_width(&suffix));
                        format!(
                            "{}{} {} ",
                            prefix,
                            truncate_to_width(&q.suggested_answer, budget),
                            suffix
                        )
                    },
                    Style::default()
                        .fg(theme::SUCCESS)
                        .add_modifier(Modifier::UNDERLINED),
                )]),
                Some(TranscriptHit::Question(
                    nca_common::event::QuestionSelection::Suggested,
                )),
            );
            for (i, o) in q.options.iter().enumerate() {
                push(
                    Line::from(vec![Span::styled(
                        {
                            let prefix = format!("  [{}] ({}) ", i + 1, o.id);
                            let suffix = " (click)".to_string();
                            let budget =
                                w.saturating_sub(display_width(&prefix) + display_width(&suffix));
                            format!(
                                "{}{} {} ",
                                prefix,
                                truncate_to_width(&o.label, budget),
                                suffix
                            )
                        },
                        Style::default()
                            .fg(theme::TEXT)
                            .add_modifier(Modifier::UNDERLINED),
                    )]),
                    Some(TranscriptHit::Question(
                        nca_common::event::QuestionSelection::Option {
                            option_id: o.id.clone(),
                        },
                    )),
                );
            }
            if q.allow_custom {
                push(
                    Line::from(Span::styled(
                        truncate_to_width("  [c] type your own answer below, then Enter", w),
                        Style::default().fg(theme::MUTED),
                    )),
                    None,
                );
            }
            push(
                Line::from(Span::styled(
                    truncate_to_width(
                        "  Tip: /auto-answer or Enter on empty = suggested · click an option above",
                        w,
                    ),
                    Style::default().fg(theme::MUTED),
                )),
                None,
            );
            push(Line::default(), None);
        }
        DisplayBlock::Thinking {
            content,
            expanded,
            duration_ms,
        } => {
            let all_l: Vec<String> = wrap_text(content, w);
            let total = all_l.len();
            let is_exp = *expanded;
            let preview = 3usize;
            let show = if is_exp || total <= preview {
                total
            } else {
                preview
            };
            let mut title_spans = vec![Span::styled(
                " 💭 thinking ",
                Style::default().fg(theme::MUTED),
            )];
            if let Some(ms) = duration_ms {
                title_spans.push(Span::styled(
                    format!(" · {}", format_duration(*ms)),
                    Style::default().fg(theme::MUTED),
                ));
            }
            push(Line::from(title_spans), None);
            for tl in &all_l[..show] {
                push(
                    Line::from(Span::styled(tl.clone(), Style::default().fg(theme::MUTED))),
                    None,
                );
            }
            if total > preview {
                let label: String = if is_exp {
                    " ▾ hide thinking ".into()
                } else {
                    format!(" ▸ show thinking ({}/{}) ", show, total)
                };
                push(
                    Line::from(vec![
                        Span::styled(label, Style::default().fg(theme::TOOL)),
                        Span::styled("(click)", Style::default().fg(theme::MUTED)),
                    ]),
                    Some(TranscriptHit::ToggleThinking(bi)),
                );
            }
            push(Line::default(), None);
        }
        DisplayBlock::ErrorLine(s) => {
            let budget = w.saturating_sub(3); // " ✗ "
            push(
                Line::from(Span::styled(
                    format!(" ✗ {}", truncate_to_width(s, budget)),
                    Style::default().fg(theme::ERROR),
                )),
                None,
            );
        }
        DisplayBlock::TurnInfo { duration_ms } => {
            push(
                Line::from(Span::styled(
                    format!("⏱ turn completed · {}", format_duration(*duration_ms)),
                    Style::default().fg(theme::MUTED),
                )),
                None,
            );
        }
    }
}

/// Virtualized streaming reasoning lines.
pub(super) fn emit_streaming_reasoning_lines(
    state: &TranscriptState,
    w: usize,
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<LineAnswerHit>,
    skip: usize,
    max_lines: usize,
) {
    let reasoning = state.streaming_reasoning.as_deref().unwrap_or("");
    let all_rl: Vec<String> = wrap_text(reasoning, w);
    let total_rl = all_rl.len();
    let preview_rl = 5usize;
    let show_rl = if state.streaming_reasoning_expanded || total_rl <= preview_rl {
        total_rl
    } else {
        preview_rl
    };
    let mut emitted = 0usize;
    let mut skipped = 0usize;
    let mut push = |line: Line<'static>, hit: LineAnswerHit| {
        if skipped < skip {
            skipped += 1;
            return;
        }
        if emitted >= max_lines {
            return;
        }
        lines.push(line);
        hits.push(hit);
        emitted += 1;
    };
    let mut title = vec![
        Span::styled(" 💭 thinking ", Style::default().fg(theme::MUTED)),
        Span::styled("…", Style::default().fg(theme::MUTED)),
    ];
    // Live elapsed timer while thinking; redrawn at the busy animation cadence.
    if let Some(start) = state.reasoning_started_at {
        let elapsed_ms = start.elapsed().as_millis() as u64;
        title.push(Span::styled(
            format!(" · {}", format_duration(elapsed_ms)),
            Style::default().fg(theme::MUTED),
        ));
    }
    push(Line::from(title), None);
    for rl in &all_rl[..show_rl] {
        push(
            Line::from(Span::styled(rl.clone(), Style::default().fg(theme::MUTED))),
            None,
        );
    }
    if total_rl > preview_rl {
        let label: String = if state.streaming_reasoning_expanded {
            " ▾ hide thinking ".into()
        } else {
            format!(" ▸ show thinking ({}/{}) ", show_rl, total_rl)
        };
        push(
            Line::from(vec![
                Span::styled(label, Style::default().fg(theme::TOOL)),
                Span::styled("(click)", Style::default().fg(theme::MUTED)),
            ]),
            Some(TranscriptHit::ToggleStreamingThinking),
        );
    }
    push(Line::default(), None);
}

/// Virtualized streaming assistant lines.
pub(super) fn emit_streaming_assistant_lines(
    state: &TranscriptState,
    w: usize,
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<LineAnswerHit>,
    skip: usize,
    max_lines: usize,
) {
    let stream = state.streaming_assistant.as_deref().unwrap_or("");
    if stream.is_empty() {
        return;
    }
    let mut emitted = 0usize;
    let mut skipped = 0usize;
    let mut push = |line: Line<'static>, hit: LineAnswerHit| {
        if skipped < skip {
            skipped += 1;
            return;
        }
        if emitted >= max_lines {
            return;
        }
        lines.push(line);
        hits.push(hit);
        emitted += 1;
    };
    push(
        Line::from(vec![Span::styled(
            " nca ",
            Style::default()
                .fg(Color::Black)
                .bg(theme::ASSISTANT)
                .add_modifier(Modifier::BOLD),
        )]),
        None,
    );
    push(Line::default(), None);
    for tl in wrap_text(stream, w) {
        push(parse_md_line(&tl), None);
    }
}

/// Virtualized empty-state fallback.
pub(super) fn emit_empty_fallback_lines(
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<LineAnswerHit>,
    skip: usize,
    max_lines: usize,
) {
    let mut emitted = 0usize;
    let mut skipped = 0usize;
    let mut push = |line: Line<'static>, hit: LineAnswerHit| {
        if skipped < skip {
            skipped += 1;
            return;
        }
        if emitted >= max_lines {
            return;
        }
        lines.push(line);
        hits.push(hit);
        emitted += 1;
    };
    push(
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
    push(Line::default(), None);
    push(
        Line::from(Span::styled(
            "Tab  agent   Ctrl+V  image   Ctrl+P  commands   !cmd  shell   @path  search   /  inline   PgUp/Dn  scroll\n\n                drag to select  ·  release to copy to clipboard",
            Style::default().fg(theme::MUTED),
        )),
        None,
    );
}
