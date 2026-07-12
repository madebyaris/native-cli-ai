//! Transcript rendering: converting session state into styled `ratatui` lines.
//!
//! Extracted from `tui/app.rs` in Phase 2.2. These helpers are the hot path
//! every TUI frame hits on dirty redraws; see `docs/research/baselines.md`
//! for `wrap_text` / `parse_md_line` numbers.

use crate::tui::busy_indicator;
use crate::tui::state::{ApprovalRequest, DisplayBlock, TranscriptCache, TuiSessionState};
use crate::tui::theme;
use nca_common::event::{BusyState, QuestionSelection};
use pulldown_cmark::{Event as MdEvent, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Per flattened transcript line: click selects this answer for `question_id`
/// (same indices as `transcript_lines_and_hits`).
pub type LineAnswerHit = Option<(String, QuestionSelection)>;

#[inline]
pub fn push_transcript_line(
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<LineAnswerHit>,
    line: Line<'static>,
    hit: LineAnswerHit,
) {
    lines.push(line);
    hits.push(hit);
}

/// Build scrollable transcript lines + optional mouse/click targets per line
/// straight from state (no caching). The draw path still uses this to keep the
/// closure simple; callers outside draw (mouse handlers, benchmarks) should go
/// through [`ensure_transcript_cache`] to avoid rebuilding.
pub fn transcript_lines_and_hits(
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
                for text_line in render_markdown_block(content, w) {
                    push_transcript_line(&mut lines, &mut hits, text_line, None);
                }
                push_transcript_line(&mut lines, &mut hits, Line::default(), None);
            }
            DisplayBlock::ToolRunning { name, input, .. } => {
                let summary = tool_running_summary(name, input);
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
                        Span::styled(format!("{summary} "), Style::default().fg(theme::TEXT)),
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
                if !state.question_modal_open() {
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
                        Some((q.question_id.clone(), QuestionSelection::Suggested)),
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
                            Some((
                                q.question_id.clone(),
                                QuestionSelection::Option {
                                    option_id: o.id.clone(),
                                },
                            )),
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
        for text_line in render_markdown_block(stream, w) {
            push_transcript_line(&mut lines, &mut hits, text_line, None);
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
                "Tab  agent   Ctrl+V  image   Ctrl+Shift+C  copy   Ctrl+P  commands   !cmd  shell   @path  search   /  inline   wheel  scroll",
                Style::default().fg(theme::MUTED),
            )),
            None,
        );
    }

    (lines, hits)
}

/// Ensure `state.transcript_cache` is populated for the given width + current
/// `transcript_version`, rebuilding only when invalidated. Used on paths that
/// don't draw (mouse clicks, scrolls, benches) so we don't re-wrap the whole
/// transcript per event.
pub fn ensure_transcript_cache(state: &mut TuiSessionState, width: u16) -> &TranscriptCache {
    let needs_build = state
        .transcript_cache
        .as_ref()
        .map(|c| !c.is_valid(state.transcript_version, width))
        .unwrap_or(true);
    if needs_build {
        let (lines, hits) = transcript_lines_and_hits(state, width);
        state.transcript_cache = Some(TranscriptCache {
            built_for_version: state.transcript_version,
            built_for_width: width,
            lines,
            hits,
        });
    }
    state
        .transcript_cache
        .as_ref()
        .expect("cache was just ensured")
}

#[allow(dead_code)]
pub fn transcript_lines(state: &TuiSessionState, width: u16) -> Vec<Line<'static>> {
    transcript_lines_and_hits(state, width).0
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

/// One-line path/command summary for an in-flight tool call.
pub fn tool_running_summary(name: &str, input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return "…".into();
    }
    // Prefer the first non-empty line (one-liners from format_tool_input_for_display).
    let first = trimmed.lines().next().unwrap_or(trimmed).trim();
    if (name == "write_file" || name == "edit_file" || name == "read_file")
        && !first.starts_with('{')
    {
        return truncate_chars(first, 72);
    }
    truncate_chars(first, 72)
}

/// Live footer lines appended outside the transcript cache so the elapsed
/// timer keeps ticking on busy animation frames without rebuilding the cache.
pub fn live_activity_lines(state: &TuiSessionState) -> Vec<Line<'static>> {
    let secs = state.busy_state_since.elapsed().as_secs();
    let elapsed_ms = state.busy_state_since.elapsed().as_millis();
    let frame = busy_indicator::frame_for_state(state.current_busy_state, elapsed_ms);
    let color = busy_indicator::color_for_state(state.current_busy_state);

    let detail = match state.current_busy_state {
        BusyState::Thinking => format!("{frame} waiting for model · {secs}s"),
        BusyState::Streaming => {
            let n = state
                .streaming_assistant
                .as_ref()
                .map(|s| s.chars().count())
                .unwrap_or(0);
            format!("{frame} streaming · {n} chars · {secs}s")
        }
        BusyState::ToolRunning => {
            let summary = state.blocks.iter().rev().find_map(|b| match b {
                DisplayBlock::ToolRunning { name, input, .. } => {
                    Some(format!("{name} · {}", tool_running_summary(name, input)))
                }
                _ => None,
            });
            match summary {
                Some(s) => format!("{frame} {s} · {secs}s"),
                None => format!("{frame} running tool · {secs}s"),
            }
        }
        BusyState::ApprovalPending => format!("{frame} waiting for approval · {secs}s"),
        BusyState::Error | BusyState::Idle => return Vec::new(),
    };

    vec![
        Line::default(),
        Line::from(Span::styled(
            format!("  {detail}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )),
    ]
}

pub fn wrap_text(s: &str, width: usize) -> Vec<String> {
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
        for word in paragraph.split_whitespace() {
            if line.is_empty() {
                line = word.to_string();
            } else if line.len() + 1 + word.len() <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(line);
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    if out.is_empty() && !s.is_empty() {
        out.push(s.to_string());
    }
    out
}

pub fn wrap_preformatted_line(line: &str, width: usize) -> Vec<String> {
    if width < 4 || line.is_empty() {
        return vec![line.to_string()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for ch in line.chars() {
        if current_len >= width {
            out.push(current);
            current = String::new();
            current_len = 0;
        }
        current.push(ch);
        current_len += 1;
    }
    if out.is_empty() || !current.is_empty() {
        out.push(current);
    }
    out
}

pub fn push_wrapped_plain_lines(
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

pub fn render_approval_block(
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
                " approve ",
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::WARN)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}", pretty_tool_label(&req.tool)),
                Style::default()
                    .fg(theme::WARN)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        None,
    );
    push_transcript_line(lines, hits, Line::default(), None);
    if !req.description.is_empty()
        && req.description != format!("Tool `{}` requires approval", req.tool)
    {
        for text_line in wrap_text(&req.description, width) {
            push_transcript_line(
                lines,
                hits,
                Line::from(Span::styled(text_line, Style::default().fg(theme::TEXT))),
                None,
            );
        }
        push_transcript_line(lines, hits, Line::default(), None);
    }

    let preview = pretty_approval_input(&req.tool, &req.input);
    for text_line in wrap_preformatted_line(&preview, width) {
        push_transcript_line(
            lines,
            hits,
            Line::from(Span::styled(
                format!("  {text_line}"),
                Style::default().fg(theme::TEXT),
            )),
            None,
        );
    }
    push_transcript_line(lines, hits, Line::default(), None);
    push_transcript_line(
        lines,
        hits,
        Line::from(vec![
            Span::styled(
                " y ",
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" approve  ", Style::default().fg(theme::MUTED)),
            Span::styled(
                " n ",
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::ERROR)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" deny  ", Style::default().fg(theme::MUTED)),
            Span::styled(
                " Ctrl+U ",
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::ASSISTANT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" always allow", Style::default().fg(theme::MUTED)),
        ]),
        None,
    );
    push_transcript_line(lines, hits, Line::default(), None);
}

fn pretty_tool_label(tool: &str) -> String {
    match tool {
        "execute_bash" => "run command".into(),
        "delete_path" => "delete path".into(),
        other => other.replace('_', " "),
    }
}

fn pretty_approval_input(tool: &str, raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.trim().to_string();
    };
    if tool == "execute_bash"
        && let Some(cmd) = value.get("command").and_then(|v| v.as_str())
    {
        return format!("$ {cmd}");
    }
    if let Some(path) = value
        .get("path")
        .or_else(|| value.get("file_path"))
        .and_then(|v| v.as_str())
    {
        return path.to_string();
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| raw.to_string())
}

/// Parse user approval input (flexible: punctuation, synonyms, `/approve` style).
pub fn parse_approval_verdict(line: &str) -> Option<bool> {
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

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

fn syntect_to_ratatui(style: SynStyle) -> Style {
    Style::default().fg(Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    ))
}

/// Render a markdown fragment to wrapped styled lines (headings, lists, code).
pub fn render_markdown_block(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(text, options);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut current = Line::from(Vec::<Span<'static>>::new());
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut list_depth = 0usize;

    let flush_line = |out: &mut Vec<Line<'static>>, current: &mut Line<'static>| {
        if !current.spans.is_empty() || !current.style.add_modifier.is_empty() {
            out.push(current.clone());
            *current = Line::from(Vec::<Span<'static>>::new());
        }
    };

    for event in parser {
        match event {
            MdEvent::Start(Tag::CodeBlock(kind)) => {
                flush_line(&mut out, &mut current);
                in_code = true;
                code_buf.clear();
                code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
            }
            MdEvent::End(TagEnd::CodeBlock) => {
                in_code = false;
                let highlighted = highlight_code_block(&code_lang, &code_buf, width);
                out.extend(highlighted);
                code_buf.clear();
            }
            MdEvent::Code(text) => {
                current.spans.push(Span::styled(
                    text.to_string(),
                    Style::default().fg(theme::TOOL),
                ));
            }
            MdEvent::Start(Tag::Heading { .. }) => flush_line(&mut out, &mut current),
            MdEvent::End(TagEnd::Heading(_)) => flush_line(&mut out, &mut current),
            MdEvent::Start(Tag::List(_)) => list_depth += 1,
            MdEvent::End(TagEnd::List(_)) => list_depth = list_depth.saturating_sub(1),
            MdEvent::Start(Tag::Item) => {
                flush_line(&mut out, &mut current);
                let pad = "  ".repeat(list_depth.saturating_sub(1));
                current.spans.push(Span::styled(
                    format!("{pad}- "),
                    Style::default().fg(theme::MUTED),
                ));
            }
            MdEvent::End(TagEnd::Item) => flush_line(&mut out, &mut current),
            MdEvent::Start(Tag::Strong) => {}
            MdEvent::End(TagEnd::Strong) => {}
            MdEvent::Start(Tag::Emphasis) => {}
            MdEvent::End(TagEnd::Emphasis) => {}
            MdEvent::Text(t) if in_code => code_buf.push_str(&t),
            MdEvent::Text(t) => {
                for wrapped in wrap_text(&t, width) {
                    if !current.spans.is_empty() {
                        flush_line(&mut out, &mut current);
                    }
                    current
                        .spans
                        .push(Span::styled(wrapped, Style::default().fg(theme::TEXT)));
                    flush_line(&mut out, &mut current);
                }
            }
            MdEvent::SoftBreak | MdEvent::HardBreak => flush_line(&mut out, &mut current),
            _ => {}
        }
    }
    flush_line(&mut out, &mut current);
    if out.is_empty() {
        out.push(Line::from(Span::styled(
            text.to_string(),
            Style::default().fg(theme::TEXT),
        )));
    }
    out
}

fn highlight_code_block(lang: &str, code: &str, width: usize) -> Vec<Line<'static>> {
    let ss = syntax_set();
    let ts = theme_set();
    let syntax = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let theme = &ts.themes["base16-ocean.dark"];
    let mut h = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(code) {
        let mut spans = Vec::new();
        if let Ok(parts) = h.highlight_line(line, ss) {
            for fragment in parts {
                spans.push(Span::styled(
                    fragment.1.to_string(),
                    syntect_to_ratatui(fragment.0),
                ));
            }
        }
        if spans.is_empty() {
            for wrapped in wrap_text(line.trim_end(), width.saturating_sub(2)) {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default().fg(theme::MUTED)),
                    Span::styled(wrapped, Style::default().fg(theme::MUTED)),
                ]));
            }
        } else {
            let mut row = vec![Span::styled("  ", Style::default().fg(theme::MUTED))];
            row.extend(spans);
            lines.push(Line::from(row));
        }
    }
    lines
}

pub fn parse_md_line(line: &str) -> Line<'static> {
    render_markdown_block(line, 120)
        .into_iter()
        .next()
        .unwrap_or_else(|| Line::from(Span::raw(line.to_string())))
}
