//! Criterion benches for TUI hot paths.

#![allow(
    clippy::all,
    clippy::pedantic,
    dead_code,
    unused_imports,
    unused_variables
)]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nca_tui::tui::transcript::{parse_md_line, render_markdown_block, wrap_text};
use std::hint::black_box;

fn bench_wrap_text(c: &mut Criterion) {
    let short = "quick brown fox jumps over the lazy dog";
    let medium = short.repeat(10);
    let long_para = (0..40)
        .map(|i| format!("This is sentence number {i} with enough words to force wrapping."))
        .collect::<Vec<_>>()
        .join(" ");
    let multi_para = (0..20)
        .map(|i| format!("Paragraph {i}: {}", "lorem ipsum dolor sit amet ".repeat(8)))
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut group = c.benchmark_group("wrap_text");
    for (label, input) in [
        ("short", short.to_string()),
        ("medium", medium.clone()),
        ("long_para", long_para.clone()),
        ("multi_para", multi_para.clone()),
    ] {
        for width in [40usize, 80, 120] {
            group.bench_with_input(
                BenchmarkId::new(label, width),
                &(input.clone(), width),
                |b, (s, w)| {
                    b.iter(|| black_box(wrap_text(black_box(s), black_box(*w))));
                },
            );
        }
    }
    group.finish();
}

fn bench_parse_md_line(c: &mut Criterion) {
    let plain = "This is a plain line with no formatting at all to measure baseline cost.";
    let bold = "Some **bold** text and **another bold** section and trailing plain text.";
    let code = "```rust fn demo() -> u32 { 42 } ```";
    let many_bolds = (0..20)
        .map(|i| format!("word{i} **bold{i}**"))
        .collect::<Vec<_>>()
        .join(" ");

    let mut group = c.benchmark_group("parse_md_line");
    for (label, input) in [
        ("plain", plain.to_string()),
        ("bold", bold.to_string()),
        ("code", code.to_string()),
        ("many_bolds", many_bolds),
    ] {
        group.bench_with_input(BenchmarkId::from_parameter(label), &input, |b, s| {
            b.iter(|| black_box(parse_md_line(black_box(s)).spans.len()));
        });
    }
    group.finish();
}

fn bench_render_markdown_block(c: &mut Criterion) {
    let sample = "# Title\n\nParagraph with **bold** and `code`.\n\n```rust\nfn main() {}\n```\n";
    c.bench_function("render_markdown_block_80", |b| {
        b.iter(|| black_box(render_markdown_block(black_box(sample), 80).len()));
    });
}

criterion_group!(
    tui_text_benches,
    bench_wrap_text,
    bench_parse_md_line,
    bench_render_markdown_block
);
criterion_main!(tui_text_benches);
