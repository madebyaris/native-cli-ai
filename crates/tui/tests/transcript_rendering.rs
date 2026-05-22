//! Snapshot tests for transcript rendering helpers.
//!
//! These exercise the pure rendering functions (no TUI state needed) so we
//! lock in stable output for wrap_text and parse_md_line, plus a full
//! `transcript_lines` pass over a realistic session fixture.

#![allow(
    clippy::all,
    clippy::pedantic,
    dead_code,
    unused_imports,
    unused_variables
)]

use std::path::PathBuf;

use nca_tui::tui::state::{DisplayBlock, TuiSessionState};
use nca_tui::tui::transcript::{parse_md_line, render_markdown_block, transcript_lines, wrap_text};
use ratatui::text::Line;

fn line_to_plain(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn lines_to_plain(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(line_to_plain)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn wrap_text_handles_unicode_and_long_words() {
    let input = "Rust is a systems programming language focused on safety, speed, and concurrency — featuring zero-cost abstractions and a friendly compiler.";
    let wrapped = wrap_text(input, 40);
    insta::assert_debug_snapshot!("wrap_text_40", wrapped);

    let wrapped_narrow = wrap_text(input, 20);
    insta::assert_debug_snapshot!("wrap_text_20", wrapped_narrow);
}

#[test]
fn wrap_text_preserves_explicit_newlines() {
    let input = "line one\nline two is a bit longer and should wrap\nline three";
    let wrapped = wrap_text(input, 24);
    insta::assert_debug_snapshot!("wrap_text_newlines_24", wrapped);
}

#[test]
fn parse_md_line_styles_common_markdown() {
    let samples = [
        "# Heading",
        "## Subheading",
        "- bullet item",
        "1. numbered item",
        "> quoted text",
        "plain paragraph",
        "`inline code`",
        "```rust",
        "    indented code block",
    ];

    let rendered: Vec<String> = samples
        .iter()
        .map(|s| format!("{:>30} => {}", s, line_to_plain(&parse_md_line(s))))
        .collect();

    insta::assert_debug_snapshot!("parse_md_line_samples", rendered);
}

#[test]
fn markdown_block_renders_list_items() {
    let lines = render_markdown_block("- alpha\n- beta", 40);
    let plain: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<Vec<_>>()
        .join("");
    insta::assert_snapshot!("markdown_list_block", plain);
}

#[test]
fn transcript_lines_full_session_snapshot() {
    let mut state = TuiSessionState::new(
        "sess-snapshot".into(),
        "MiniMax-M2.5".into(),
        "default".into(),
        "default".into(),
        PathBuf::from("/tmp/workspace"),
    );

    state.blocks.push(DisplayBlock::User(
        "Please read src/main.rs and summarize.".into(),
    ));
    state.blocks.push(DisplayBlock::ToolRunning {
        name: "read_file".into(),
        call_id: "call-1".into(),
        input: "{\"path\":\"src/main.rs\"}".into(),
        live_output: Vec::new(),
    });
    state.blocks.push(DisplayBlock::ToolDone {
        name: "read_file".into(),
        ok: true,
        detail: "42 lines".into(),
    });
    state.blocks.push(DisplayBlock::Assistant(
        "The file defines the `main` function and a helper.\n\n- Loads config.\n- Starts runtime.\n- Blocks on Ctrl-C.".into(),
    ));
    state
        .blocks
        .push(DisplayBlock::System("Session saved.".into()));

    let lines = transcript_lines(&state, 60);
    insta::assert_snapshot!("transcript_lines_full", lines_to_plain(&lines));
}
